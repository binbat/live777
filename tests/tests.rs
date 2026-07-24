use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Once,
};

use tokio::net::TcpListener;
#[cfg(feature = "rsmpeg")]
use tokio_util::sync::CancellationToken;

mod common;
use common::shutdown_signal;

#[cfg(feature = "rsmpeg")]
const CONNECTION_WAIT_ATTEMPTS: usize = 300;
const WEBRTC_ICE_UDP_ADDRS: &str = "127.0.0.1:0";

static TRACING_INIT: Once = Once::new();

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

/// Parse one counter out of a Prometheus text exposition.
#[cfg(feature = "rsmpeg")]
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

    // Registration is process-global and panics on a second call; nextest
    // isolates test processes, but plain `cargo test` shares one.
    static METRICS_REGISTER: Once = Once::new();
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

    // Per-session counters mirror the stream direction: inbound for the
    // publisher, outbound for the subscriber.
    let publish_session = &stream.publish.sessions[0];
    assert!(publish_session.stats.bytes > 0);
    assert!(publish_session.stats.packets > 0);
    assert!(publish_session.stats.bitrate > 0);

    let subscribe_session = &stream.subscribe.sessions[0];
    assert!(subscribe_session.stats.bytes > 0);
    assert!(subscribe_session.stats.packets > 0);
    assert!(subscribe_session.stats.bitrate > 0);

    // Server-wide totals via the Prometheus endpoint.
    let metrics = reqwest::get(format!("http://{addr}{}", api::path::METRICS))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        metric_value(&metrics, "live777_bytes_in_total").unwrap_or(0) > 0,
        "live777_bytes_in_total missing or zero in /metrics:\n{metrics}"
    );
    assert!(
        metric_value(&metrics, "live777_bytes_out_total").unwrap_or(0) > 0,
        "live777_bytes_out_total missing or zero in /metrics:\n{metrics}"
    );

    let result_whip = handle_whip.await.unwrap();
    assert!(result_whip.is_ok());
    let result_whep = handle_whep.await.unwrap();
    assert!(result_whep.is_ok());
}
