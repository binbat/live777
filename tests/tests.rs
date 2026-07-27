use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::{Arc, Once},
};

use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use webrtc::media_stream::MediaStreamTrack;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
use webrtc::peer_connection::{PeerConnection, RTCSessionDescription};

mod common;
use common::shutdown_signal;

const CONNECTION_WAIT_ATTEMPTS: usize = 300;
const WEBRTC_ICE_UDP_ADDRS: &str = "127.0.0.1:0";

static TRACING_INIT: Once = Once::new();

/// Metrics registration is process-global and panics on a second call;
/// nextest isolates test processes, but plain `cargo test` shares one, so
/// every test that needs the registry must go through this single `Once`.
static METRICS_REGISTER: Once = Once::new();

fn init_liveion_test_environment() {
    TRACING_INIT.call_once(|| {
        // These tests run both WebRTC peers locally. Pin ICE candidates to
        // loopback so CI runners cannot choose an unroutable host interface.
        unsafe {
            std::env::set_var("LIVE777_WEBRTC_ICE_UDP_ADDRS", WEBRTC_ICE_UDP_ADDRS);
        }

        let filter = std::env::var("RUST_LOG")
            .unwrap_or_else(|_| "live777=info,liveion=info,livetwo=info,libwish=info".to_string());
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

#[test]
fn liveion_test_environment_pins_webrtc_ice_to_loopback() {
    init_liveion_test_environment();

    assert_eq!(
        std::env::var("LIVE777_WEBRTC_ICE_UDP_ADDRS").as_deref(),
        Ok(WEBRTC_ICE_UDP_ADDRS)
    );
    assert_eq!(
        livetwo::utils::webrtc::ice_udp_addrs(),
        vec![WEBRTC_ICE_UDP_ADDRS.parse::<SocketAddr>().unwrap()]
    );
}

#[tokio::test]
async fn test_liveion_simple() {
    let cfg = liveion::config::Config::default();
    let strategy = cfg.strategy.clone();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let listener = TcpListener::bind(SocketAddr::new(ip, port)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let res = reqwest::get(format!("http://{addr}{}", api::path::strategy()))
        .await
        .unwrap();

    assert_eq!(http::StatusCode::OK, res.status());

    let body = res.json::<api::strategy::Strategy>().await.unwrap();

    assert_eq!(strategy, body);
}

#[tokio::test]
async fn test_liveion_info() {
    let cfg = liveion::config::Config::default();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let listener = TcpListener::bind(SocketAddr::new(ip, port)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let res = reqwest::get(format!("http://{addr}{}", api::path::INFO))
        .await
        .unwrap();

    assert_eq!(http::StatusCode::OK, res.status());

    let body = res.json::<api::response::ServerInfo>().await.unwrap();

    assert!(!body.version.is_empty());
    assert!(body.version.contains(&body.git_hash));
    assert!(!body.git_hash.is_empty());
    assert!(!body.build_time.is_empty());
    assert!(body.features.iter().all(|f| !f.is_empty()));

    #[cfg(feature = "recorder")]
    assert!(body.features.contains(&"recorder".to_string()));
    #[cfg(not(feature = "recorder"))]
    assert!(!body.features.contains(&"recorder".to_string()));

    #[cfg(feature = "cascade")]
    assert!(body.features.contains(&"cascade".to_string()));
    #[cfg(not(feature = "cascade"))]
    assert!(!body.features.contains(&"cascade".to_string()));
}

#[tokio::test]
async fn test_liveion_ipv6() {
    let cfg = liveion::config::Config::default();
    let strategy = cfg.strategy.clone();
    let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let port = 0;

    let listener = TcpListener::bind(SocketAddr::new(ip, port)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let res = reqwest::get(format!("http://{addr}{}", api::path::strategy()))
        .await
        .unwrap();

    assert_eq!(http::StatusCode::OK, res.status());

    let body = res.json::<api::strategy::Strategy>().await.unwrap();

    assert_eq!(strategy, body);
}

#[tokio::test]
async fn test_liveion_stream_create() {
    let cfg = liveion::config::Config::default();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let listener = TcpListener::bind(SocketAddr::new(ip, port)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let res = reqwest::Client::new()
        .post(format!("http://{addr}{}", api::path::streams("-")))
        .send()
        .await
        .unwrap();

    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
        .await
        .unwrap();

    let body = res.json::<Vec<api::response::Stream>>().await.unwrap();

    assert_eq!(1, body.len());
}

#[cfg(feature = "rsmpeg")]
#[tokio::test]
async fn test_livetwo_whipinto_synth_input() {
    init_liveion_test_environment();

    let cfg = liveion::config::Config::default();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let res = reqwest::Client::new()
        .post(format!("http://{addr}{}", api::path::streams("-")))
        .send()
        .await
        .unwrap();

    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    // Publish an in-process synthetic stream through the unified `whip::into`
    // entry point, the same path `whipinto --input synth://...` uses.
    // The empty ICE server list disables ICE servers so the test stays on
    // loopback.
    let ct = CancellationToken::new();
    let handle_whip = tokio::spawn(livetwo::whip::into(
        ct.clone(),
        "synth://vp8?width=320&height=240&fps=15&duration=30".to_string(),
        format!("http://{addr}{}", api::path::whip("-")),
        None,
        None,
        Vec::new(),
    ));

    let mut result = None;
    let mut last_publish_state = None;
    for _ in 0..CONNECTION_WAIT_ATTEMPTS {
        let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();

        if let Some(r) = body.into_iter().find(|i| i.id == "-")
            && !r.publish.sessions.is_empty()
        {
            let s = r.publish.sessions[0].clone();
            last_publish_state = Some(s.state);
            if s.state == api::response::RTCPeerConnectionState::Connected {
                result = Some(s);
                break;
            }
        };

        if handle_whip.is_finished() {
            let result_whip = handle_whip.await.unwrap();
            panic!(
                "synth WHIP task exited before publish connected: result={result_whip:?}, last_state={last_publish_state:?}"
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(
        result.is_some(),
        "Synth publish session did not reach Connected within {}ms: last_state={last_publish_state:?}",
        CONNECTION_WAIT_ATTEMPTS * 100,
    );

    ct.cancel();

    let result_whip = handle_whip.await.unwrap();
    assert!(result_whip.is_ok());
}

/// Parse one counter out of a Prometheus text exposition. `name` is the
/// full series key, including any label set (e.g.
/// `live777_rtp_bytes_total{direction="in"}`).
fn metric_value(body: &str, name: &str) -> Option<u64> {
    body.lines()
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| match line.split_once(' ') {
            Some((key, value)) if key == name => value.parse().ok(),
            _ => None,
        })
}

/// Issue #252: stream statistics. A synthetic publisher plus a WHEP
/// subscriber must produce non-zero in/out counters, bitrates and
/// server-wide Prometheus totals after the stats tick samples the traffic.
#[cfg(feature = "rsmpeg")]
#[tokio::test]
async fn test_liveion_stream_stats() {
    init_liveion_test_environment();
    METRICS_REGISTER.call_once(liveion::metrics_register);

    let cfg = liveion::config::Config::default();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let res = reqwest::Client::new()
        .post(format!("http://{addr}{}", api::path::streams("-")))
        .send()
        .await
        .unwrap();

    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    let ct = CancellationToken::new();
    let handle_whip = tokio::spawn(livetwo::whip::into(
        ct.clone(),
        "synth://vp8?width=320&height=240&fps=15&duration=30".to_string(),
        format!("http://{addr}{}", api::path::whip("-")),
        None,
        None,
        Vec::new(),
    ));

    // WHEP subscriber; the RTP output goes nowhere in particular, only the
    // server-side session matters here. The empty ICE server list keeps the
    // test on loopback.
    let handle_whep = tokio::spawn(livetwo::whep::from(
        ct.clone(),
        format!("rtp://{ip}"),
        format!("http://{addr}{}", api::path::whep("-")),
        None,
        None,
        None,
        None,
        Vec::new(),
    ));

    // Wait until publisher and subscriber are both connected and the stats
    // tick (2 s interval) has sampled real traffic in both directions.
    let mut snapshot = None;
    for _ in 0..CONNECTION_WAIT_ATTEMPTS {
        let body = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap()
            .json::<Vec<api::response::Stream>>()
            .await
            .unwrap_or_default();

        if let Some(stream) = body.into_iter().find(|i| i.id == "-") {
            let publish_connected = stream
                .publish
                .sessions
                .first()
                .is_some_and(|s| s.state == api::response::RTCPeerConnectionState::Connected);
            let subscribe_connected = stream
                .subscribe
                .sessions
                .first()
                .is_some_and(|s| s.state == api::response::RTCPeerConnectionState::Connected);
            let stats_ready = stream.stats.publish.bytes > 0
                && stream.stats.publish.bitrate > 0
                && stream.stats.subscribe.bytes > 0
                && stream.stats.subscribe.bitrate > 0;
            if publish_connected && subscribe_connected && stats_ready {
                snapshot = Some(stream);
                break;
            }
        }

        if handle_whip.is_finished() || handle_whep.is_finished() {
            panic!(
                "WHIP/WHEP task exited before stats became ready: whip_ok={:?}, whep_ok={:?}",
                handle_whip.is_finished(),
                handle_whep.is_finished()
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    ct.cancel();

    let stream = snapshot.unwrap_or_else(|| {
        panic!(
            "stream stats did not become ready within {}ms",
            CONNECTION_WAIT_ATTEMPTS * 100
        )
    });
    assert_eq!(stream.stats_scope, api::response::StatsScope::Node);

    // Per-session counters mirror the stream direction: inbound for the
    // publisher, outbound for the subscriber.
    let publish_session_stats = stream.publish.sessions[0].stats.clone();
    assert!(publish_session_stats.bytes > 0);
    assert!(publish_session_stats.packets > 0);
    assert!(publish_session_stats.bitrate > 0);

    let subscribe_session_stats = stream.subscribe.sessions[0].stats.clone();
    assert!(subscribe_session_stats.bytes > 0);
    assert!(subscribe_session_stats.packets > 0);
    assert!(subscribe_session_stats.bitrate > 0);

    // Server-wide totals via the Prometheus endpoint.
    let metrics = reqwest::get(format!("http://{addr}{}", api::path::METRICS))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metric_value(&metrics, r#"live777_rtp_bytes_total{direction="in"}"#).unwrap_or(0) > 0,
        r#"live777_rtp_bytes_total{{direction="in"}} missing or zero in /metrics:
{metrics}"#
    );
    assert!(
        metric_value(&metrics, r#"live777_rtp_bytes_total{direction="out"}"#).unwrap_or(0) > 0,
        r#"live777_rtp_bytes_total{{direction="out"}} missing or zero in /metrics:
{metrics}"#
    );

    let result_whip = handle_whip.await.unwrap();
    assert!(result_whip.is_ok());
    let result_whep = handle_whep.await.unwrap();
    assert!(result_whep.is_ok());
}

/// Fetch the `"-"` stream snapshot from a test liveion server.
async fn fetch_stream(addr: &SocketAddr) -> Option<api::response::Stream> {
    reqwest::get(format!("http://{addr}{}", api::path::streams("")))
        .await
        .ok()?
        .json::<Vec<api::response::Stream>>()
        .await
        .ok()?
        .into_iter()
        .find(|i| i.id == "-")
}

/// Fetch the `/metrics` Prometheus exposition from a test liveion server.
async fn fetch_metrics(addr: &SocketAddr) -> String {
    reqwest::get(format!("http://{addr}{}", api::path::METRICS))
        .await
        .expect("GET /metrics")
        .text()
        .await
        .expect("metrics body")
}

/// Poll the stream snapshot until `pred` holds, returning that snapshot.
async fn poll_stream_until(
    addr: &SocketAddr,
    what: &str,
    mut pred: impl FnMut(&api::response::Stream) -> bool,
) -> api::response::Stream {
    for _ in 0..CONNECTION_WAIT_ATTEMPTS {
        if let Some(stream) = fetch_stream(addr).await
            && pred(&stream)
        {
            return stream;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    panic!(
        "timed out waiting for {what} within {}ms",
        CONNECTION_WAIT_ATTEMPTS * 100
    );
}

/// Poll the stream snapshot until the `"-"` stream is gone.
async fn poll_stream_gone(addr: &SocketAddr, what: &str) {
    for _ in 0..CONNECTION_WAIT_ATTEMPTS {
        if fetch_stream(addr).await.is_none() {
            return;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    panic!(
        "timed out waiting for {what} within {}ms",
        CONNECTION_WAIT_ATTEMPTS * 100
    );
}

/// A minimal in-process WHIP publisher whose teardown the test controls
/// precisely: [`RawPublisher::pause_pump`] stops the RTP flow while keeping
/// the peer (and thus the server-side session) alive, and
/// [`RawPublisher::stop`] closes the peer *without* DELETE-ing the WHIP
/// session (deleting the *publish* session takes the whole stream down by
/// design — see `RemovePeerOutcome::PublisherRemoved`).
/// `livetwo::whip::into` cannot be used here: its graceful shutdown always
/// DELETEs first, and aborting it only leaves the server-side peer to the
/// disconnected watchdog, whose teardown does not complete (the local
/// `close()` never surfaces a `Closed` state with this webrtc stack).
struct RawPublisher {
    peer: Arc<dyn PeerConnection>,
    pump: Option<tokio::task::JoinHandle<()>>,
}

impl RawPublisher {
    /// Publish a single VP8 video track of synthetic RTP packets (the
    /// payload is never decoded, only forwarded and counted) to the `"-"`
    /// stream, returning once the peer is connected and media is flowing.
    async fn start(addr: &SocketAddr) -> Self {
        let gather_complete = Arc::new(Notify::new());
        let publish = livetwo::whip::core::create_publish_peer(
            gather_complete.clone(),
            livetwo::whip::core::PublishPeerOptions {
                ice_servers: Vec::new(),
                extra_video_codecs: Vec::new(),
            },
        )
        .await
        .expect("create publish peer");
        let peer = publish.peer.clone();

        let ssrc = 0x5a5a_5a5a_u32;
        let track_id = "stats-churn-video".to_owned();
        let track = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
            "stats-churn".to_owned(),
            track_id.clone(),
            track_id,
            RtpCodecKind::Video,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(ssrc),
                    ..Default::default()
                },
                codec: RTCRtpCodec {
                    mime_type: "video/VP8".to_owned(),
                    clock_rate: 90_000,
                    channels: 0,
                    sdp_fmtp_line: String::new(),
                    rtcp_feedback: Vec::new(),
                },
                ..Default::default()
            }],
        )));
        peer.add_track(track.clone()).await.expect("add track");

        let offer = peer.create_offer(None).await.expect("create offer");
        peer.set_local_description(offer)
            .await
            .expect("set local description");
        tokio::time::timeout(
            tokio::time::Duration::from_secs(5),
            gather_complete.notified(),
        )
        .await
        .expect("ICE gathering timed out");
        let local = peer
            .local_description()
            .await
            .expect("local description after gathering");

        let res = reqwest::Client::new()
            .post(format!("http://{addr}{}", api::path::whip("-")))
            .header("content-type", "application/sdp")
            .body(local.sdp)
            .send()
            .await
            .expect("WHIP POST");
        assert_eq!(http::StatusCode::CREATED, res.status());
        let answer = res.text().await.expect("WHIP answer body");
        peer.set_remote_description(
            RTCSessionDescription::answer(answer).expect("parse WHIP answer"),
        )
        .await
        .expect("set remote description");

        livetwo::whip::core::wait_for_peer_connected(
            peer.clone(),
            publish.state_rx.clone(),
            publish.diagnostics.clone(),
        )
        .await
        .expect("publish peer did not connect");

        // The answer may have remapped the dynamic payload type; write
        // packets with the negotiated one.
        let mut payload_type = None;
        for sender in peer.get_senders().await {
            if let Ok(params) = sender.get_parameters().await
                && let Some(codec) = params.rtp_parameters.codecs.first()
            {
                payload_type = Some(codec.payload_type);
            }
        }
        let payload_type = payload_type.expect("negotiated payload type");

        let pump = tokio::spawn(async move {
            let mut seq: u16 = 0;
            let mut timestamp: u32 = 0;
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(20));
            loop {
                interval.tick().await;
                let packet = rtc::rtp::packet::Packet {
                    header: rtc::rtp::header::Header {
                        version: 2,
                        marker: true,
                        payload_type,
                        sequence_number: seq,
                        timestamp,
                        ssrc,
                        ..Default::default()
                    },
                    payload: vec![0u8; 1000].into(),
                };
                if track.write_rtp(packet).await.is_err() {
                    // The peer is closing; the test drives teardown.
                    break;
                }
                seq = seq.wrapping_add(1);
                timestamp = timestamp.wrapping_add(3000);
            }
        });

        RawPublisher {
            peer,
            pump: Some(pump),
        }
    }

    /// Stop the media pump; the peer (and the server-side session) stays
    /// connected, so the stream simply goes silent.
    async fn pause_pump(&mut self) {
        if let Some(pump) = self.pump.take() {
            pump.abort();
            let _ = pump.await;
        }
    }

    /// Stop the media pump and close the peer (no session DELETE).
    async fn stop(mut self) {
        self.pause_pump().await;
        self.peer.close().await.expect("close publish peer");
    }
}

/// Issue #252 follow-up: stream totals stay monotonic across subscriber
/// churn and republishes — each departing flow's un-sampled tail is folded
/// into the totals instead of resetting them — and a direction whose media
/// stopped decays to a zero bitrate on the next stats tick.
#[tokio::test]
async fn test_liveion_stream_stats_churn() {
    init_liveion_test_environment();
    METRICS_REGISTER.call_once(liveion::metrics_register);

    let cfg = liveion::config::Config::default();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let res = reqwest::Client::new()
        .post(format!("http://{addr}{}", api::path::streams("-")))
        .send()
        .await
        .unwrap();

    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    // The WHEP subscribers run as livetwo tasks, each with its own
    // cancellation token; cancelling one gracefully DELETEs its server-side
    // session (see `graceful_shutdown` in livetwo), which removes only the
    // subscriber — unlike a publish-session DELETE it leaves the stream
    // alive.
    let spawn_whep = |ct: CancellationToken| {
        tokio::spawn(livetwo::whep::from(
            ct,
            format!("rtp://{ip}"),
            format!("http://{addr}{}", api::path::whep("-")),
            None,
            None,
            None,
            None,
            Vec::new(),
        ))
    };

    // ── Phase 1: publisher plus first subscriber flowing ──
    let mut publisher1 = RawPublisher::start(&addr).await;
    let ct_whep1 = CancellationToken::new();
    let handle_whep1 = spawn_whep(ct_whep1.clone());

    let stream = poll_stream_until(&addr, "publish + subscribe stats flowing", |s| {
        s.publish
            .sessions
            .iter()
            .any(|x| x.state == api::response::RTCPeerConnectionState::Connected)
            && s.subscribe
                .sessions
                .iter()
                .any(|x| x.state == api::response::RTCPeerConnectionState::Connected)
            && s.stats.publish.bytes > 0
            && s.stats.subscribe.bytes > 0
    })
    .await;
    let publish_bytes_1 = stream.stats.publish.bytes;
    let subscribe_bytes_1 = stream.stats.subscribe.bytes;

    // ── Phase 2: the subscriber leaves ──
    // The closed session keeps its cumulative counters (with the rate
    // zeroed), the stream total keeps the folded tail (no reset), and the
    // subscribe rate decays to zero on the next tick.
    ct_whep1.cancel();
    assert!(handle_whep1.await.unwrap().is_ok());

    let stream = poll_stream_until(&addr, "first subscriber closed", |s| {
        // Require the closed session to be listed: between its removal from
        // the active group and the push to the closed list the snapshot has
        // neither, and an `all(Closed)` on an empty list would pass early.
        s.subscribe
            .sessions
            .iter()
            .any(|x| x.state == api::response::RTCPeerConnectionState::Closed)
            && s.stats.subscribe.bitrate == 0
    })
    .await;
    assert!(
        stream.stats.subscribe.bytes >= subscribe_bytes_1,
        "subscribe total reset across churn: was {subscribe_bytes_1}, now {}",
        stream.stats.subscribe.bytes
    );
    let closed = stream
        .subscribe
        .sessions
        .iter()
        .find(|x| x.state == api::response::RTCPeerConnectionState::Closed)
        .expect("departed subscriber must linger as a closed session");
    assert!(closed.stats.bytes > 0);
    assert_eq!(0, closed.stats.bitrate);
    let subscribe_bytes_folded = stream.stats.subscribe.bytes;

    // ── Phase 3: a second subscriber arrives — the total keeps growing ──
    let ct_whep2 = CancellationToken::new();
    let handle_whep2 = spawn_whep(ct_whep2.clone());

    let stream = poll_stream_until(&addr, "second subscriber flowing", |s| {
        s.subscribe
            .sessions
            .iter()
            .any(|x| x.state == api::response::RTCPeerConnectionState::Connected)
            && s.stats.subscribe.bitrate > 0
    })
    .await;
    assert!(
        stream.stats.subscribe.bytes > subscribe_bytes_folded,
        "subscribe total did not grow after resubscribe: was {subscribe_bytes_folded}, now {}",
        stream.stats.subscribe.bytes
    );

    // ── Phase 4: the publisher goes silent — the rate decays to zero while
    // the session and the totals stay ──
    publisher1.pause_pump().await;

    let stream = poll_stream_until(&addr, "publish rate decayed to zero", |s| {
        s.publish
            .sessions
            .iter()
            .any(|x| x.state == api::response::RTCPeerConnectionState::Connected)
            && s.stats.publish.bitrate == 0
    })
    .await;
    assert!(
        stream.stats.publish.bytes >= publish_bytes_1,
        "publish total decreased while silent: was {publish_bytes_1}, now {}",
        stream.stats.publish.bytes
    );

    // ── Phase 5: deleting the publish session folds its tail and retires
    // the stream (by design) — the server-wide counter must not lose a
    // byte. (The per-stream republish-monotonicity path — a publish peer
    // vanishing and `remove_publish` folding its tracks — is not covered:
    // with the current webrtc stack a local `close()` never surfaces a
    // `Closed` state to the event handler, so the disconnected watchdog's
    // teardown never runs and the session zombifies instead.)
    let session_id = stream
        .publish
        .sessions
        .iter()
        .find(|x| x.state == api::response::RTCPeerConnectionState::Connected)
        .expect("silent publisher must still be connected")
        .id
        .clone();
    let metric_in_before = metric_value(
        &fetch_metrics(&addr).await,
        r#"live777_rtp_bytes_total{direction="in"}"#,
    )
    .unwrap_or(0);

    let res = reqwest::Client::new()
        .delete(format!(
            "http://{addr}{}",
            api::path::session("-", &session_id)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    poll_stream_gone(&addr, "stream retired").await;
    publisher1.stop().await;

    let metric_in_folded = metric_value(
        &fetch_metrics(&addr).await,
        r#"live777_rtp_bytes_total{direction="in"}"#,
    )
    .unwrap_or(0);
    assert!(
        metric_in_folded >= metric_in_before,
        "server in-counter lost bytes across the publish-session delete: was {metric_in_before}, now {metric_in_folded}"
    );

    // ── Phase 6: republish on a fresh stream — the server-wide counter
    // keeps growing; the new stream's own totals start from zero ──
    let res = reqwest::Client::new()
        .post(format!("http://{addr}{}", api::path::streams("-")))
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    let publisher2 = RawPublisher::start(&addr).await;

    let stream = poll_stream_until(&addr, "republish flowing", |s| {
        s.publish
            .sessions
            .iter()
            .any(|x| x.state == api::response::RTCPeerConnectionState::Connected)
            && s.stats.publish.bitrate > 0
    })
    .await;
    assert!(stream.stats.publish.bytes > 0);

    let metric_in_republished = metric_value(
        &fetch_metrics(&addr).await,
        r#"live777_rtp_bytes_total{direction="in"}"#,
    )
    .unwrap_or(0);
    assert!(
        metric_in_republished > metric_in_folded,
        "server in-counter did not grow after republish: was {metric_in_folded}, now {metric_in_republished}"
    );
    // The fresh stream restarts its own totals from zero, so its publish
    // total is strictly below the server-wide counter (which still carries
    // the retired stream's folded bytes).
    assert!(
        stream.stats.publish.bytes < metric_in_republished,
        "fresh stream publish total {} should be below the server in-counter {metric_in_republished}",
        stream.stats.publish.bytes
    );

    // The "out" counter absorbed the folded tails of both departed
    // subscribers (whep1 via its DELETE, whep2 via the stream teardown).
    assert!(
        metric_value(
            &fetch_metrics(&addr).await,
            r#"live777_rtp_bytes_total{direction="out"}"#
        )
        .unwrap_or(0)
            >= subscribe_bytes_folded,
        "server out-counter lost bytes across subscriber churn"
    );

    ct_whep2.cancel();
    publisher2.stop().await;
    // The stream died under whep2 (phase 5), so its task may exit with an
    // error instead of a clean shutdown — either is fine here.
    let _ = handle_whep2.await;
}
