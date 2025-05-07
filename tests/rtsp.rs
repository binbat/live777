use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use tokio::net::TcpListener;
use tokio::process::Command;

mod common;
use common::shutdown_signal;

struct Detect {
    // channels
    audio: Option<u8>,
    // (width, height)
    video: Option<(u16, u16)>,
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 8550;
    let whep_port: u16 = 8555;

    let width = 640;
    let height = 480;
    let prefix =
        format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -vcodec libvpx");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        whip_port,
        whep_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_ipv6() {
    let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 8560;
    let whep_port: u16 = 8565;

    let width = 640;
    let height = 480;
    let prefix =
        format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -vcodec libvpx");
    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        whip_port,
        whep_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp9() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 8570;
    let whep_port: u16 = 8575;

    let width = 640;
    let height = 480;
    let codec = "-strict experimental -vcodec libvpx-vp9 -pix_fmt yuv420p";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {codec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        whip_port,
        whep_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_opus() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 8660;
    let whep_port: u16 = 8665;

    let codec = "-acodec libopus";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {codec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        whip_port,
        whep_port,
        Detect {
            audio: Some(2),
            video: None,
        },
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_g722() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 8670;
    let whep_port: u16 = 8675;

    let codec = "-acodec g722";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {codec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        whip_port,
        whep_port,
        Detect {
            audio: Some(1),
            video: None,
        },
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_opus() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 8710;
    let whep_port: u16 = 8715;

    let width = 640;
    let height = 480;
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size={width}x{height}:rate=30 -acodec libopus -vcodec libvpx");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        whip_port,
        whep_port,
        Detect {
            audio: Some(2),
            video: Some((width, height)),
        },
    )
    .await;
}

async fn helper_livetwo_rtsp(
    ip: IpAddr,
    port: u16,
    prefix: &str,
    whip_port: u16,
    whep_port: u16,
    detect: Detect,
) {
    let cfg = liveion::config::Config::default();

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

    tokio::spawn(livetwo::whip::into(
        format!(
            "{}://{}",
            livetwo::SCHEME_RTSP_SERVER,
            SocketAddr::new(ip, whip_port)
        ),
        format!("http://{addr}{}", api::path::whip("-")),
        None,
        Some(format!(
            "{prefix} -f rtsp 'rtsp://{}'",
            SocketAddr::new(ip, whip_port)
        )),
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

    tokio::spawn(livetwo::whep::from(
        format!(
            "{}://{}",
            livetwo::SCHEME_RTSP_SERVER,
            SocketAddr::new(ip, whep_port)
        ),
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
            "-i",
            &format!(
                "{}://{}",
                livetwo::SCHEME_RTSP_CLIENT,
                SocketAddr::new(ip, whep_port)
            ),
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
            codec_type: String,
            height: Option<u16>,
            width: Option<u16>,
            channels: Option<u8>,
        }

        #[derive(serde::Deserialize)]
        struct Ffprobe {
            streams: Vec<FfprobeStream>,
        }

        let r: Ffprobe = serde_json::from_slice(output.stdout.as_slice()).unwrap();

        for stream in r.streams.iter() {
            match stream.codec_type.as_str() {
                "video" => {
                    if let Some((width, height)) = detect.video {
                        assert_eq!(stream.width.unwrap(), width);
                        assert_eq!(stream.height.unwrap(), height);
                    } else {
                        panic!("Shouldn't exsit video");
                    }
                }
                "audio" => {
                    if let Some(channels) = detect.audio {
                        assert_eq!(stream.channels.unwrap(), channels);
                    } else {
                        panic!("Shouldn't exsit audio");
                    }
                }
                _ => panic!("Unknown codec_type: {}", stream.codec_type),
            }
        }
    }
}
