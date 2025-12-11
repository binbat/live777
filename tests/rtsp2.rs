use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use tokio::net::TcpListener;
use tokio::process::Command;

mod common;
use common::shutdown_signal;

// === RTSP Bootstrapping ===
//
// - ffmpeg
// - whipinto rtsp server
//
// # stream: A
//
// - whepfrom rtsp server
// - whipinto rtsp client
//
// # stream: B
//
// - whipinto rtsp server
// - whepfrom rtsp client
//
// # stream: C
//
// - whepfrom rtsp server
// - ffprobe

enum Transport {
    Udp,
    Tcp,
}

impl Transport {
    fn as_str(&self) -> &str {
        match self {
            Transport::Udp => "",
            Transport::Tcp => "?transport=tcp",
        }
    }
}

struct Ports {
    whip: u16,
    p_ab: u16,
    p_bc: u16,
    whep: u16,
}

struct Detect {
    // channels
    audio: Option<u8>,
    // (width, height)
    video: Option<(u16, u16)>,
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_h264_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7160;
    let whep_port: u16 = 7175;

    let width = 640;
    let height = 480;
    let prefix = format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -vcodec libx264 -profile:v baseline -level 3.1 -pix_fmt yuv420p -g 15 -keyint_min 15 -b:v 1000k -minrate 1000k -maxrate 1000k -bufsize 1000k -preset ultrafast -tune zerolatency -x264-params repeat_headers=1"
    );

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7165,
            p_bc: 7170,
            whep: whep_port,
        },
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_h264_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7360;
    let whep_port: u16 = 7375;

    let width = 640;
    let height = 480;
    let prefix = format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -vcodec libx264 -profile:v baseline -level 3.1 -pix_fmt yuv420p -g 15 -keyint_min 15 -b:v 1000k -minrate 1000k -maxrate 1000k -bufsize 1000k -preset ultrafast -tune zerolatency -x264-params repeat_headers=1 -rtsp_transport tcp"
    );

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7365,
            p_bc: 7370,
            whep: whep_port,
        },
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7020;
    let whep_port: u16 = 7035;

    let width = 640;
    let height = 480;
    let codec = "-vcodec libvpx -pix_fmt yuv420p -b:v 1000k -deadline realtime";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {codec}");

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7025,
            p_bc: 7030,
            whep: whep_port,
        },
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7220;
    let whep_port: u16 = 7235;

    let width = 640;
    let height = 480;
    let codec = "-vcodec libvpx -pix_fmt yuv420p -b:v 1000k -deadline realtime";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {codec} -rtsp_transport tcp"
    );

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7225,
            p_bc: 7230,
            whep: whep_port,
        },
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_ipv6_udp() {
    let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7040;
    let whep_port: u16 = 7055;

    let width = 640;
    let height = 480;
    let codec = "-vcodec libvpx -pix_fmt yuv420p -b:v 1000k -deadline realtime";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {codec}");

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7045,
            p_bc: 7050,
            whep: whep_port,
        },
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_ipv6_tcp() {
    let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7240;
    let whep_port: u16 = 7255;

    let width = 640;
    let height = 480;
    let codec = "-vcodec libvpx -pix_fmt yuv420p -b:v 1000k -deadline realtime";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {codec} -rtsp_transport tcp"
    );

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7245,
            p_bc: 7250,
            whep: whep_port,
        },
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp9_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7060;
    let whep_port: u16 = 7075;

    let width = 640;
    let height = 480;
    let codec =
        "-strict experimental -vcodec libvpx-vp9 -pix_fmt yuv420p -b:v 1000k -deadline realtime";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {codec}");

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7065,
            p_bc: 7070,
            whep: whep_port,
        },
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp9_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7260;
    let whep_port: u16 = 7275;

    let width = 640;
    let height = 480;
    let codec =
        "-strict experimental -vcodec libvpx-vp9 -pix_fmt yuv420p -b:v 1000k -deadline realtime";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {codec} -rtsp_transport tcp"
    );

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7265,
            p_bc: 7270,
            whep: whep_port,
        },
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_opus_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7080;
    let whep_port: u16 = 7095;

    let codec = "-acodec libopus";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {codec}");

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7085,
            p_bc: 7090,
            whep: whep_port,
        },
        Detect {
            audio: Some(2),
            video: None,
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_opus_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7280;
    let whep_port: u16 = 7295;

    let codec = "-acodec libopus";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {codec} -rtsp_transport tcp");

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7285,
            p_bc: 7290,
            whep: whep_port,
        },
        Detect {
            audio: Some(2),
            video: None,
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_g722_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7120;
    let whep_port: u16 = 7135;

    let codec = "-acodec g722";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {codec}");

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7125,
            p_bc: 7130,
            whep: whep_port,
        },
        Detect {
            audio: Some(1),
            video: None,
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_g722_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7320;
    let whep_port: u16 = 7335;

    let codec = "-acodec g722";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {codec} -rtsp_transport tcp");

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7325,
            p_bc: 7330,
            whep: whep_port,
        },
        Detect {
            audio: Some(1),
            video: None,
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_opus_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7140;
    let whep_port: u16 = 7155;

    let width = 640;
    let height = 480;
    let a_codec = "-acodec libopus";
    let v_codec = "-vcodec libvpx -pix_fmt yuv420p -b:v 1000k -deadline realtime";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size={width}x{height}:rate=30 {a_codec} {v_codec}"
    );

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7145,
            p_bc: 7150,
            whep: whep_port,
        },
        Detect {
            audio: Some(2),
            video: Some((width, height)),
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_opus_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 7340;
    let whep_port: u16 = 7355;

    let width = 640;
    let height = 480;
    let a_codec = "-acodec libopus";
    let v_codec = "-vcodec libvpx -pix_fmt yuv420p -b:v 1000k -deadline realtime";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size={width}x{height}:rate=30 {a_codec} {v_codec} -rtsp_transport tcp"
    );

    helper_livetwo_cycle_rtsp(
        ip,
        port,
        &prefix,
        Ports {
            whip: whip_port,
            p_ab: 7345,
            p_bc: 7350,
            whep: whep_port,
        },
        Detect {
            audio: Some(2),
            video: Some((width, height)),
        },
        Transport::Tcp,
    )
    .await;
}

pub fn stream_id(stream: &str) -> String {
    format!("test-cycle-{stream}")
}

async fn helper_livetwo_cycle_rtsp(
    ip: IpAddr,
    port: u16,
    prefix: &str,
    ports: Ports,
    detect: Detect,
    transport: Transport,
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

    let stream_a = stream_id("a");

    tokio::spawn(livetwo::whip::into(
        format!(
            "{}://{}",
            livetwo::SCHEME_RTSP_SERVER,
            SocketAddr::new(ip, ports.whip),
        ),
        format!("http://{addr}{}", api::path::whip(&stream_a)),
        None,
        Some(format!(
            "{prefix} -f rtsp 'rtsp://{}'",
            SocketAddr::new(ip, ports.whip),
        )),
    ));

    let mut result = None;
    for _ in 0..100 {
        let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();

        if let Some(r) = body.into_iter().find(|i| i.id == stream_a)
            && !r.publish.sessions.is_empty()
        {
            let s = r.publish.sessions[0].clone();
            if s.state == api::response::RTCPeerConnectionState::Connected {
                result = Some(s);
                break;
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(result.is_some());

    // TODO: publish.state == connected is not ready
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    tokio::spawn(livetwo::whep::from(
        format!(
            "{}://{}",
            livetwo::SCHEME_RTSP_SERVER,
            SocketAddr::new(ip, ports.p_ab),
        ),
        format!("http://{addr}{}", api::path::whep(&stream_a)),
        None,
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

        if let Some(r) = body.into_iter().find(|i| i.id == stream_a)
            && !r.subscribe.sessions.is_empty()
        {
            let s = r.subscribe.sessions[0].clone();
            if s.state == api::response::RTCPeerConnectionState::Connected {
                result = Some(s);
                break;
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(result.is_some());

    tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;

    let stream_b = stream_id("b");

    tokio::spawn(livetwo::whip::into(
        format!(
            "{}://{}{}",
            livetwo::SCHEME_RTSP_CLIENT,
            SocketAddr::new(ip, ports.p_ab),
            transport.as_str()
        ),
        format!("http://{addr}{}", api::path::whip(&stream_b)),
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

        if let Some(r) = body.into_iter().find(|i| i.id == stream_b)
            && !r.publish.sessions.is_empty()
        {
            let s = r.publish.sessions[0].clone();
            if s.state == api::response::RTCPeerConnectionState::Connected {
                result = Some(s);
                break;
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(result.is_some());

    let stream_c = stream_id("c");

    tokio::spawn(livetwo::whip::into(
        format!(
            "{}://{}",
            livetwo::SCHEME_RTSP_SERVER,
            SocketAddr::new(ip, ports.p_bc),
        ),
        format!("http://{addr}{}", api::path::whip(&stream_c)),
        None,
        None,
    ));

    tokio::spawn(livetwo::whep::from(
        format!(
            "{}://{}{}",
            livetwo::SCHEME_RTSP_CLIENT,
            SocketAddr::new(ip, ports.p_bc),
            transport.as_str()
        ),
        format!("http://{addr}{}", api::path::whep(&stream_b)),
        None,
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

        if let Some(r) = body.into_iter().find(|i| i.id == stream_b)
            && !r.subscribe.sessions.is_empty()
        {
            let s = r.subscribe.sessions[0].clone();
            if s.state == api::response::RTCPeerConnectionState::Connected {
                result = Some(s);
                break;
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(result.is_some());

    tokio::time::sleep(tokio::time::Duration::from_millis(3000)).await;

    tokio::spawn(livetwo::whep::from(
        format!(
            "{}://{}",
            livetwo::SCHEME_RTSP_SERVER,
            SocketAddr::new(ip, ports.whep),
        ),
        format!("http://{addr}{}", api::path::whep(&stream_c)),
        None,
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

        if let Some(r) = body.into_iter().find(|i| i.id == stream_c)
            && !r.subscribe.sessions.is_empty()
        {
            let s = r.subscribe.sessions[0].clone();
            if s.state == api::response::RTCPeerConnectionState::Connected {
                result = Some(s);
                break;
            }
        }

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
                "{}://{}{}",
                livetwo::SCHEME_RTSP_CLIENT,
                SocketAddr::new(ip, ports.whep),
                transport.as_str()
            ),
            "-show_streams",
            "-of",
            "json",
        ])
        .output()
        .await
        .expect("Failed to execute command");

    tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

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
