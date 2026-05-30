use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
    sync::{LazyLock, Mutex},
};

use tokio::net::TcpListener;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod common;
use common::shutdown_signal;

struct Detect {
    // channels
    audio: Option<u8>,
    // (width, height)
    video: Option<(u16, u16)>,
}

const CONNECTION_WAIT_ATTEMPTS: usize = 300;

static ALLOCATED_UDP_PORTS: LazyLock<Mutex<HashSet<u16>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Allocate UDP ports for RTP tests and reserve the chosen port numbers in this
/// test process so concurrent `cargo test --test rtp` cases cannot reuse them.
fn alloc_udp_ports(ip: IpAddr, count: u16) -> u16 {
    let mut allocated = ALLOCATED_UDP_PORTS.lock().unwrap();

    for _ in 0..1000 {
        let socket = UdpSocket::bind(SocketAddr::new(ip, 0)).unwrap();
        let base_port = socket.local_addr().unwrap().port();
        drop(socket);
        if base_port > u16::MAX - count {
            continue;
        }

        let ports = base_port..base_port + count;
        if ports.clone().any(|port| allocated.contains(&port)) {
            continue;
        }
        if ports
            .clone()
            .all(|port| UdpSocket::bind(SocketAddr::new(ip, port)).is_ok())
        {
            allocated.extend(ports);
            return base_port;
        }
    }

    panic!("failed to allocate {count} available UDP ports for {ip}");
}

#[tokio::test]
async fn test_livetwo_rtp_vp8() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

    helper_livetwo_rtp(
        ip,
        &prefix,
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

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

    helper_livetwo_rtp(
        ip,
        &prefix,
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

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx-vp9 -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 5 -row-mt 1 -tile-columns 2 -frame-parallel 1 -b:v 1800k -maxrate 2200k -bufsize 4400k";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -strict experimental {vcodec}"
    );

    helper_livetwo_rtp(
        ip,
        &prefix,
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

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libx264 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 23 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

    helper_livetwo_rtp(
        ip,
        &prefix,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtp_h265() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libx265 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 25 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

    helper_livetwo_rtp(
        ip,
        &prefix,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtp_vp9_4k() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

    let width = 3840;
    let height = 2160;
    let vcodec = "-vcodec libvpx-vp9 -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 5 -row-mt 1 -tile-columns 2 -frame-parallel 1 -b:v 10m -maxrate 15m -bufsize 30m";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -strict experimental {vcodec}"
    );

    helper_livetwo_rtp(
        ip,
        &prefix,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtp_opus() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

    let acodec = "-acodec libopus -ar 48000 -ac 2 -b:a 48k -application voip -frame_duration 10 -vbr constrained";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {acodec}");

    helper_livetwo_rtp(
        ip,
        &prefix,
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

    let acodec = "-acodec g722";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {acodec}");

    helper_livetwo_rtp(
        ip,
        &prefix,
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

    let width = 1280;
    let height = 720;

    let acodec = "-acodec libopus -ar 48000 -ac 2 -b:a 48k -application voip -frame_duration 10 -vbr constrained";
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {acodec} {vcodec} -an"
    );

    helper_livetwo_rtp(
        ip,
        &prefix,
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

    let width = 1280;
    let height = 720;

    let acodec = "-acodec g722";
    let vcodec = "-vcodec libx264 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 23 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {acodec} {vcodec} -an",
    );

    helper_livetwo_rtp(
        ip,
        &prefix,
        Detect {
            audio: Some(1),
            video: Some((width, height)),
        },
    )
    .await;
}

async fn helper_livetwo_rtp(ip: IpAddr, prefix: &str, detect: Detect) {
    let whip_port = alloc_udp_ports(ip, 1);
    let whep_port = if detect.audio.is_some() && detect.video.is_some() {
        alloc_udp_ports(ip, 3)
    } else {
        alloc_udp_ports(ip, 1)
    };

    let cfg = liveion::config::Config::default();

    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
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

    let _whip_sdp = tempfile::NamedTempFile::new().unwrap();
    let tmp_path = _whip_sdp.path().to_str().unwrap().to_string();

    let ct = CancellationToken::new();
    let whip_command = format!(
        "{prefix} -f rtp 'rtp://{}' -sdp_file {tmp_path}",
        SocketAddr::new(ip, whip_port)
    );
    let handle_whip = tokio::spawn(livetwo::whip::into(
        ct.clone(),
        tmp_path.clone(),
        format!("http://{addr}{}", api::path::whip("-")),
        None,
        Some(whip_command),
    ));

    let mut last_state = None;
    let mut result = None;
    for _ in 0..CONNECTION_WAIT_ATTEMPTS {
        if handle_whip.is_finished() {
            panic!(
                "whip task exited before publish connected: {:?}",
                handle_whip.await
            );
        }
        let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();

        if let Some(r) = body.into_iter().find(|i| i.id == "-")
            && !r.publish.sessions.is_empty()
        {
            let s = r.publish.sessions[0].clone();
            last_state = Some(s.state);
            if s.state == api::response::RTCPeerConnectionState::Connected {
                result = Some(s);
                break;
            }
        };

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(
        result.is_some(),
        "Publish session did not reach Connected state within {}ms: whip_port={whip_port}, whep_port={whep_port}, liveion={addr}, last_state={last_state:?}",
        CONNECTION_WAIT_ATTEMPTS * 100,
    );

    // TODO: publish.state == connected is not ready
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    let _whep_sdp = tempfile::NamedTempFile::new().unwrap();
    let tmp_path = _whep_sdp.path().to_str().unwrap().to_string();

    // Wrap IPv6 addresses in brackets for a valid URI host segment.
    let ip_str = match ip {
        std::net::IpAddr::V6(_) => format!("[{ip}]"),
        _ => ip.to_string(),
    };

    let target_url = if detect.audio.is_some() && detect.video.is_some() {
        format!(
            "rtp://{}?video={}&audio={}",
            ip_str,
            whep_port,
            whep_port + 2
        )
    } else if detect.video.is_some() {
        format!("rtp://{ip_str}?video={whep_port}")
    } else if detect.audio.is_some() {
        format!("rtp://{ip_str}?audio={whep_port}")
    } else {
        format!("rtp://{ip_str}")
    };

    let handle_whep = tokio::spawn(livetwo::whep::from(
        ct.clone(),
        target_url,
        format!("http://{addr}{}", api::path::whep("-")),
        Some(tmp_path.clone()),
        None,
        None,
        None,
    ));

    let mut last_state = None;
    let mut result = None;
    for _ in 0..CONNECTION_WAIT_ATTEMPTS {
        if handle_whep.is_finished() {
            panic!(
                "whep task exited before subscribe connected: {:?}",
                handle_whep.await
            );
        }
        let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();

        if let Some(r) = body.into_iter().find(|i| i.id == "-")
            && !r.subscribe.sessions.is_empty()
        {
            let s = r.subscribe.sessions[0].clone();
            last_state = Some(s.state);
            if s.state == api::response::RTCPeerConnectionState::Connected {
                result = Some(s);
                break;
            }
        };

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(
        result.is_some(),
        "Subscribe session did not reach Connected state within {}ms: whip_port={whip_port}, whep_port={whep_port}, liveion={addr}, last_state={last_state:?}",
        CONNECTION_WAIT_ATTEMPTS * 100,
    );

    wait_for_sdp_file(&tmp_path).await;

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
        "ffprobe failed: whip_port={whip_port}, whep_port={whep_port}\nstdout: {}\nstderr: {}",
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

    ct.cancel();

    let result_whip = handle_whip.await.unwrap();
    let result_whep = handle_whep.await.unwrap();

    assert!(result_whip.is_ok());
    assert!(result_whep.is_ok());
}

async fn wait_for_sdp_file(path: &str) {
    for _ in 0..CONNECTION_WAIT_ATTEMPTS {
        if let Ok(contents) = std::fs::read_to_string(path)
            && contents.contains("m=")
        {
            return;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    panic!(
        "SDP file was not populated within {}ms: {path}",
        CONNECTION_WAIT_ATTEMPTS * 100
    );
}
