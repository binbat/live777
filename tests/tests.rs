use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use tokio::net::TcpListener;
use tokio::process::Command;

async fn shutdown_signal() {
    let _str = signal::wait_for_stop_signal().await;
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

#[tokio::test]
async fn test_liveion_stream_connect() {
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

    let tmp_path = tempfile::tempdir()
        .unwrap()
        .path()
        .to_str()
        .unwrap()
        .to_string();

    use std::io::Write;

    let mut file = std::fs::File::create(tmp_path.clone()).unwrap();
    file.write_all(
        r#"
v=0
o=- 0 0 IN IP4 127.0.0.1
s=No Name
c=IN IP4 127.0.0.1
t=0 0
a=tool:libavformat 61.1.100
m=video 8765 RTP/AVP 96
b=AS:256
a=rtpmap:96 VP8/90000
    "#
        .as_bytes(),
    )
    .unwrap();

    tokio::spawn(livetwo::whip::into(
        tmp_path.clone(),
        None,
        format!("http://{addr}{}", api::path::whip("-")),
        None,
        None,
    ));

    let mut result = None;
    for _ in 0..100 {
        let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();

        if let Some(r) = body.into_iter().find(|i| i.id == "-") {
            if !r.publish.sessions.is_empty() {
                let s = r.publish.sessions[0].clone();
                if s.state == api::response::RTCPeerConnectionState::Connected {
                    result = Some(s);
                    break;
                }
            }
        };

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(result.is_some());

    let tmp_path = tempfile::tempdir()
        .unwrap()
        .path()
        .to_str()
        .unwrap()
        .to_string();
    tokio::spawn(livetwo::whep::from(
        tmp_path.clone(),
        None,
        format!("http://{addr}{}", api::path::whep("-")),
        None,
        None,
    ));

    let mut result = None;
    for _ in 0..100 {
        let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();

        if let Some(r) = body.into_iter().find(|i| i.id == "-") {
            if !r.subscribe.sessions.is_empty() {
                let s = r.subscribe.sessions[0].clone();
                if s.state == api::response::RTCPeerConnectionState::Connected {
                    result = Some(s);
                    break;
                }
            }
        };

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(result.is_some());
}

#[tokio::test]
async fn test_liveion_stream_ffmpeg() {
    let cfg = liveion::config::Config::default();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let width = 640;
    let height = 480;

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
    tokio::spawn(livetwo::whip::into(
        tmp_path.clone(),
        None,
        format!("http://{addr}{}", api::path::whip("-")),
        None,
        Some(format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -vcodec libvpx -f rtp 'rtp://127.0.0.1:5002' -sdp_file {tmp_path}")),
    ));

    let mut result = None;
    for _ in 0..100 {
        let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();

        if let Some(r) = body.into_iter().find(|i| i.id == "-") {
            if !r.publish.sessions.is_empty() {
                let s = r.publish.sessions[0].clone();
                if s.state == api::response::RTCPeerConnectionState::Connected {
                    result = Some(s);
                    break;
                }
            }
        };

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(result.is_some());

    let tmp_path = tempfile::tempdir()
        .unwrap()
        .path()
        .to_str()
        .unwrap()
        .to_string();
    tokio::spawn(livetwo::whep::from(
        tmp_path.clone(),
        None,
        format!("http://{addr}{}", api::path::whep("-")),
        None,
        None,
    ));

    let mut result = None;
    for _ in 0..100 {
        let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();

        if let Some(r) = body.into_iter().find(|i| i.id == "-") {
            if !r.subscribe.sessions.is_empty() {
                let s = r.subscribe.sessions[0].clone();
                if s.state == api::response::RTCPeerConnectionState::Connected {
                    result = Some(s);
                    break;
                }
            }
        };

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(result.is_some());

    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    let output = Command::new("ffprobe")
        .args(vec![
            "-v",
            "error",
            "-hide_banner",
            "-protocol_whitelist",
            "file,rtp,udp",
            "-i",
            tmp_path.as_str(),
            "-show_format",
            "-show_streams",
            "-of",
            "json",
        ])
        .output()
        .await
        .expect("Failed to execute command");

    assert!(output.status.success());

    if output.status.success() {
        #[derive(serde::Deserialize)]
        struct FfprobeStream {
            height: u16,
            width: u16,
        }

        #[derive(serde::Deserialize)]
        struct Ffprobe {
            streams: Vec<FfprobeStream>,
        }

        let r: Ffprobe = serde_json::from_slice(output.stdout.as_slice()).unwrap();

        assert_eq!(r.streams[0].width, width);
        assert_eq!(r.streams[0].height, height);
    }
}
