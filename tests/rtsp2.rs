use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Once;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

mod common;
use common::shutdown_signal;

// === RTSP Bootstrapping ===
//
// - ffmpeg → whip into rtsp server
//
// # stream: A
//
// - whep from rtsp server
// - whip into rtsp client
//
// # stream: B
//
// - whip into rtsp server
// - whep from rtsp client
//
// # stream: C
//
// - whep from rtsp server
// - ffprobe

const WEBRTC_ICE_UDP_ADDRS: &str = "127.0.0.1:0";

static TRACING_INIT: Once = Once::new();

fn init_rtsp2_test_environment() {
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
fn rtsp2_test_environment_pins_webrtc_ice_to_loopback() {
    init_rtsp2_test_environment();

    assert_eq!(
        std::env::var("LIVE777_WEBRTC_ICE_UDP_ADDRS").as_deref(),
        Ok(WEBRTC_ICE_UDP_ADDRS)
    );
    assert_eq!(
        livetwo::utils::webrtc::ice_udp_addrs(),
        vec![WEBRTC_ICE_UDP_ADDRS.parse::<SocketAddr>().unwrap()]
    );
}

fn rtsp2_ice_candidate_hint(text: &str) -> &'static str {
    if text.contains("a=candidate:") && (text.contains(" 0.0.0.0 ") || text.contains(" :: ")) {
        " RTSP2 test ICE candidate override did not apply: SDP candidate contains an unspecified address; expected LIVE777_WEBRTC_ICE_UDP_ADDRS=127.0.0.1:0 before PeerConnection creation."
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

#[derive(Clone, Copy)]
enum Transport {
    Udp,
    Tcp,
}

impl Transport {
    fn as_query_param(&self) -> &str {
        match self {
            Transport::Udp => "",
            Transport::Tcp => "?transport=tcp",
        }
    }

    fn as_ffmpeg_flag(&self) -> &str {
        match self {
            Transport::Udp => "",
            Transport::Tcp => "-rtsp_transport tcp",
        }
    }

    fn ffprobe_args(&self) -> &[&str] {
        match self {
            Transport::Udp => &[],
            Transport::Tcp => &["-rtsp_transport", "tcp"],
        }
    }
}

struct Ports {
    whip: u16,
    p_ab: u16,
    p_bc: u16,
    whep: u16,
}

struct MediaExpectation {
    audio_channels: Option<u8>,
    video_resolution: Option<(u16, u16)>,
}

struct TestConfig {
    ip: IpAddr,
    server_port: u16,
    ffmpeg_command: String,
    media: MediaExpectation,
    transport: Transport,
}

const CONNECTION_CHECK_INTERVAL_MS: u64 = 100;
const MAX_CONNECTION_ATTEMPTS: u32 = 300;
const STREAM_STABILIZATION_MS: u64 = 1000;
const INTER_STREAM_DELAY_MS: u64 = 3000;
const FFPROBE_PREPARATION_MS: u64 = 7000;
const FFPROBE_TIMEOUT_MS: u64 = 5000;
const FFPROBE_MAX_RETRIES: u32 = 3;
const FFPROBE_RETRY_DELAY_MS: u64 = 3000;

#[tokio::test]
async fn test_livetwo_cycle_rtsp_h264_udp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_h264_command(1280, 720, Transport::Udp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Udp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_h264_tcp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_h264_command(1280, 720, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Tcp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_h265_udp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_h265_command(1280, 720, Transport::Udp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Udp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_h265_tcp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_h265_command(1280, 720, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Tcp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_udp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_vp8_command(1280, 720, Transport::Udp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Udp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_tcp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_vp8_command(1280, 720, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Tcp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_ipv6_udp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V6(Ipv6Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_vp8_command(1280, 720, Transport::Udp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Udp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_ipv6_tcp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V6(Ipv6Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_vp8_command(1280, 720, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Tcp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp9_udp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_vp9_command(1280, 720, Transport::Udp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Udp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp9_tcp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_vp9_command(1280, 720, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Tcp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_opus_udp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_opus_command(Transport::Udp),
        media: MediaExpectation {
            audio_channels: Some(2),
            video_resolution: None,
        },
        transport: Transport::Udp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_opus_tcp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_opus_command(Transport::Tcp),
        media: MediaExpectation {
            audio_channels: Some(2),
            video_resolution: None,
        },
        transport: Transport::Tcp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_g722_udp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_g722_command(Transport::Udp),
        media: MediaExpectation {
            audio_channels: Some(1),
            video_resolution: None,
        },
        transport: Transport::Udp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_g722_tcp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_g722_command(Transport::Tcp),
        media: MediaExpectation {
            audio_channels: Some(1),
            video_resolution: None,
        },
        transport: Transport::Tcp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_opus_udp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_vp8_opus_command(1280, 720, Transport::Udp),
        media: MediaExpectation {
            audio_channels: Some(2),
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Udp,
    })
    .await;
}

#[tokio::test]
async fn test_livetwo_cycle_rtsp_vp8_opus_tcp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ffmpeg_command: build_vp8_opus_command(1280, 720, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: Some(2),
            video_resolution: Some((1280, 720)),
        },
        transport: Transport::Tcp,
    })
    .await;
}

fn build_h264_command(width: u16, height: u16, transport: Transport) -> String {
    let vcodec = "-vcodec libx264 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 23 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 \
            {vcodec} \
            {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

fn build_h265_command(width: u16, height: u16, transport: Transport) -> String {
    let vcodec = "-vcodec libx265 -pix_fmt yuv420p -g 30 -keyint_min 30 -crf 25 -preset ultrafast -tune zerolatency -profile:v main -level 4.1";
    format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 \
            {vcodec} \
            {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

fn build_vp8_command(width: u16, height: u16, transport: Transport) -> String {
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 \
            {vcodec} \
            {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

fn build_vp9_command(width: u16, height: u16, transport: Transport) -> String {
    let vcodec = "-vcodec libvpx-vp9 -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 5 -row-mt 1 -tile-columns 2 -frame-parallel 1 -b:v 1800k -maxrate 2200k -bufsize 4400k";
    format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 \
            -strict experimental {vcodec} \
            {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

fn build_opus_command(transport: Transport) -> String {
    let acodec = "-acodec libopus -ar 48000 -ac 2 -b:a 48k -application voip -frame_duration 10 -vbr constrained";
    format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000
            {acodec} \
            {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

fn build_g722_command(transport: Transport) -> String {
    format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -acodec g722 \
         {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

fn build_vp8_opus_command(width: u16, height: u16, transport: Transport) -> String {
    let acodec = "-acodec libopus -ar 48000 -ac 2 -b:a 48k -application voip -frame_duration 10 -vbr constrained";
    let vcodec = "-vcodec libvpx -pix_fmt yuv420p -g 30 -keyint_min 30 -deadline realtime -speed 4 -b:v 2000k -maxrate 2500k -bufsize 5000k";
    format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 \
         -f lavfi -i testsrc=size={width}x{height}:rate=30 \
            {acodec} \
            {vcodec} \
            {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

async fn run_rtsp_cycle_test(config: TestConfig) {
    init_rtsp2_test_environment();

    // Allocate all ports dynamically to avoid conflicts under nextest parallel execution.
    let ports = Ports {
        whip: pick_tcp_port(config.ip).await,
        p_ab: pick_tcp_port(config.ip).await,
        p_bc: pick_tcp_port(config.ip).await,
        whep: pick_tcp_port(config.ip).await,
    };

    let server_addr = setup_liveion_server(config.ip, config.server_port).await;
    let ct = CancellationToken::new();

    create_default_stream(&server_addr).await;

    // Stream A: ffmpeg → RTSP server → WebRTC
    let stream_a = stream_id("a");
    let mut handle_a_whip =
        start_stream_a_whip(ct.clone(), &config, &ports, &server_addr, &stream_a).await;
    wait_for_publish_connected_with_diagnostics(
        &server_addr,
        &stream_a,
        &mut handle_a_whip,
        ports.whip,
        server_addr,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(STREAM_STABILIZATION_MS)).await;

    // Stream A → RTSP server → Stream B
    let handle_a_whep =
        start_stream_a_whep(ct.clone(), &config, &ports, &server_addr, &stream_a).await;
    wait_for_subscribe_connected(&server_addr, &stream_a).await;
    tokio::time::sleep(Duration::from_millis(INTER_STREAM_DELAY_MS)).await;

    // Stream B: RTSP client → WebRTC
    let stream_b = stream_id("b");
    let handle_b_whip =
        start_stream_b_whip(ct.clone(), &config, &ports, &server_addr, &stream_b).await;
    wait_for_publish_connected(&server_addr, &stream_b).await;

    // Stream C: Stream B → RTSP server
    let stream_c = stream_id("c");
    let handle_c_whip =
        start_stream_c_whip(ct.clone(), &config, &ports, &server_addr, &stream_c).await;

    let handle_b_whep =
        start_stream_b_whep(ct.clone(), &config, &ports, &server_addr, &stream_b).await;
    wait_for_subscribe_connected(&server_addr, &stream_b).await;
    wait_for_publish_connected(&server_addr, &stream_c).await;
    tokio::time::sleep(Duration::from_millis(INTER_STREAM_DELAY_MS)).await;

    // Stream C → RTSP server → ffprobe
    let handle_c_whep =
        start_stream_c_whep(ct.clone(), &config, &ports, &server_addr, &stream_c).await;
    wait_for_subscribe_connected(&server_addr, &stream_c).await;
    tokio::time::sleep(Duration::from_millis(FFPROBE_PREPARATION_MS)).await;

    // Verify with ffprobe
    verify_stream_with_ffprobe(&config, &ports).await;

    ct.cancel();

    match tokio::try_join!(
        handle_a_whip,
        handle_a_whep,
        handle_b_whip,
        handle_b_whep,
        handle_c_whip,
        handle_c_whep
    ) {
        Ok((
            result_a_whip,
            result_a_whep,
            result_b_whip,
            result_b_whep,
            result_c_whip,
            result_c_whep,
        )) => {
            assert!(result_a_whip.is_ok());
            assert!(result_a_whep.is_ok());
            assert!(result_b_whip.is_ok());
            assert!(result_b_whep.is_ok());
            assert!(result_c_whip.is_ok());
            assert!(result_c_whep.is_ok());
        }
        Err(e) => panic!("Task panicked with: {:?}", e),
    };
}

async fn setup_liveion_server(ip: IpAddr, port: u16) -> SocketAddr {
    let cfg = liveion::config::Config::default();
    let listener = TcpListener::bind(SocketAddr::new(ip, port))
        .await
        .expect("Failed to bind server");
    let addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    addr
}

async fn create_default_stream(server_addr: &SocketAddr) {
    let res = reqwest::Client::new()
        .post(format!("http://{server_addr}{}", api::path::streams("-")))
        .send()
        .await
        .expect("Failed to create default stream");

    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    let res = reqwest::get(format!("http://{server_addr}{}", api::path::streams("")))
        .await
        .expect("Failed to get streams");

    let body = res
        .json::<Vec<api::response::Stream>>()
        .await
        .expect("Failed to parse streams response");

    assert_eq!(1, body.len(), "Expected exactly one default stream");
}

async fn start_stream_a_whip(
    ct: CancellationToken,
    config: &TestConfig,
    ports: &Ports,
    server_addr: &SocketAddr,
    stream_id: &str,
) -> JoinHandle<anyhow::Result<()>> {
    let rtsp_addr = SocketAddr::new(config.ip, ports.whip);
    let ffmpeg_cmd = config.ffmpeg_command.replace("{}", &rtsp_addr.to_string());

    tokio::spawn(livetwo::whip::into(
        ct,
        format!("{}://{}", livetwo::SCHEME_RTSP_SERVER, rtsp_addr),
        format!("http://{server_addr}{}", api::path::whip(stream_id)),
        None,
        Some(ffmpeg_cmd),
    ))
}

async fn start_stream_a_whep(
    ct: CancellationToken,
    config: &TestConfig,
    ports: &Ports,
    server_addr: &SocketAddr,
    stream_id: &str,
) -> JoinHandle<anyhow::Result<()>> {
    let rtsp_addr = SocketAddr::new(config.ip, ports.p_ab);

    tokio::spawn(livetwo::whep::from(
        ct,
        format!("{}://{}", livetwo::SCHEME_RTSP_SERVER, rtsp_addr),
        format!("http://{server_addr}{}", api::path::whep(stream_id)),
        None,
        None,
        None,
        None,
    ))
}

async fn start_stream_b_whip(
    ct: CancellationToken,
    config: &TestConfig,
    ports: &Ports,
    server_addr: &SocketAddr,
    stream_id: &str,
) -> JoinHandle<anyhow::Result<()>> {
    let rtsp_addr = SocketAddr::new(config.ip, ports.p_ab);

    tokio::spawn(livetwo::whip::into(
        ct,
        format!(
            "{}://{}{}",
            livetwo::SCHEME_RTSP_CLIENT,
            rtsp_addr,
            config.transport.as_query_param()
        ),
        format!("http://{server_addr}{}", api::path::whip(stream_id)),
        None,
        None,
    ))
}

async fn start_stream_c_whip(
    ct: CancellationToken,
    config: &TestConfig,
    ports: &Ports,
    server_addr: &SocketAddr,
    stream_c_id: &str,
) -> JoinHandle<anyhow::Result<()>> {
    let rtsp_addr = SocketAddr::new(config.ip, ports.p_bc);

    tokio::spawn(livetwo::whip::into(
        ct.clone(),
        format!("{}://{}", livetwo::SCHEME_RTSP_SERVER, rtsp_addr),
        format!("http://{server_addr}{}", api::path::whip(stream_c_id)),
        None,
        None,
    ))
}

async fn start_stream_b_whep(
    ct: CancellationToken,
    config: &TestConfig,
    ports: &Ports,
    server_addr: &SocketAddr,
    stream_b_id: &str,
) -> JoinHandle<anyhow::Result<()>> {
    let rtsp_addr = SocketAddr::new(config.ip, ports.p_bc);

    tokio::spawn(livetwo::whep::from(
        ct,
        format!(
            "{}://{}{}",
            livetwo::SCHEME_RTSP_CLIENT,
            rtsp_addr,
            config.transport.as_query_param()
        ),
        format!("http://{server_addr}{}", api::path::whep(stream_b_id)),
        None,
        None,
        None,
        None,
    ))
}

async fn start_stream_c_whep(
    ct: CancellationToken,
    config: &TestConfig,
    ports: &Ports,
    server_addr: &SocketAddr,
    stream_id: &str,
) -> JoinHandle<anyhow::Result<()>> {
    let rtsp_addr = SocketAddr::new(config.ip, ports.whep);

    tokio::spawn(livetwo::whep::from(
        ct,
        format!("{}://{}", livetwo::SCHEME_RTSP_SERVER, rtsp_addr),
        format!("http://{server_addr}{}", api::path::whep(stream_id)),
        None,
        None,
        None,
        None,
    ))
}

async fn wait_for_publish_connected_with_diagnostics(
    server_addr: &SocketAddr,
    stream_id: &str,
    handle_whip: &mut JoinHandle<anyhow::Result<()>>,
    whip_port: u16,
    liveion_addr: SocketAddr,
) {
    let mut last_state = None;
    let mut last_codecs = Vec::new();
    for attempt in 0..MAX_CONNECTION_ATTEMPTS {
        if handle_whip.is_finished() {
            let result = handle_whip.await.unwrap();
            let result_debug = format!("{:?}", result);
            let ice_hint = rtsp2_ice_candidate_hint(&result_debug);
            panic!(
                "WHIP task exited before publish connected: result={result_debug}, \
                 whip_port={whip_port}, liveion={liveion_addr}, stream={stream_id}, \
                 last_state={last_state:?}, last_codecs={last_codecs:?}.{ice_hint}"
            );
        }

        let res = reqwest::get(format!("http://{server_addr}{}", api::path::streams("")))
            .await
            .expect("Failed to get streams");

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res
            .json::<Vec<api::response::Stream>>()
            .await
            .expect("Failed to parse streams response");

        if let Some(stream) = body.into_iter().find(|s| s.id == stream_id)
            && !stream.publish.sessions.is_empty()
        {
            let state = stream.publish.sessions[0].state;
            last_state = Some(state);
            last_codecs = stream.codecs.clone();
            if state == api::response::RTCPeerConnectionState::Connected {
                return;
            }
        }

        if attempt == MAX_CONNECTION_ATTEMPTS - 1 {
            panic!(
                "Stream '{}' did not reach connected state after {} attempts; \
                 whip_port={whip_port}, liveion={liveion_addr}, last_state={:?}, last_codecs={:?}",
                stream_id, MAX_CONNECTION_ATTEMPTS, last_state, last_codecs
            );
        }

        tokio::time::sleep(Duration::from_millis(CONNECTION_CHECK_INTERVAL_MS)).await;
    }
}

async fn wait_for_publish_connected(server_addr: &SocketAddr, stream_id: &str) {
    wait_for_connection_state(
        server_addr,
        stream_id,
        |stream| !stream.publish.sessions.is_empty(),
        |stream| stream.publish.sessions[0].state,
    )
    .await;
}

async fn wait_for_subscribe_connected(server_addr: &SocketAddr, stream_id: &str) {
    wait_for_connection_state(
        server_addr,
        stream_id,
        |stream| !stream.subscribe.sessions.is_empty(),
        |stream| stream.subscribe.sessions[0].state,
    )
    .await;
}

async fn wait_for_connection_state<F, G>(
    server_addr: &SocketAddr,
    stream_id: &str,
    has_sessions: F,
    get_state: G,
) where
    F: Fn(&api::response::Stream) -> bool,
    G: Fn(&api::response::Stream) -> api::response::RTCPeerConnectionState,
{
    let mut last_state = None;
    let mut last_codecs = Vec::new();
    for attempt in 0..MAX_CONNECTION_ATTEMPTS {
        let res = reqwest::get(format!("http://{server_addr}{}", api::path::streams("")))
            .await
            .expect("Failed to get streams");

        assert_eq!(http::StatusCode::OK, res.status());

        let body = res
            .json::<Vec<api::response::Stream>>()
            .await
            .expect("Failed to parse streams response");

        if let Some(stream) = body.into_iter().find(|s| s.id == stream_id)
            && has_sessions(&stream)
        {
            let state = get_state(&stream);
            last_state = Some(state);
            last_codecs = stream.codecs.clone();
            if state == api::response::RTCPeerConnectionState::Connected {
                return;
            }
        }

        if attempt == MAX_CONNECTION_ATTEMPTS - 1 {
            panic!(
                "Stream '{}' did not reach connected state after {} attempts; last_state={:?}, last_codecs={:?}",
                stream_id, MAX_CONNECTION_ATTEMPTS, last_state, last_codecs
            );
        }

        tokio::time::sleep(Duration::from_millis(CONNECTION_CHECK_INTERVAL_MS)).await;
    }
}

async fn verify_stream_with_ffprobe(config: &TestConfig, ports: &Ports) {
    let rtsp_url = format!(
        "{}://{}",
        livetwo::SCHEME_RTSP_CLIENT,
        SocketAddr::new(config.ip, ports.whep)
    );

    let mut last_error = None;

    for attempt in 0..FFPROBE_MAX_RETRIES {
        let output = Command::new("ffprobe")
            .args(config.transport.ffprobe_args())
            .args([
                "-v",
                "error",
                "-hide_banner",
                "-i",
                &rtsp_url,
                "-show_streams",
                "-of",
                "json",
            ])
            .output()
            .await
            .expect("Failed to execute ffprobe");

        tokio::time::sleep(Duration::from_millis(FFPROBE_TIMEOUT_MS)).await;

        if output.status.success() {
            validate_ffprobe_output(&output.stdout, &config.media);
            return;
        }

        last_error = Some(format!(
            "Attempt {}/{} failed\nstdout: {}\nstderr: {}",
            attempt + 1,
            FFPROBE_MAX_RETRIES,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));

        if attempt < FFPROBE_MAX_RETRIES - 1 {
            tokio::time::sleep(Duration::from_millis(FFPROBE_RETRY_DELAY_MS)).await;
        }
    }

    panic!(
        "ffprobe failed after {} attempts:\n{}",
        FFPROBE_MAX_RETRIES,
        last_error.unwrap()
    );
}

fn validate_ffprobe_output(stdout: &[u8], expected: &MediaExpectation) {
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

    let result: Ffprobe =
        serde_json::from_slice(stdout).expect("Failed to parse ffprobe JSON output");

    for stream in result.streams.iter() {
        match stream.codec_type.as_str() {
            "video" => {
                if let Some((expected_width, expected_height)) = expected.video_resolution {
                    assert_eq!(
                        stream.width.unwrap(),
                        expected_width,
                        "Video width mismatch"
                    );
                    assert_eq!(
                        stream.height.unwrap(),
                        expected_height,
                        "Video height mismatch"
                    );
                } else {
                    panic!("Unexpected video stream found");
                }
            }
            "audio" => {
                if let Some(expected_channels) = expected.audio_channels {
                    assert_eq!(
                        stream.channels.unwrap(),
                        expected_channels,
                        "Audio channels mismatch"
                    );
                } else {
                    panic!("Unexpected audio stream found");
                }
            }
            _ => panic!("Unknown codec_type: {}", stream.codec_type),
        }
    }
}

fn stream_id(suffix: &str) -> String {
    format!("test-cycle-{}", suffix)
}
