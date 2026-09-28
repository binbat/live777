//! Regression tests for liveman's cascade routing on the edge-cloud
//! topology (`conf/livenil/edge-cloud/` made into an in-process cluster):
//!
//! - the first WHEP viewer of a stream is proxied to the edge node directly;
//! - a second viewer overflows the edge (`sub_max = 1` on liveman), so the
//!   stream is cascaded to the cloud node and the direct viewer is kicked
//!   (`close_other_sub`) while the hop itself is spared;
//! - further viewers land on the cloud; the edge never carries more than
//!   one outbound copy of the stream;
//! - after the last cloud viewer leaves, the hop is torn down and the next
//!   viewer goes direct to the edge again.
//!
//! Both cascade modes are covered: pull (the cloud WHEP-pulls the edge) and
//! push (the edge WHIP-pushes through liveman to the cloud).
//!
//! All nodes run in-process on ephemeral ports; every client goes through
//! liveman, and ground truth is read from the nodes' own `/api/streams/`.

#[cfg(feature = "cascade")]
mod common;

#[cfg(feature = "cascade")]
mod cascade_cluster {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::{Arc, Once};
    use std::time::Duration;

    use tokio::net::TcpListener;
    use tokio::sync::watch;
    use webrtc::media_stream::MediaStreamTrack;
    use webrtc::media_stream::track_local::TrackLocal;
    use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
    use webrtc::peer_connection::{
        MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
        RTCConfigurationBuilder, RTCPeerConnectionState, RTCSessionDescription,
    };
    use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};

    use rtc::rtp::header::Header;
    use rtc::rtp::packet::Packet;
    use rtc::rtp_transceiver::rtp_sender::{
        RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
    };

    use liveman::config::{CascadeCheckAttempts, CascadeMode, CheckCascadeTickTime};

    use crate::common::shutdown_signal;

    static TRACING_INIT: Once = Once::new();

    fn init_test_environment() {
        TRACING_INIT.call_once(|| {
            // All WebRTC peers run in this process; pin ICE candidates to
            // loopback so CI runners cannot choose an unroutable interface.
            unsafe {
                std::env::set_var("LIVE777_WEBRTC_ICE_UDP_ADDRS", "127.0.0.1:0");
            }
            let filter = std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "liveman=info,liveion=info".to_string());
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_test_writer()
                .try_init();
        });
    }

    struct Cluster {
        liveman: SocketAddr,
        edge0: SocketAddr,
        cloud: SocketAddr,
    }

    async fn boot_liveion(strategy_edit: impl FnOnce(&mut liveion::config::Config)) -> SocketAddr {
        let mut cfg = liveion::config::Config::default();
        strategy_edit(&mut cfg);
        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));
        addr
    }

    /// Two edge nodes (subscriber-capped via liveman's `sub_max`) plus one
    /// cloud node behind a single liveman. Nodes update liveman over SSE so
    /// routing decisions see fresh state.
    async fn boot_cluster(mode: CascadeMode) -> Cluster {
        let edge0 = boot_liveion(|_| {}).await;
        let edge1 = boot_liveion(|_| {}).await;
        let cloud = boot_liveion(|cfg| {
            // Torn-down cascaded streams must vanish quickly so the next
            // viewer goes direct to the edge again.
            cfg.strategy.auto_delete_whep = api::strategy::AutoDestrayTime(500);
        })
        .await;

        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .unwrap();
        let liveman = listener.local_addr().unwrap();

        let mut cfg = liveman::config::Config::default();
        cfg.http.listen = liveman;
        cfg.database.url = "sqlite::memory:".to_string();
        cfg.nodes = vec![
            liveman::config::Node {
                alias: "edge0".to_string(),
                url: format!("http://{edge0}"),
                mode: liveman::config::UpdateMode::Sse,
                sub_max: Some(1),
                ..Default::default()
            },
            liveman::config::Node {
                alias: "edge1".to_string(),
                url: format!("http://{edge1}"),
                mode: liveman::config::UpdateMode::Sse,
                sub_max: Some(1),
                ..Default::default()
            },
            liveman::config::Node {
                alias: "cloud".to_string(),
                url: format!("http://{cloud}"),
                mode: liveman::config::UpdateMode::Sse,
                ..Default::default()
            },
        ];
        cfg.cascade.mode = mode;
        cfg.cascade.close_other_sub = true;
        cfg.cascade.check_attempts = CascadeCheckAttempts(10);
        cfg.cascade.check_tick_time = CheckCascadeTickTime(1000);
        cfg.cascade.maximum_idle_time = 1000;
        cfg.validate().unwrap();

        tokio::spawn(liveman::serve(cfg, listener, shutdown_signal()));
        Cluster {
            liveman,
            edge0,
            cloud,
        }
    }

    #[derive(Clone)]
    struct StateHandler {
        state_tx: watch::Sender<RTCPeerConnectionState>,
    }

    #[async_trait::async_trait]
    impl PeerConnectionEventHandler for StateHandler {
        async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
            let _ = self.state_tx.send(state);
        }
    }

    async fn build_peer() -> (
        Arc<dyn PeerConnection>,
        watch::Receiver<RTCPeerConnectionState>,
    ) {
        let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
        let handler: Arc<dyn PeerConnectionEventHandler> = Arc::new(StateHandler { state_tx });
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs().unwrap();
        let peer: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::<SocketAddr>::new()
                .with_media_engine(media_engine)
                .with_handler(handler)
                .with_udp_addrs(livetwo::utils::webrtc::ice_udp_addrs())
                .with_configuration(RTCConfigurationBuilder::new().build())
                .build()
                .await
                .unwrap(),
        );
        (peer, state_rx)
    }

    async fn wait_connected(mut state_rx: watch::Receiver<RTCPeerConnectionState>, who: &str) {
        let ok = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                state_rx.changed().await.unwrap();
                match *state_rx.borrow() {
                    RTCPeerConnectionState::Connected => return true,
                    RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                        return false;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap_or(false);
        assert!(
            ok,
            "{who} did not reach Connected; last state: {:?}",
            state_rx.borrow()
        );
    }

    async fn post_sdp(client: &reqwest::Client, url: String, sdp: String) -> String {
        let res = client
            .post(url)
            .header(http::header::CONTENT_TYPE, "application/sdp")
            .body(sdp)
            .send()
            .await
            .unwrap();
        assert_eq!(http::StatusCode::CREATED, res.status());
        res.text().await.unwrap()
    }

    /// A WHEP viewer attached through liveman (never directly to a node).
    async fn whep_viewer(cluster: &Cluster, stream: &str, who: &str) -> Arc<dyn PeerConnection> {
        let (peer, state_rx) = build_peer().await;
        peer.add_transceiver_from_kind(
            RtpCodecKind::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                streams: vec![],
                send_encodings: vec![],
            }),
        )
        .await
        .unwrap();
        let offer = peer.create_offer(None).await.unwrap();
        peer.set_local_description(offer).await.unwrap();
        // Gather fully so the offer carries candidates (no trickle needed).
        tokio::time::sleep(Duration::from_millis(500)).await;
        let offer_sdp = peer.local_description().await.unwrap().sdp;

        let answer = post_sdp(
            &reqwest::Client::new(),
            format!("http://{}{}", cluster.liveman, api::path::whep(stream)),
            offer_sdp,
        )
        .await;
        peer.set_remote_description(RTCSessionDescription::answer(answer).unwrap())
            .await
            .unwrap();
        wait_connected(state_rx, who).await;
        peer
    }

    /// The camera: a WHIP publisher pinned to its edge node through liveman,
    /// so a new stream can never land on the cloud directly.
    async fn whip_publish(cluster: &Cluster, alias: &str, stream: &str) -> Arc<dyn PeerConnection> {
        let (peer, state_rx) = build_peer().await;
        let track: Arc<dyn TrackLocal> = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
            stream.to_string(),
            format!("{stream}-video"),
            stream.to_string(),
            RtpCodecKind::Video,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(0xca5cade0),
                    ..Default::default()
                },
                codec: RTCRtpCodec {
                    mime_type: "video/VP8".to_owned(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: String::new(),
                    rtcp_feedback: vec![],
                },
                ..Default::default()
            }],
        )));
        peer.add_track(track.clone()).await.unwrap();
        let offer = peer.create_offer(None).await.unwrap();
        peer.set_local_description(offer).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let offer_sdp = peer.local_description().await.unwrap().sdp;

        let answer = post_sdp(
            &reqwest::Client::new(),
            format!(
                "http://{}{}",
                cluster.liveman,
                api::path::whip_with_node(stream, alias)
            ),
            offer_sdp,
        )
        .await;
        peer.set_remote_description(RTCSessionDescription::answer(answer).unwrap())
            .await
            .unwrap();
        wait_connected(state_rx, "publisher").await;

        // Pump dummy VP8 packets so codec-readiness checks (WHEP sources,
        // subscriber answers) see real media. Stops on its own when the
        // peer closes (writes start failing).
        tokio::spawn(async move {
            let mut sequence_number: u16 = 0;
            let mut timestamp: u32 = 0;
            loop {
                let packet = Packet {
                    header: Header {
                        version: 2,
                        payload_type: 96,
                        sequence_number,
                        timestamp,
                        ssrc: 0xca5cade0,
                        ..Default::default()
                    },
                    // Minimal VP8 payload descriptor + dummy picture data;
                    // the SFU forwards RTP without parsing the payload.
                    payload: vec![0x10, 0x80, 0x01, 0x02, 0x03, 0x04].into(),
                };
                if track.write_rtp(packet).await.is_err() {
                    break;
                }
                sequence_number = sequence_number.wrapping_add(1);
                timestamp = timestamp.wrapping_add(3000);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
        peer
    }

    async fn streams_of(addr: SocketAddr) -> Vec<api::response::Stream> {
        reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    fn find_stream<'a>(
        streams: &'a [api::response::Stream],
        id: &str,
    ) -> Option<&'a api::response::Stream> {
        streams.iter().find(|s| s.id == id)
    }

    /// Subscribe sessions that still hold capacity: Closed sessions linger in
    /// the node listing for a display TTL but must not count.
    fn active_subs(stream: &api::response::Stream) -> Vec<&api::response::Session> {
        stream
            .subscribe
            .sessions
            .iter()
            .filter(|s| s.state != api::response::RTCPeerConnectionState::Closed)
            .collect()
    }

    async fn wait_until<F, Fut>(what: &str, timeout: Duration, mut f: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let ok = tokio::time::timeout(timeout, async {
            loop {
                if f().await {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .is_ok();
        assert!(ok, "timed out waiting for: {what}");
    }

    async fn delete_viewer(cluster: &Cluster, stream: &str, session: &str) {
        let res = reqwest::Client::new()
            .delete(format!(
                "http://{}/session/{}/{}",
                cluster.liveman, stream, session
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(http::StatusCode::NO_CONTENT, res.status());
    }

    async fn run_edge_cloud_loop(mode: CascadeMode) {
        init_test_environment();
        let pull = matches!(mode, CascadeMode::Pull);
        let cluster = boot_cluster(mode).await;
        let stream = "cam0";

        // --- camera publishes to its edge node (pinned through liveman) ---
        let publisher = whip_publish(&cluster, "edge0", stream).await;

        // liveman learns the stream over SSE before the first viewer.
        wait_until(
            "liveman sees cam0 on edge0",
            Duration::from_secs(10),
            || async {
                let res = reqwest::get(format!(
                    "http://{}{}",
                    cluster.liveman,
                    api::path::streams("")
                ))
                .await
                .unwrap();
                let body: Vec<serde_json::Value> = res.json().await.unwrap();
                body.iter().any(|s| s["id"] == stream)
            },
        )
        .await;

        // --- viewer 1 goes direct to the edge ---
        let v1 = whep_viewer(&cluster, stream, "viewer1").await;
        wait_until(
            "edge0 has exactly one direct viewer",
            Duration::from_secs(10),
            || async {
                let streams = streams_of(cluster.edge0).await;
                match find_stream(&streams, stream) {
                    Some(s) => active_subs(s).len() == 1,
                    None => false,
                }
            },
        )
        .await;
        assert!(
            find_stream(&streams_of(cluster.cloud).await, stream).is_none(),
            "cloud must not host the stream while a single viewer is direct"
        );

        // --- viewer 2 overflows the edge: cascade to the cloud, kick viewer 1 ---
        let v2 = whep_viewer(&cluster, stream, "viewer2").await;
        wait_until(
            "cloud has the cascaded stream with a connected publisher",
            Duration::from_secs(20),
            || async {
                let streams = streams_of(cluster.cloud).await;
                match find_stream(&streams, stream) {
                    Some(s) => s
                        .publish
                        .sessions
                        .iter()
                        .any(|p| p.state == api::response::RTCPeerConnectionState::Connected),
                    None => false,
                }
            },
        )
        .await;

        // The direct viewer is kicked; the hop itself survives on the edge —
        // exactly one outbound copy of the stream leaves the edge.
        wait_until(
            "viewer1 kicked off the edge, hop spared",
            Duration::from_secs(15),
            || async {
                let streams = streams_of(cluster.edge0).await;
                match find_stream(&streams, stream) {
                    Some(s) => {
                        let active = active_subs(s);
                        let kicked = s
                            .subscribe
                            .sessions
                            .iter()
                            .any(|x| x.state == api::response::RTCPeerConnectionState::Closed);
                        active.len() == 1 && kicked
                    }
                    None => false,
                }
            },
        )
        .await;
        let streams = streams_of(cluster.edge0).await;
        let hop = active_subs(find_stream(&streams, stream).unwrap());
        if pull {
            // A pull hop is an unmarked, ordinary subscriber on the source;
            // liveman spares it via the destination's cascade.session_url.
            assert!(hop[0].cascade.is_none());
        } else {
            // A push hop is self-marked by the source node.
            assert!(hop[0].cascade.is_some());
        }

        // --- viewer 3 (e.g. viewer1's player reconnecting) lands on the cloud ---
        let v3 = whep_viewer(&cluster, stream, "viewer3").await;
        wait_until(
            "cloud serves both viewers",
            Duration::from_secs(10),
            || async {
                let streams = streams_of(cluster.cloud).await;
                match find_stream(&streams, stream) {
                    Some(s) => active_subs(s).len() == 2,
                    None => false,
                }
            },
        )
        .await;
        let streams = streams_of(cluster.edge0).await;
        assert_eq!(
            active_subs(find_stream(&streams, stream).unwrap()).len(),
            1,
            "the edge must still carry exactly one outbound copy (the hop)"
        );

        // --- a broken hop is healed by the supervisor (pull mode) ---
        // Kill the hop on its edge side (the cloud's incoming WHEP session).
        // The viewers stay attached to the cloud throughout: the pull's dead
        // incumbent is replaced via the media-generation machinery, so there
        // is no publisher-leave teardown.
        //
        // Push mode is not covered here: a push hop killed through the
        // session API shuts down gracefully — the push client DELETEs the
        // session it created on the destination, and the destination's
        // publisher-leave teardown drops the viewers by design. The seamless
        // path (override-displacing a network-dead incoming push) cannot be
        // simulated over HTTP in-process.
        if pull {
            let streams = streams_of(cluster.edge0).await;
            let hop_id = active_subs(find_stream(&streams, stream).unwrap())
                .first()
                .map(|s| s.id.clone())
                .expect("the hop is the only active subscriber on the edge");
            let pre_kill_publish = find_stream(&streams_of(cluster.cloud).await, stream)
                .and_then(|s| s.publish.sessions.first().map(|p| p.id.clone()));
            let res = reqwest::Client::new()
                .delete(format!(
                    "http://{}/session/{}/{}",
                    cluster.edge0, stream, hop_id
                ))
                .send()
                .await
                .unwrap();
            assert_eq!(http::StatusCode::NO_CONTENT, res.status());

            wait_until(
                "supervisor re-establishes the killed hop",
                Duration::from_secs(20),
                || async {
                    let streams = streams_of(cluster.cloud).await;
                    match find_stream(&streams, stream) {
                        Some(s) => s.publish.sessions.iter().any(|p| {
                            p.state == api::response::RTCPeerConnectionState::Connected
                                && Some(&p.id) != pre_kill_publish.as_ref()
                        }),
                        None => false,
                    }
                },
            )
            .await;
            wait_until(
                "both viewers still attached after the hop rebuild",
                Duration::from_secs(10),
                || async {
                    let streams = streams_of(cluster.cloud).await;
                    match find_stream(&streams, stream) {
                        Some(s) => active_subs(s).len() == 2,
                        None => false,
                    }
                },
            )
            .await;
        }

        // --- all viewers leave gracefully: the hop is torn down ---
        let ids: Vec<String> = find_stream(&streams_of(cluster.cloud).await, stream)
            .unwrap()
            .subscribe
            .sessions
            .iter()
            .map(|s| s.id.clone())
            .collect();
        assert_eq!(ids.len(), 2);
        for id in ids {
            delete_viewer(&cluster, stream, &id).await;
        }
        wait_until(
            "cloud destroys the leftover stream (auto_delete_whep)",
            Duration::from_secs(15),
            || async { find_stream(&streams_of(cluster.cloud).await, stream).is_none() },
        )
        .await;
        wait_until(
            "the hop on the edge is gone",
            Duration::from_secs(20),
            || async {
                let streams = streams_of(cluster.edge0).await;
                active_subs(find_stream(&streams, stream).unwrap()).is_empty()
            },
        )
        .await;

        // --- the next viewer goes direct to the edge again ---
        let v4 = whep_viewer(&cluster, stream, "viewer4").await;
        wait_until(
            "viewer4 is direct on the edge",
            Duration::from_secs(10),
            || async {
                let streams = streams_of(cluster.edge0).await;
                active_subs(find_stream(&streams, stream).unwrap()).len() == 1
            },
        )
        .await;
        assert!(find_stream(&streams_of(cluster.cloud).await, stream).is_none());

        for peer in [publisher, v1, v2, v3, v4] {
            let _ = peer.close().await;
        }
    }

    #[tokio::test]
    async fn cascade_pull_mode_overflow_migration_loop() {
        run_edge_cloud_loop(CascadeMode::Pull).await;
    }

    #[tokio::test]
    async fn cascade_push_mode_overflow_migration_loop() {
        run_edge_cloud_loop(CascadeMode::Push).await;
    }

    /// Cluster operators manage per-node sources through liveman only
    /// (alias-pinned proxy of liveion's `/api/sources/...`): create a WHEP
    /// source on the cloud pulling from the edge, watch the bridge come up,
    /// query state, then delete it — all without touching the node directly.
    #[cfg(feature = "source-whep")]
    #[tokio::test]
    async fn liveman_proxies_source_management() {
        init_test_environment();
        let cluster = boot_cluster(CascadeMode::Pull).await;
        let stream = "cam0";
        let publisher = whip_publish(&cluster, "edge0", stream).await;

        wait_until(
            "liveman sees cam0 on edge0",
            Duration::from_secs(10),
            || async {
                let res = reqwest::get(format!(
                    "http://{}{}",
                    cluster.liveman,
                    api::path::streams("")
                ))
                .await
                .unwrap();
                let body: Vec<serde_json::Value> = res.json().await.unwrap();
                body.iter().any(|s| s["id"] == stream)
            },
        )
        .await;

        // Create a WHEP source on the cloud through liveman (alias-pinned).
        let res = reqwest::Client::new()
            .post(format!(
                "http://{}/api/sources/cloud/{}",
                cluster.liveman, stream
            ))
            .json(&serde_json::json!({
                "url": format!("whep://{}/whep/{}", cluster.edge0, stream)
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(http::StatusCode::OK, res.status());

        // The source bridges the stream onto the cloud.
        wait_until(
            "cloud hosts cam0 via the source",
            Duration::from_secs(15),
            || async {
                let streams = streams_of(cluster.cloud).await;
                match find_stream(&streams, stream) {
                    Some(s) => s
                        .publish
                        .sessions
                        .iter()
                        .any(|p| p.state == api::response::RTCPeerConnectionState::Connected),
                    None => false,
                }
            },
        )
        .await;

        // Query source info and the node-wide list through liveman.
        for url in [
            format!("http://{}/api/sources/cloud/{}", cluster.liveman, stream),
            format!("http://{}/api/sources/cloud", cluster.liveman),
        ] {
            let res = reqwest::get(url).await.unwrap();
            assert_eq!(http::StatusCode::OK, res.status());
        }

        // Eager registration: a viewer through liveman lands on the cloud
        // (whose source-bridge publish beats the edge's capped capacity).
        let viewer = whep_viewer(&cluster, stream, "source-viewer").await;
        wait_until(
            "cloud serves the viewer through its source",
            Duration::from_secs(10),
            || async {
                let streams = streams_of(cluster.cloud).await;
                match find_stream(&streams, stream) {
                    Some(s) => active_subs(s).len() == 1,
                    None => false,
                }
            },
        )
        .await;
        let _ = viewer.close().await;

        // Delete the source through liveman: the bridge and the stream's
        // virtual publisher go away.
        let res = reqwest::Client::new()
            .delete(format!(
                "http://{}/api/sources/cloud/{}",
                cluster.liveman, stream
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(http::StatusCode::OK, res.status());
        wait_until(
            "source removed from the cloud",
            Duration::from_secs(15),
            || async {
                let res = reqwest::get(format!("http://{}/api/sources", cluster.cloud))
                    .await
                    .unwrap();
                let body: serde_json::Value = res.json().await.unwrap();
                body["sources"].as_array().unwrap().is_empty()
            },
        )
        .await;

        let _ = publisher.close().await;
    }
}
