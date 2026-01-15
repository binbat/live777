use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::process::Command;

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
    ports: Ports,
    ffmpeg_command: String,
    media: MediaExpectation,
    transport: Transport,
}

const CONNECTION_CHECK_INTERVAL_MS: u64 = 100;
const MAX_CONNECTION_ATTEMPTS: u32 = 100;
const STREAM_STABILIZATION_MS: u64 = 1000;
const INTER_STREAM_DELAY_MS: u64 = 3000;
const FFPROBE_PREPARATION_MS: u64 = 5000;
const FFPROBE_TIMEOUT_MS: u64 = 5000;
const FFPROBE_MAX_RETRIES: u32 = 3;
const FFPROBE_RETRY_DELAY_MS: u64 = 3000;

#[tokio::test]
async fn test_livetwo_cycle_rtsp_h264_udp() {
    run_rtsp_cycle_test(TestConfig {
        ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
        server_port: 0,
        ports: Ports {
            whip: 8000,
            p_ab: 8010,
            p_bc: 8020,
            whep: 8030,
        },
        ffmpeg_command: build_h264_command(640, 480, Transport::Udp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((640, 480)),
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
        ports: Ports {
            whip: 7360,
            p_ab: 7365,
            p_bc: 7370,
            whep: 7375,
        },
        ffmpeg_command: build_h264_command(640, 480, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((640, 480)),
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
        ports: Ports {
            whip: 7160,
            p_ab: 7165,
            p_bc: 7170,
            whep: 7175,
        },
        ffmpeg_command: build_h265_command(640, 480, Transport::Udp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((640, 480)),
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
        ports: Ports {
            whip: 7380,
            p_ab: 7385,
            p_bc: 7390,
            whep: 7395,
        },
        ffmpeg_command: build_h265_command(640, 480, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((640, 480)),
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
        ports: Ports {
            whip: 7020,
            p_ab: 7025,
            p_bc: 7030,
            whep: 7035,
        },
        ffmpeg_command: build_vp8_command(640, 480, Transport::Udp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((640, 480)),
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
        ports: Ports {
            whip: 7220,
            p_ab: 7225,
            p_bc: 7230,
            whep: 7235,
        },
        ffmpeg_command: build_vp8_command(640, 480, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((640, 480)),
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
        ports: Ports {
            whip: 7040,
            p_ab: 7045,
            p_bc: 7050,
            whep: 7055,
        },
        ffmpeg_command: build_vp8_command(640, 480, Transport::Udp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((640, 480)),
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
        ports: Ports {
            whip: 7240,
            p_ab: 7245,
            p_bc: 7250,
            whep: 7255,
        },
        ffmpeg_command: build_vp8_command(640, 480, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((640, 480)),
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
        ports: Ports {
            whip: 7060,
            p_ab: 7065,
            p_bc: 7070,
            whep: 7075,
        },
        ffmpeg_command: build_vp9_command(640, 480, Transport::Udp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((640, 480)),
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
        ports: Ports {
            whip: 7260,
            p_ab: 7265,
            p_bc: 7270,
            whep: 7275,
        },
        ffmpeg_command: build_vp9_command(640, 480, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: None,
            video_resolution: Some((640, 480)),
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
        ports: Ports {
            whip: 7080,
            p_ab: 7085,
            p_bc: 7090,
            whep: 7095,
        },
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
        ports: Ports {
            whip: 7280,
            p_ab: 7285,
            p_bc: 7290,
            whep: 7295,
        },
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
        ports: Ports {
            whip: 7120,
            p_ab: 7125,
            p_bc: 7130,
            whep: 7135,
        },
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
        ports: Ports {
            whip: 7320,
            p_ab: 7325,
            p_bc: 7330,
            whep: 7335,
        },
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
        ports: Ports {
            whip: 7140,
            p_ab: 7145,
            p_bc: 7150,
            whep: 7155,
        },
        ffmpeg_command: build_vp8_opus_command(640, 480, Transport::Udp),
        media: MediaExpectation {
            audio_channels: Some(2),
            video_resolution: Some((640, 480)),
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
        ports: Ports {
            whip: 7340,
            p_ab: 7345,
            p_bc: 7350,
            whep: 7355,
        },
        ffmpeg_command: build_vp8_opus_command(640, 480, Transport::Tcp),
        media: MediaExpectation {
            audio_channels: Some(2),
            video_resolution: Some((640, 480)),
        },
        transport: Transport::Tcp,
    })
    .await;
}

fn build_h264_command(width: u16, height: u16, transport: Transport) -> String {
    format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 \
         -vcodec libx264 -profile:v baseline -level 3.1 -pix_fmt yuv420p \
         -g 15 -keyint_min 15 -b:v 1000k -minrate 1000k -maxrate 1000k \
         -bufsize 1000k -preset ultrafast -tune zerolatency \
         -x264-params repeat_headers=1 {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

fn build_h265_command(width: u16, height: u16, transport: Transport) -> String {
    format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 \
         -vcodec libx265 -preset ultrafast -tune zerolatency \
         -x265-params keyint=15:min-keyint=15:bframes=0:repeat-headers=1 \
         -pix_fmt yuv420p -b:v 1000k -minrate 1000k -maxrate 1000k \
         -bufsize 1000k {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

fn build_vp8_command(width: u16, height: u16, transport: Transport) -> String {
    format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 \
         -vcodec libvpx -pix_fmt yuv420p -b:v 1000k -deadline realtime \
         {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

fn build_vp9_command(width: u16, height: u16, transport: Transport) -> String {
    format!(
        "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate=30 \
         -strict experimental -vcodec libvpx-vp9 -pix_fmt yuv420p \
         -b:v 1000k -deadline realtime {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

fn build_opus_command(transport: Transport) -> String {
    format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 -acodec libopus \
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
    format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000 \
         -f lavfi -i testsrc=size={width}x{height}:rate=30 \
         -acodec libopus -vcodec libvpx -pix_fmt yuv420p \
         -b:v 1000k -deadline realtime {} -f rtsp 'rtsp://{{}}'",
        transport.as_ffmpeg_flag()
    )
}

async fn run_rtsp_cycle_test(config: TestConfig) {
    let server_addr = setup_liveion_server(config.ip, config.server_port).await;

    create_default_stream(&server_addr).await;

    // Stream A: ffmpeg → RTSP server → WebRTC
    let stream_a = stream_id("a");
    start_stream_a(&config, &server_addr, &stream_a).await;
    wait_for_publish_connected(&server_addr, &stream_a).await;
    tokio::time::sleep(Duration::from_millis(STREAM_STABILIZATION_MS)).await;

    // Stream A → RTSP server → Stream B
    start_stream_a_to_b(&config, &server_addr, &stream_a).await;
    wait_for_subscribe_connected(&server_addr, &stream_a).await;
    tokio::time::sleep(Duration::from_millis(INTER_STREAM_DELAY_MS)).await;

    // Stream B: RTSP client → WebRTC
    let stream_b = stream_id("b");
    start_stream_b(&config, &server_addr, &stream_b).await;
    wait_for_publish_connected(&server_addr, &stream_b).await;

    // Stream C: Stream B → RTSP server
    let stream_c = stream_id("c");
    start_stream_b_to_c(&config, &server_addr, &stream_b, &stream_c).await;
    wait_for_subscribe_connected(&server_addr, &stream_b).await;
    tokio::time::sleep(Duration::from_millis(INTER_STREAM_DELAY_MS)).await;

    // Stream C → RTSP server → ffprobe
    start_stream_c_output(&config, &server_addr, &stream_c).await;
    wait_for_subscribe_connected(&server_addr, &stream_c).await;
    tokio::time::sleep(Duration::from_millis(FFPROBE_PREPARATION_MS)).await;

    // Verify with ffprobe
    verify_stream_with_ffprobe(&config).await;
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

async fn start_stream_a(config: &TestConfig, server_addr: &SocketAddr, stream_id: &str) {
    let rtsp_addr = SocketAddr::new(config.ip, config.ports.whip);
    let ffmpeg_cmd = config.ffmpeg_command.replace("{}", &rtsp_addr.to_string());

    tokio::spawn(livetwo::whip::into(
        format!("{}://{}", livetwo::SCHEME_RTSP_SERVER, rtsp_addr),
        format!("http://{server_addr}{}", api::path::whip(stream_id)),
        None,
        Some(ffmpeg_cmd),
    ));
}

async fn start_stream_a_to_b(config: &TestConfig, server_addr: &SocketAddr, stream_id: &str) {
    let rtsp_addr = SocketAddr::new(config.ip, config.ports.p_ab);

    tokio::spawn(livetwo::whep::from(
        format!("{}://{}", livetwo::SCHEME_RTSP_SERVER, rtsp_addr),
        format!("http://{server_addr}{}", api::path::whep(stream_id)),
        None,
        None,
        None,
    ));
}

async fn start_stream_b(config: &TestConfig, server_addr: &SocketAddr, stream_id: &str) {
    let rtsp_addr = SocketAddr::new(config.ip, config.ports.p_ab);

    tokio::spawn(livetwo::whip::into(
        format!(
            "{}://{}{}",
            livetwo::SCHEME_RTSP_CLIENT,
            rtsp_addr,
            config.transport.as_query_param()
        ),
        format!("http://{server_addr}{}", api::path::whip(stream_id)),
        None,
        None,
    ));
}

async fn start_stream_b_to_c(
    config: &TestConfig,
    server_addr: &SocketAddr,
    stream_b_id: &str,
    stream_c_id: &str,
) {
    let rtsp_addr = SocketAddr::new(config.ip, config.ports.p_bc);

    tokio::spawn(livetwo::whip::into(
        format!("{}://{}", livetwo::SCHEME_RTSP_SERVER, rtsp_addr),
        format!("http://{server_addr}{}", api::path::whip(stream_c_id)),
        None,
        None,
    ));

    tokio::spawn(livetwo::whep::from(
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
    ));
}

async fn start_stream_c_output(config: &TestConfig, server_addr: &SocketAddr, stream_id: &str) {
    let rtsp_addr = SocketAddr::new(config.ip, config.ports.whep);

    tokio::spawn(livetwo::whep::from(
        format!("{}://{}", livetwo::SCHEME_RTSP_SERVER, rtsp_addr),
        format!("http://{server_addr}{}", api::path::whep(stream_id)),
        None,
        None,
        None,
    ));
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
            && get_state(&stream) == api::response::RTCPeerConnectionState::Connected
        {
            return;
        }

        if attempt == MAX_CONNECTION_ATTEMPTS - 1 {
            panic!(
                "Stream '{}' did not reach connected state after {} attempts",
                stream_id, MAX_CONNECTION_ATTEMPTS
            );
        }

        tokio::time::sleep(Duration::from_millis(CONNECTION_CHECK_INTERVAL_MS)).await;
    }
}

async fn verify_stream_with_ffprobe(config: &TestConfig) {
    let rtsp_url = format!(
        "{}://{}{}",
        livetwo::SCHEME_RTSP_CLIENT,
        SocketAddr::new(config.ip, config.ports.whep),
        config.transport.as_query_param()
    );

    let mut last_error = None;

    for attempt in 0..FFPROBE_MAX_RETRIES {
        let output = Command::new("ffprobe")
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
