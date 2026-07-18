use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    sync::Once,
};

use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

mod common;
use common::shutdown_signal;

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

fn liveion_ice_candidate_hint(text: &str) -> &'static str {
    if text.contains("a=candidate:") && (text.contains(" 0.0.0.0 ") || text.contains(" :: ")) {
        " Liveion stream test ICE candidate override did not apply: SDP candidate contains an unspecified address; expected LIVE777_WEBRTC_ICE_UDP_ADDRS=127.0.0.1:0 before PeerConnection creation."
    } else {
        ""
    }
}

fn pick_udp_port(ip: IpAddr) -> u16 {
    let socket = UdpSocket::bind(SocketAddr::new(ip, 0)).expect("Failed to reserve UDP port");
    socket
        .local_addr()
        .expect("Failed to read temporary UDP port")
        .port()
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
    // `stun=` (empty) disables ICE servers so the test stays on loopback.
    let ct = CancellationToken::new();
    let handle_whip = tokio::spawn(livetwo::whip::into(
        ct.clone(),
        "synth://vp8?width=320&height=240&fps=15&duration=30&stun=".to_string(),
        format!("http://{addr}{}", api::path::whip("-")),
        None,
        None,
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

#[tokio::test]
async fn test_liveion_stream_connect() {
    init_liveion_test_environment();

    let cfg = liveion::config::Config::default();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;
    let rtp_port = pick_udp_port(ip);

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

    let tmp_path = tempfile::tempdir()
        .unwrap()
        .path()
        .to_str()
        .unwrap()
        .to_string();

    use std::io::Write;

    let mut file = std::fs::File::create(tmp_path.clone()).unwrap();
    let sdp = format!(
        r#"
v=0
o=- 0 0 IN IP4 127.0.0.1
s=No Name
c=IN IP4 127.0.0.1
t=0 0
a=tool:libavformat 61.1.100
m=video {rtp_port} RTP/AVP 96
b=AS:256
a=rtpmap:96 VP8/90000
    "#
    );

    file.write_all(sdp.as_bytes()).unwrap();

    let ct = CancellationToken::new();
    let handle_whip = tokio::spawn(livetwo::whip::into(
        ct.clone(),
        tmp_path.clone(),
        format!("http://{addr}{}", api::path::whip("-")),
        None,
        None,
    ));

    let mut result = None;
    let mut last_publish_state = None;
    let mut last_codecs = Vec::new();
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
            last_codecs = r.codecs.clone();
            if s.state == api::response::RTCPeerConnectionState::Connected {
                result = Some(s);
                break;
            }
        };

        if handle_whip.is_finished() {
            let result_whip = handle_whip.await.unwrap();
            let result_debug = format!("{result_whip:?}");
            let ice_hint = liveion_ice_candidate_hint(&result_debug);
            panic!(
                "WHIP task exited before publish connected: result={result_debug}, liveion={addr}, stream=-, rtp_port={rtp_port}, last_state={last_publish_state:?}, last_codecs={last_codecs:?}.{ice_hint}"
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(
        result.is_some(),
        "Publish session did not reach Connected within {}ms: liveion={addr}, stream=-, rtp_port={rtp_port}, last_state={last_publish_state:?}, last_codecs={last_codecs:?}",
        CONNECTION_WAIT_ATTEMPTS * 100,
    );

    let tmp_path = tempfile::tempdir()
        .unwrap()
        .path()
        .to_str()
        .unwrap()
        .to_string();

    let handle_whep = tokio::spawn(livetwo::whep::from(
        ct.clone(),
        format!("rtp://{ip}"),
        format!("http://{addr}{}", api::path::whep("-")),
        Some(tmp_path.clone()),
        None,
        None,
        None,
    ));

    let mut result = None;
    let mut last_subscribe_state = None;
    for _ in 0..CONNECTION_WAIT_ATTEMPTS {
        let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();

        if let Some(r) = body.into_iter().find(|i| i.id == "-")
            && !r.subscribe.sessions.is_empty()
        {
            let s = r.subscribe.sessions[0].clone();
            last_subscribe_state = Some(s.state);
            if s.state == api::response::RTCPeerConnectionState::Connected {
                result = Some(s);
                break;
            }
        };

        if handle_whep.is_finished() {
            let result_whep = handle_whep.await.unwrap();
            let result_debug = format!("{result_whep:?}");
            let ice_hint = liveion_ice_candidate_hint(&result_debug);
            panic!(
                "WHEP task exited before subscribe connected: result={result_debug}, liveion={addr}, stream=-, rtp_port={rtp_port}, publish_state={last_publish_state:?}, subscribe_state={last_subscribe_state:?}, codecs={last_codecs:?}.{ice_hint}"
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(
        result.is_some(),
        "Subscribe session did not reach Connected within {}ms: liveion={addr}, stream=-, rtp_port={rtp_port}, publish_state={last_publish_state:?}, subscribe_state={last_subscribe_state:?}, codecs={last_codecs:?}",
        CONNECTION_WAIT_ATTEMPTS * 100,
    );

    ct.cancel();

    let result_whip = handle_whip.await.unwrap();
    let result_whep = handle_whep.await.unwrap();

    assert!(result_whip.is_ok());
    assert!(result_whep.is_ok());
}
