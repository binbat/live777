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
    use tokio::sync::{Notify, oneshot, watch};
    use webrtc::media_stream::MediaStreamTrack;
    use webrtc::media_stream::track_local::TrackLocal;
    use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
    use webrtc::peer_connection::{
        MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
        RTCConfigurationBuilder, RTCIceGatheringState, RTCPeerConnectionState,
        RTCSessionDescription,
    };
    use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};

    use rtc::rtp::header::Header;
    use rtc::rtp::packet::Packet;
    use rtc::rtp_transceiver::rtp_sender::{
        RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
    };

    use liveman::config::{CascadeMode, CheckCascadeTickTime};

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
        /// Kept alive for the cluster's lifetime; dropping it stops liveman.
        _liveman_shutdown: oneshot::Sender<()>,
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

    /// Build the liveman config for the three-node edge+cloud topology. The
    /// listener is bound separately so a test can restart liveman (new port,
    /// independent shutdown) against the same nodes.
    fn liveman_config(
        mode: CascadeMode,
        liveman: SocketAddr,
        edge0: SocketAddr,
        edge1: SocketAddr,
        cloud: SocketAddr,
    ) -> liveman::config::Config {
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
        cfg.cascade.check_tick_time = CheckCascadeTickTime(1000);
        cfg.cascade.maximum_idle_time = 1000;
        cfg.validate().unwrap();
        cfg
    }

    /// Spawn a liveman on an ephemeral port; the returned shutdown sender
    /// stops just this instance when dropped (the nodes keep running).
    async fn boot_liveman(
        mode: CascadeMode,
        edge0: SocketAddr,
        edge1: SocketAddr,
        cloud: SocketAddr,
    ) -> (SocketAddr, oneshot::Sender<()>) {
        let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        // Bound first: `http.public` (the pinned cascade endpoint nodes push
        // to) is derived from the real address by validate().
        let cfg = liveman_config(mode, addr, edge0, edge1, cloud);
        let (tx, rx) = oneshot::channel::<()>();
        tokio::spawn(liveman::serve(cfg, listener, async {
            let _ = rx.await;
        }));
        (addr, tx)
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

        let (liveman, shutdown) = boot_liveman(mode, edge0, edge1, cloud).await;
        Cluster {
            liveman,
            edge0,
            cloud,
            _liveman_shutdown: shutdown,
        }
    }

    #[derive(Clone)]
    struct StateHandler {
        state_tx: watch::Sender<RTCPeerConnectionState>,
        gather_complete: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl PeerConnectionEventHandler for StateHandler {
        async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
            let _ = self.state_tx.send(state);
        }
        async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
            if state == RTCIceGatheringState::Complete {
                self.gather_complete.notify_one();
            }
        }
    }

    async fn build_peer() -> (
        Arc<dyn PeerConnection>,
        watch::Receiver<RTCPeerConnectionState>,
        Arc<Notify>,
    ) {
        let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
        let gather_complete = Arc::new(Notify::new());
        let handler: Arc<dyn PeerConnectionEventHandler> = Arc::new(StateHandler {
            state_tx,
            gather_complete: gather_complete.clone(),
        });
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
        (peer, state_rx, gather_complete)
    }

    /// Wait for local ICE gathering to complete (bounded) so the offer
    /// carries candidates — no fixed sleeps.
    async fn gather_fully(gather_complete: &Notify) {
        tokio::time::timeout(Duration::from_secs(5), gather_complete.notified())
            .await
            .expect("ICE gathering did not complete");
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
    /// The returned state receiver lets callers assert what the viewer's own
    /// peer observes later (e.g. being kicked).
    async fn whep_viewer(
        cluster: &Cluster,
        stream: &str,
        who: &str,
    ) -> (
        Arc<dyn PeerConnection>,
        watch::Receiver<RTCPeerConnectionState>,
    ) {
        let (peer, state_rx, gather_complete) = build_peer().await;
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
        gather_fully(&gather_complete).await;
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
        wait_connected(state_rx.clone(), who).await;
        (peer, state_rx)
    }

    /// The camera: a WHIP publisher pinned to its edge node through liveman,
    /// so a new stream can never land on the cloud directly.
    async fn whip_publish(cluster: &Cluster, alias: &str, stream: &str) -> Arc<dyn PeerConnection> {
        let (peer, state_rx, gather_complete) = build_peer().await;
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
        gather_fully(&gather_complete).await;
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
        reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
            .get(format!("http://{addr}{}", api::path::streams("")))
            .send()
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

    /// liveman learns a stream exists (on any node) via SSE/poll snapshots.
    async fn wait_liveman_sees(cluster: &Cluster, stream: &str) {
        wait_until(
            "liveman sees the stream",
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
    }

    async fn run_edge_cloud_loop(mode: CascadeMode) {
        init_test_environment();
        let pull = matches!(mode, CascadeMode::Pull);
        let cluster = boot_cluster(mode).await;
        let stream = "cam0";

        // --- camera publishes to its edge node (pinned through liveman) ---
        let publisher = whip_publish(&cluster, "edge0", stream).await;

        // liveman learns the stream over SSE before the first viewer.
        wait_liveman_sees(&cluster, stream).await;

        // --- viewer 1 goes direct to the edge ---
        let (v1, mut v1_state) = whep_viewer(&cluster, stream, "viewer1").await;
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
        let (v2, _) = whep_viewer(&cluster, stream, "viewer2").await;
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

        // The kicked viewer's own peer must observe the disconnect — a kick
        // that only updates server-side listings would leave the player
        // silently stuck instead of reconnecting onto the cloud.
        let observed = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                v1_state.changed().await.unwrap();
                if matches!(
                    *v1_state.borrow(),
                    RTCPeerConnectionState::Closed
                        | RTCPeerConnectionState::Failed
                        | RTCPeerConnectionState::Disconnected
                ) {
                    return;
                }
            }
        })
        .await;
        assert!(
            observed.is_ok(),
            "kicked viewer1 never observed a terminal state; last state: {:?}",
            v1_state.borrow()
        );

        // --- viewer 3 (e.g. viewer1's player reconnecting) lands on the cloud ---
        let (v3, _) = whep_viewer(&cluster, stream, "viewer3").await;
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
                .and_then(|s| s.publish.sessions.first().map(|p| p.id.clone()))
                .expect("cloud must have a publish session before the kill");
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
                                && p.id != pre_kill_publish
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
        let (v4, _) = whep_viewer(&cluster, stream, "viewer4").await;
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

        wait_liveman_sees(&cluster, stream).await;

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
        let (viewer, _) = whep_viewer(&cluster, stream, "source-viewer").await;
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

    /// Two viewers arriving inside the same snapshot-propagation window must
    /// not both win direct routing to the capped edge: WHEP admission is
    /// serialized per stream and eagerly-recorded sessions count toward
    /// capacity, so the second viewer overflows to the cloud immediately.
    /// Runs in push mode, where the hop is self-marked and direct viewers
    /// stay distinguishable.
    #[tokio::test]
    async fn concurrent_viewers_never_oversubscribe_the_edge() {
        init_test_environment();
        let cluster = boot_cluster(CascadeMode::Push).await;
        let stream = "cam0";
        let publisher = whip_publish(&cluster, "edge0", stream).await;
        wait_liveman_sees(&cluster, stream).await;

        let ((v1, _), (v2, _)) = tokio::join!(
            whep_viewer(&cluster, stream, "race-viewer-1"),
            whep_viewer(&cluster, stream, "race-viewer-2"),
        );

        // At no instant may the edge carry two direct (unmarked) viewers —
        // before serialized admission, both raced viewers landed on the edge.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            let streams = streams_of(cluster.edge0).await;
            if let Some(s) = find_stream(&streams, stream) {
                let direct = s
                    .subscribe
                    .sessions
                    .iter()
                    .filter(|x| {
                        x.state != api::response::RTCPeerConnectionState::Closed
                            && x.cascade.is_none()
                    })
                    .count();
                assert!(
                    direct <= 1,
                    "edge oversubscribed with {direct} direct viewers"
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // And the overflow viewer did land on the cloud (through the hop).
        wait_until(
            "cloud serves a raced viewer",
            Duration::from_secs(20),
            || async {
                let streams = streams_of(cluster.cloud).await;
                match find_stream(&streams, stream) {
                    Some(s) => !active_subs(s).is_empty(),
                    None => false,
                }
            },
        )
        .await;

        for peer in [publisher, v1, v2] {
            let _ = peer.close().await;
        }
    }

    /// After a liveman restart, a hop found on the nodes without an intent
    /// is adopted while viewers remain (nobody tears it down), and once the
    /// last viewer leaves the adopted intent is reaped like any other.
    #[tokio::test]
    async fn liveman_restart_adopts_live_hop_and_reaps_it_when_idle() {
        init_test_environment();
        let edge0 = boot_liveion(|_| {}).await;
        let edge1 = boot_liveion(|_| {}).await;
        let cloud = boot_liveion(|cfg| {
            // No auto_delete_whep here: the assertions below must measure
            // liveman's reaping, not the node's own stream teardown.
            cfg.strategy.auto_delete_whep = api::strategy::AutoDestrayTime(-1);
        })
        .await;
        let stream = "cam0";

        // --- first liveman: publish, overflow, establish the push hop ---
        let (liveman, shutdown) = boot_liveman(CascadeMode::Push, edge0, edge1, cloud).await;
        let cluster1 = Cluster {
            liveman,
            edge0,
            cloud,
            _liveman_shutdown: shutdown,
        };
        let publisher = whip_publish(&cluster1, "edge0", stream).await;
        wait_liveman_sees(&cluster1, stream).await;

        let (v1, _) = whep_viewer(&cluster1, stream, "viewer1").await;
        wait_until(
            "viewer1 direct on the edge",
            Duration::from_secs(10),
            || async {
                let streams = streams_of(edge0).await;
                match find_stream(&streams, stream) {
                    Some(s) => active_subs(s).len() == 1,
                    None => false,
                }
            },
        )
        .await;
        let (v2, _) = whep_viewer(&cluster1, stream, "viewer2").await;
        let hop_on_edge = |streams: &[api::response::Stream]| {
            find_stream(streams, stream)
                .map(|s| {
                    s.subscribe
                        .sessions
                        .iter()
                        .filter(|x| x.state != api::response::RTCPeerConnectionState::Closed)
                        .any(|x| x.cascade.is_some())
                })
                .unwrap_or(false)
        };
        wait_until(
            "push hop established on the edge (self-marked)",
            Duration::from_secs(20),
            || async { hop_on_edge(&streams_of(edge0).await) },
        )
        .await;

        // --- kill liveman; the hop and the viewers are unaffected ---
        drop(cluster1);
        // --- a fresh liveman on a new port adopts the live hop ---
        let (liveman, shutdown) = boot_liveman(CascadeMode::Push, edge0, edge1, cloud).await;
        let _cluster2 = Cluster {
            liveman,
            edge0,
            cloud,
            _liveman_shutdown: shutdown,
        };
        // Several reaper ticks pass (check_tick_time = 1s); with adoption
        // the hop survives all of them — without it, the untracked-hop
        // reaper deletes it at the first tick.
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert!(
            hop_on_edge(&streams_of(edge0).await),
            "the hop must survive the liveman restart (adopted)"
        );
        let streams = streams_of(cloud).await;
        assert_eq!(
            active_subs(find_stream(&streams, stream).unwrap()).len(),
            1,
            "viewer2 must still be attached to the cloud"
        );

        // --- the last viewer leaves: the adopted intent is reaped ---
        let _ = v2.close().await;
        wait_until(
            "the adopted hop is reaped once viewer-less",
            Duration::from_secs(20),
            || async { !hop_on_edge(&streams_of(edge0).await) },
        )
        .await;

        let _ = publisher.close().await;
        let _ = v1.close().await;
    }

    /// A viewer that overflows the edge and immediately leaves must not
    /// cause create/destroy thrash: the supervisor may build the hop once,
    /// but the idle reaper tears it down and the edge settles back to a
    /// single outbound copy — and stays there.
    #[tokio::test]
    async fn viewer_churn_does_not_thrash_the_cascade() {
        init_test_environment();
        let cluster = boot_cluster(CascadeMode::Pull).await;
        let stream = "cam0";
        let publisher = whip_publish(&cluster, "edge0", stream).await;
        wait_liveman_sees(&cluster, stream).await;

        let (v1, _) = whep_viewer(&cluster, stream, "viewer1").await;
        wait_until(
            "viewer1 direct on the edge",
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

        // Overflow, then leave before the hop establishes.
        let (v2, _) = whep_viewer(&cluster, stream, "churn-viewer").await;
        let _ = v2.close().await;

        // Within a few idle/reaper cycles the cascade (if it was ever built)
        // is torn down and the edge settles back to the one direct viewer.
        wait_until(
            "cascade settled after churn",
            Duration::from_secs(20),
            || async {
                let streams = streams_of(cluster.edge0).await;
                match find_stream(&streams, stream) {
                    Some(s) => active_subs(s).len() == 1,
                    None => false,
                }
            },
        )
        .await;
        // No late re-establishment: still exactly one outbound copy seconds
        // later.
        tokio::time::sleep(Duration::from_secs(5)).await;
        let streams = streams_of(cluster.edge0).await;
        assert_eq!(
            active_subs(find_stream(&streams, stream).unwrap()).len(),
            1,
            "the edge must keep exactly one outbound copy (no re-cascade)"
        );
        // The leftover cloud stream is reaped too — liveman's idle teardown
        // kills the hop, and the node's own auto_delete_whep / orphan reaper
        // removes the empty stream after its grace.
        wait_until(
            "the cloud holds no leftover shell after churn",
            Duration::from_secs(20),
            || async { find_stream(&streams_of(cluster.cloud).await, stream).is_none() },
        )
        .await;

        let _ = publisher.close().await;
        let _ = v1.close().await;
    }
}
