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

fn rtsp_ice_candidate_override_hint(text: &str) -> &'static str {
    if text.contains("a=candidate:") && (text.contains(" 0.0.0.0 ") || text.contains(" :: ")) {
        " RTSP test ICE candidate override did not apply: SDP candidate contains an unspecified address; expected LIVE777_WEBRTC_ICE_UDP_ADDRS=127.0.0.1:0 before PeerConnection creation."
    } else {
        ""
    }
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

#[tokio::test]
async fn test_livetwo_rtsp_h264_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libx264 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 23 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

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
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_h264_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libx264 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 23 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

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
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_h265_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libx265 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 25 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

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
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_h265_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libx265 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 25 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

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
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

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
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

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
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_ipv6_udp() {
    let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

    let width = 1280;
    let height = 720;
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    let prefix = format!("ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 {vcodec}");

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
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_ipv6_tcp() {
    let ip = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

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
        whip_port,
        whep_port,
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

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

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
        whip_port,
        whep_port,
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

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

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
        whip_port,
        whep_port,
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

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

    let acodec = "-acodec libopus -ar 48000 -ac 2 -b:a 48k -application voip -frame_duration 10 -vbr constrained";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {acodec}");

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
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_opus_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

    let acodec = "-acodec libopus -ar 48000 -ac 2 -b:a 48k -application voip -frame_duration 10 -vbr constrained";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {acodec} -rtsp_transport tcp");

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
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_g722_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

    let acodec = "-acodec g722";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {acodec}");

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
        Transport::Udp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_g722_tcp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

    let acodec = "-acodec g722";
    let prefix = format!("ffmpeg -re -f lavfi -i sine=frequency=1000 {acodec} -rtsp_transport tcp");

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
        Transport::Tcp,
    )
    .await;
}

#[tokio::test]
async fn test_livetwo_rtsp_vp8_opus_udp() {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let port = 0;

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

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
        whip_port,
        whep_port,
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

    let whip_port: u16 = 0;
    let whep_port: u16 = 0;

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
        whip_port,
        whep_port,
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
    whip_port: u16,
    whep_port: u16,
    detect: Detect,
    transport: Transport,
) {
    init_rtsp_test_environment();

    let whip_port = if whip_port == 0 {
        pick_tcp_port(ip).await
    } else {
        whip_port
    };
    let whep_port = if whep_port == 0 {
        pick_tcp_port(ip).await
    } else {
        whep_port
    };

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

    let ct = CancellationToken::new();
    let handle_whip = tokio::spawn(livetwo::whip::into(
        ct.clone(),
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
    let mut last_state = None;
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
            last_state = Some(s.state);
            last_codecs = r.codecs.clone();
            if s.state == api::response::RTCPeerConnectionState::Connected && !r.codecs.is_empty() {
                result = Some(s);
                break;
            }
        };

        if handle_whip.is_finished() {
            let result_whip = handle_whip.await.unwrap();
            let result_whip_debug = format!("{result_whip:?}");
            let ice_hint = rtsp_ice_candidate_override_hint(&result_whip_debug);
            panic!(
                "WHIP task exited before publish connected: result={result_whip_debug}, whip_port={whip_port}, whep_port={whep_port}, liveion={addr}, last_state={last_state:?}, last_codecs={last_codecs:?}.{ice_hint}"
            );
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    assert!(
        result.is_some(),
        "Publish session did not reach Connected state with codecs within {}ms: whip_port={whip_port}, whep_port={whep_port}, liveion={addr}, last_state={last_state:?}, last_codecs={last_codecs:?}",
        CONNECTION_WAIT_ATTEMPTS * 100,
    );

    // TODO: publish.state == connected is not ready
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    let handle_whep = tokio::spawn(livetwo::whep::from(
        ct.clone(),
        format!(
            "{}://{}",
            livetwo::SCHEME_RTSP_SERVER,
            SocketAddr::new(ip, whep_port)
        ),
        format!("http://{addr}{}", api::path::whep("-")),
        None,
        None,
        None,
        None,
    ));

    let mut result = None;
    let mut last_state = None;
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

    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    let input_url = format!(
        "{}://{}",
        livetwo::SCHEME_RTSP_CLIENT,
        SocketAddr::new(ip, whep_port)
    );
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

    let result_whip = handle_whip.await.unwrap();
    let result_whep = handle_whep.await.unwrap();

    assert!(result_whip.is_ok());
    assert!(result_whep.is_ok());
}
