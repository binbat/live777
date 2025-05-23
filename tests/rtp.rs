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
async fn test_livetwo_rtp_vp8() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 5000;
    let whep_port: u16 = 5005;

    let width = 640;
    let height = 480;
    let prefix =
        format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -vcodec libvpx");

    helper_livetwo_rtp(
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
async fn test_livetwo_rtp_vp8_ipv6() {
    let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 5010;
    let whep_port: u16 = 5015;

    let width = 640;
    let height = 480;
    let prefix =
        format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -vcodec libvpx");

    helper_livetwo_rtp(
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
async fn test_livetwo_rtp_vp9() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 5020;
    let whep_port: u16 = 5025;

    let width = 640;
    let height = 480;
    let codec = "-strict experimental -vcodec libvpx-vp9 -pix_fmt yuv420p";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {codec}");

    helper_livetwo_rtp(
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
async fn test_livetwo_rtp_h264() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 5030;
    let whep_port: u16 = 5035;

    let width = 640;
    let height = 480;
    let codec = "-profile:v baseline -level 3.0 -pix_fmt yuv420p -g 30 -keyint_min 30 -b:v 1000k -minrate 1000k -maxrate 1000k -bufsize 1000k -preset ultrafast -tune zerolatency";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -vcodec libx264 {codec}"
    );

    helper_livetwo_rtp(
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

// TODO: In GitHub Actions, always have some problem
//
//#[tokio::test]
//async fn test_livetwo_rtp_vp9_4k() {
//    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
//    let port = 0;
//
//    let whip_port: u16 = 5040;
//    let whep_port: u16 = 5045;
//
//    let width = 3840;
//    let height = 2160;
//    let codec = "-strict experimental -vcodec libvpx-vp9 -pix_fmt yuv420p";
//    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {codec}");
//
//    helper_livetwo_rtp(
//        ip,
//        port,
//        &prefix,
//        whip_port,
//        whep_port,
//        Detect {
//            audio: None,
//            video: Some((width, height)),
//        },
//    )
//    .await;
//}

#[tokio::test]
async fn test_livetwo_rtp_opus() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 5050;
    let whep_port: u16 = 5055;

    let codec = "-acodec libopus";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {codec}");

    helper_livetwo_rtp(
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
async fn test_livetwo_rtp_g722() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 5060;
    let whep_port: u16 = 5065;

    let codec = "-acodec g722";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {codec}");

    helper_livetwo_rtp(
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
async fn test_livetwo_rtp_vp8_opus() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 5070;
    let whep_port: u16 = 5075;

    let width = 640;
    let height = 480;

    let prefix = format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size={width}x{height}:rate=30 -acodec libopus -vcodec libvpx -an"
    );

    helper_livetwo_rtp(
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

#[tokio::test]
async fn test_livetwo_rtp_h264_g722() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 5080;
    let whep_port: u16 = 5085;

    let width = 640;
    let height = 480;

    let vcodec = "-profile:v baseline -level 3.0 -pix_fmt yuv420p -g 30 -keyint_min 30 -b:v 1000k -minrate 1000k -maxrate 1000k -bufsize 1000k -preset ultrafast -tune zerolatency";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size={width}x{height}:rate=30 -acodec g722 -vcodec libx264 {vcodec} -an",
    );

    helper_livetwo_rtp(
        ip,
        port,
        &prefix,
        whip_port,
        whep_port,
        Detect {
            audio: Some(1),
            video: Some((width, height)),
        },
    )
    .await;
}

async fn helper_livetwo_rtp(
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

    let tmp_path = tempfile::tempdir()
        .unwrap()
        .path()
        .to_str()
        .unwrap()
        .to_string();
    tokio::spawn(livetwo::whip::into(
        tmp_path.clone(),
        format!("http://{addr}{}", api::path::whip("-")),
        None,
        Some(format!(
            "{prefix} -f rtp 'rtp://{}' -sdp_file {tmp_path}",
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

    let tmp_path = tempfile::tempdir()
        .unwrap()
        .path()
        .to_str()
        .unwrap()
        .to_string();

    let target_url = if detect.audio.is_some() && detect.video.is_some() {
        format!("rtp://{}?video={}&audio={}", ip, whep_port, whep_port + 2)
    } else if detect.video.is_some() {
        format!("rtp://{}?video={}", ip, whep_port)
    } else if detect.audio.is_some() {
        format!("rtp://{}?audio={}", ip, whep_port)
    } else {
        format!("rtp://{}", ip)
    };

    tokio::spawn(livetwo::whep::from(
        target_url,
        format!("http://{addr}{}", api::path::whep("-")),
        Some(tmp_path.clone()),
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

    // TODO: Need wait SDP exists
    let mut tmp_path_ok = false;
    for _ in 0..100 {
        if std::path::Path::new(&tmp_path).exists() {
            tmp_path_ok = true;
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    assert!(tmp_path_ok, "{tmp_path} is not exists");

    // TODO: Need wait SDP
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

    let output = Command::new("ffprobe")
        .args(vec![
            "-v",
            "error",
            "-hide_banner",
            "-protocol_whitelist",
            "file,rtp,udp",
            "-i",
            tmp_path.as_str(),
            "-show_streams",
            "-of",
            "json",
        ])
        .output()
        .await
        .expect("Failed to execute command");

    assert!(
        output.status.success(),
        "stdout: {}\r\nstderr: {}",
        std::str::from_utf8(output.stdout.as_slice()).unwrap(),
        std::str::from_utf8(output.stderr.as_slice()).unwrap()
    );

    if output.status.success() {
        #[derive(serde::Deserialize)]
        struct FfprobeStream {
            codec_type: String,
            width: Option<u16>,
            height: Option<u16>,
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
                        panic!("Shouldn't exist video");
                    }
                }
                "audio" => {
                    if let Some(channels) = detect.audio {
                        assert_eq!(stream.channels.unwrap(), channels);
                    } else {
                        panic!("Shouldn't exist audio");
                    }
                }
                _ => panic!("Unknown codec_type: {}", stream.codec_type),
            }
        }
    }
}
