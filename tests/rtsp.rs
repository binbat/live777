#![cfg(feature = "rtsp")]

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Once,
};

use tokio::net::TcpListener;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

mod common;
use common::shutdown_signal;

enum Transport {
    Udp,
    Tcp,
}

impl Transport {
    fn ffprobe_args(&self) -> &[&str] {
        match self {
            Transport::Udp => &[],
            Transport::Tcp => &["-rtsp_transport", "tcp"],
        }
    }
}
struct Detect {
    // channels
    audio: Option<u8>,
    // (width, height)
    video: Option<(u16, u16)>,
}

const CONNECTION_WAIT_ATTEMPTS: usize = 300;
const WEBRTC_ICE_UDP_ADDRS: &str = "127.0.0.1:0";

static TRACING_INIT: Once = Once::new();

fn init_rtsp_test_environment() {
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
fn rtsp_test_environment_pins_webrtc_ice_to_loopback() {
    init_rtsp_test_environment();

    assert_eq!(
        std::env::var("LIVE777_WEBRTC_ICE_UDP_ADDRS").as_deref(),
        Ok(WEBRTC_ICE_UDP_ADDRS)
    );
    assert_eq!(
        livetwo::utils::webrtc::ice_udp_addrs(),
        vec![WEBRTC_ICE_UDP_ADDRS.parse::<SocketAddr>().unwrap()]
    );
}

async fn pick_tcp_port(ip: IpAddr) -> u16 {
    let listener = TcpListener::bind(SocketAddr::new(ip, 0))
        .await
        .expect("Failed to reserve temporary TCP port");
    listener
        .local_addr()
        .expect("Failed to read temporary TCP port")
        .port()
}

fn rtsp_url(ip: IpAddr, port: u16, stream_id: &str) -> String {
    let host = match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    };
    format!("rtsp://{host}:{port}/{stream_id}")
}

#[tokio::test]
async fn test_livetwo_rtsp_h264_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libx264 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 23 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_h264_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libx264 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 23 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_h265_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libx265 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 25 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_h265_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libx265 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 25 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_ipv6_udp() {
    let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_ipv6_tcp() {
    let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec} -rtsp_transport tcp"
    );

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp9_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx-vp9 -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 5 -row-mt 1 -tile-columns 2 -frame-parallel 1 -b:v 1800k -maxrate 2200k -bufsize 4400k";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -strict experimental {vcodec}"
    );

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp9_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx-vp9 -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 5 -row-mt 1 -tile-columns 2 -frame-parallel 1 -b:v 1800k -maxrate 2200k -bufsize 4400k";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 -strict experimental {vcodec} -rtsp_transport tcp"
    );

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: None,
            video: Some((width, height)),
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_opus_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let acodec = "-acodec libopus -ar 48000 -ac 2 -b:a 48k -application voip -frame_duration 10 -vbr constrained";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {acodec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: Some(2),
            video: None,
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_opus_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let acodec = "-acodec libopus -ar 48000 -ac 2 -b:a 48k -application voip -frame_duration 10 -vbr constrained";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {acodec} -rtsp_transport tcp");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: Some(2),
            video: None,
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_g722_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let acodec = "-acodec g722 -ar 16000";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {acodec}");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: Some(1),
            video: None,
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_g722_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let acodec = "-acodec g722 -ar 16000";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {acodec} -rtsp_transport tcp");

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: Some(1),
            video: None,
        },
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_opus_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;

    let acodec = "-acodec libopus -ar 48000 -ac 2 -b:a 48k -application voip -frame_duration 10 -vbr constrained";
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size={width}x{height}:rate=30 {acodec} {vcodec}"
    );

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: Some(2),
            video: Some((width, height)),
        },
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_opus_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let rtsp_port: u16 = 0;

    let width = 1280;
    let height = 720;

    let acodec = "-acodec libopus -ar 48000 -ac 2 -b:a 48k -application voip -frame_duration 10 -vbr constrained";
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -f lavfi -i testsrc=size={width}x{height}:rate=30 {acodec} {vcodec} -rtsp_transport tcp"
    );

    helper_livetwo_rtsp(
        ip,
        port,
        &prefix,
        rtsp_port,
        Detect {
            audio: Some(2),
            video: Some((width, height)),
        },
        Transport::Tcp,
    )
    .await;
}

async fn helper_livetwo_rtsp(
    ip: IpAddr,
    port: u16,
    prefix: &str,
    rtsp_port: u16,
    detect: Detect,
    transport: Transport,
) {
    init_rtsp_test_environment();

    let rtsp_port = if rtsp_port == 0 {
        pick_tcp_port(ip).await
    } else {
        rtsp_port
    };

    let mut cfg = liveion::config::Config::default();
    cfg.rtsp.listen = SocketAddr::new(ip, rtsp_port);

    let listener = TcpListener::bind(SocketAddr::new(ip, port)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    // Wait briefly for the RTSP server to start listening.
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    let stream_id = "-";
    let ct = CancellationToken::new();

    let push_url = rtsp_url(ip, rtsp_port, stream_id);
    let ffmpeg_cmd = format!("{prefix} -f rtsp '{push_url}'");
    let ffmpeg_handle = tokio::spawn(run_ffmpeg(ct.clone(), ffmpeg_cmd));

    let mut result = None;
    let mut last_state = None;
    let mut last_codecs = Vec::new();
    for _ in 0..CONNECTION_WAIT_ATTEMPTS {
        let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();

        if let Some(r) = body.into_iter().find(|i| i.id == stream_id)
            && !r.publish.sessions.is_empty()
        {
            let s = r.publish.sessions[0].clone();
            last_state = Some(s.state);
            last_codecs = r.codecs.clone();
            if s.state == api::response::RTCPeerConnectionState::Connected && !r.codecs.is_empty() {
                result = Some(s);
                break;
            }
        };

        if ffmpeg_handle.is_finished() {
            let result_ffmpeg = ffmpeg_handle.await.unwrap();
            panic!(
                "ffmpeg task exited before publish connected: result={result_ffmpeg:?}, rtsp_port={rtsp_port}, liveion={addr}, last_state={last_state:?}, last_codecs={last_codecs:?}"
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(
        result.is_some(),
        "Publish session did not reach Connected state with codecs within {}ms: rtsp_port={rtsp_port}, liveion={addr}, last_state={last_state:?}, last_codecs={last_codecs:?}",
        CONNECTION_WAIT_ATTEMPTS * 100,
    );

    // Wait a moment for media to flow through to the pull side.
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    let input_url = rtsp_url(ip, rtsp_port, stream_id);
    let output = Command::new("ffprobe")
        .args(transport.ffprobe_args())
        .args([
            "-v",
            "error",
            "-hide_banner",
            "-i",
            &input_url,
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

    ct.cancel();

    let result_ffmpeg = ffmpeg_handle.await.unwrap();

    assert!(result_ffmpeg.is_ok());
}

async fn run_ffmpeg(ct: CancellationToken, command: String) -> anyhow::Result<()> {
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .kill_on_drop(true)
        .spawn()?;
    tokio::select! {
        _ = ct.cancelled() => {
            let _ = child.kill().await;
            Ok(())
        }
        status = child.wait() => {
            status.map(|_| ()).map_err(|e| e.into())
        }
    }
}
