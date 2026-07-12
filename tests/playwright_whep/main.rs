use std::{
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    sync::Once,
    time::Duration,
};

use test_case::test_matrix;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[path = "../common.rs"]
mod common;
mod player;
mod source;

use common::shutdown_signal;
use player::{PlayResult, Player};
use source::{Source, SourceHandle, VideoCodec};

#[cfg(feature = "rsmpeg")]
use livetwo::probe::{ProbeBackend, ProbeConfig, rsmpeg::RsmpegProbe};

static TRACING_INIT: Once = Once::new();

fn init_liveion_test_environment() {
    TRACING_INIT.call_once(|| {
        unsafe {
            std::env::set_var("LIVE777_WEBRTC_ICE_UDP_ADDRS", "127.0.0.1:0");
        }

        let filter = std::env::var("RUST_LOG")
            .unwrap_or_else(|_| "live777=info,liveion=info,livetwo=info,libwish=info".to_string());
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

/// Holds a bound UDP socket so the reserved port cannot be reused by another
/// process until we are ready to hand it to liveion. Dropping the guard
/// releases the port.
struct UdpPortGuard {
    socket: UdpSocket,
}

impl UdpPortGuard {
    fn port(&self) -> u16 {
        self.socket
            .local_addr()
            .expect("Failed to read temporary UDP port")
            .port()
    }
}

fn reserve_udp_port(ip: IpAddr) -> UdpPortGuard {
    let socket = UdpSocket::bind(SocketAddr::new(ip, 0)).expect("Failed to reserve UDP port");
    UdpPortGuard { socket }
}

/// Matrix test for FFmpeg RTP sources played back by the in-process livetwo WHEP player.
/// This runs without any browser dependency.
#[test_matrix(
    [
        source::ffmpeg::FfmpegSource::new(VideoCodec::Vp8),
        source::ffmpeg::FfmpegSource::new(VideoCodec::H264),
    ],
    [player::livetwo::LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whep_livetwo_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// Matrix test for the rsmpeg VP8 source played back by the in-process livetwo WHEP player.
#[cfg(feature = "rsmpeg")]
#[test_matrix(
    [source::rsmpeg_vp8::RsmpegVp8Source::default()],
    [player::livetwo::LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whep_livetwo_rsmpeg_vp8_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// Matrix test for FFmpeg RTP sources played back in a real browser via Playwright.
#[cfg(feature = "whepwright")]
#[test_matrix(
    [
        source::ffmpeg::FfmpegSource::new(VideoCodec::Vp8),
        source::ffmpeg::FfmpegSource::new(VideoCodec::H264),
    ],
    [player::playwright::PlaywrightWhepPlayer::default()]
)]
#[tokio::test]
async fn whep_playwright_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// Matrix test for rsmpeg-generated sources played back in a real browser via Playwright.
#[cfg(all(feature = "whepwright", feature = "rsmpeg"))]
#[test_matrix(
    [source::rsmpeg_vp8::RsmpegVp8Source::default()],
    [player::playwright::PlaywrightWhepPlayer::default()]
)]
#[tokio::test]
async fn whep_playwright_rsmpeg_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// Pure rsmpeg baseline: rsmpeg/FFmpeg VP8 source -> liveion -> rsmpeg decoder.
#[cfg(feature = "rsmpeg")]
#[test_matrix(
    [
        source::rsmpeg_vp8::RsmpegVp8Source::default(),
        source::ffmpeg::FfmpegSource::new(VideoCodec::Vp8),
    ],
    [player::rsmpeg_receiver::RsmpegWhepReceiver::default()]
)]
#[tokio::test]
async fn whep_rsmpeg_baseline_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

async fn run_whep_test_with_host<S, P>(source: S, player: P, bind_ip: IpAddr, whep_host: &str)
where
    S: Source,
    P: Player,
{
    let (_api_addr, port, source_handle, whip_ct, whip_handle) =
        start_published_stream(&source, bind_ip).await;

    // Give the source a moment to produce keyframes before subscribing.
    source.wait_for_ready().await;

    // Run the WHEP player and verify playback.
    let whep_url = format!("http://{whep_host}:{port}{}", api::path::whep("-"));
    let playback = player.play(&whep_url).await.expect("WHEP player failed");

    tracing::info!(
        source = source.name(),
        player = player.name(),
        ?playback,
        "WHEP playback result"
    );

    assert_playback_ok(player.name(), &playback);

    source_handle.stop().await;
    whip_ct.cancel();
    let result_whip = whip_handle.await.unwrap();
    assert!(result_whip.is_ok());
}

/// Directly exercise `livetwo::probe::rsmpeg::RsmpegProbe` against an rsmpeg VP8 source.
#[cfg(feature = "rsmpeg")]
#[tokio::test]
async fn whep_probe_rsmpeg_vp8() {
    let source = source::rsmpeg_vp8::RsmpegVp8Source::default();
    let (_api_addr, port, source_handle, whip_ct, whip_handle) =
        start_published_stream(&source, IpAddr::V4(Ipv4Addr::LOCALHOST)).await;

    source.wait_for_ready().await;

    let whep_url = format!("http://127.0.0.1:{port}{}", api::path::whep("-"));
    let config = ProbeConfig {
        whep_url,
        timeout: Duration::from_secs(30),
        codec: Some(cli::Codec::Vp8),
        sprop_params: None,
        token: None,
    };

    let result = RsmpegProbe::default()
        .probe(&config)
        .await
        .expect("probe failed");
    tracing::info!(?result, "RsmpegProbe result");

    assert!(
        result.connected,
        "probe did not connect: {:?}",
        result.error
    );
    assert!(result.success, "probe did not succeed: {:?}", result.error);
    assert!(result.frame_count > 0, "probe decoded no frames");
    assert!(
        result.width > 0 && result.height > 0,
        "probe got no resolution"
    );

    source_handle.stop().await;
    whip_ct.cancel();
    let result_whip = whip_handle.await.unwrap();
    assert!(result_whip.is_ok());
}

/// `whipsynth` video-only sources verified in a real browser.
#[cfg(all(feature = "rsmpeg", feature = "whepwright"))]
#[test_matrix(
    [
        source::whipsynth::WhipgenSource::default(),
        source::whipsynth::WhipgenSource::new(VideoCodec::H264),
    ],
    [player::playwright::PlaywrightWhepPlayer::default()]
)]
#[tokio::test]
async fn whipsynth_video_playwright_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// `whipsynth` video-only sources verified with the in-process livetwo WHEP player.
#[cfg(feature = "rsmpeg")]
#[test_matrix(
    [
        source::whipsynth::WhipgenSource::default(),
        source::whipsynth::WhipgenSource::new(VideoCodec::H264),
        source::whipsynth::WhipgenSource::new(VideoCodec::H265),
        source::whipsynth::WhipgenSource::new(VideoCodec::Av1),
    ],
    [player::livetwo::LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whipsynth_video_livetwo_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// `whipsynth` audio+video sources verified in a real browser.
#[cfg(all(feature = "rsmpeg", feature = "whepwright"))]
#[test_matrix(
    [
        source::whipsynth::WhipgenSource::default(),
        source::whipsynth::WhipgenSource::new(VideoCodec::H264)
            .with_audio(source::whipsynth::WhipgenAudioCodec::Opus),
    ],
    [player::playwright::PlaywrightWhepPlayer::default()]
)]
#[tokio::test]
async fn whipsynth_audio_playwright_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// `whipsynth` audio+video sources verified with the in-process livetwo WHEP player.
#[cfg(feature = "rsmpeg")]
#[test_matrix(
    [
        source::whipsynth::WhipgenSource::default(),
        source::whipsynth::WhipgenSource::new(VideoCodec::Vp8)
            .with_audio(source::whipsynth::WhipgenAudioCodec::G722),
        source::whipsynth::WhipgenSource::new(VideoCodec::H264)
            .with_audio(source::whipsynth::WhipgenAudioCodec::Opus),
        source::whipsynth::WhipgenSource::new(VideoCodec::H265)
            .with_audio(source::whipsynth::WhipgenAudioCodec::Opus),
    ],
    [player::livetwo::LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whipsynth_audio_livetwo_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// `whipsynth` codec coverage using the in-process rsmpeg receiver.
#[cfg(feature = "rsmpeg")]
#[test_matrix(
    [source::whipsynth::WhipgenSource::default()],
    [player::rsmpeg_receiver::RsmpegWhepReceiver::default()]
)]
#[tokio::test]
async fn whipsynth_vp8_rsmpeg_receiver_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// `whipsynth` H265 coverage using the in-process rsmpeg receiver.
#[cfg(feature = "rsmpeg")]
#[tokio::test]
async fn whipsynth_h265_rsmpeg_receiver_test() {
    let source = source::whipsynth::WhipgenSource::new(VideoCodec::H265);
    let sprop = match source.sprop_params() {
        Some(s) => s,
        None => {
            tracing::warn!(
                "skipping H265 test: libx265 encoder not available for sprop extraction"
            );
            return;
        }
    };
    let player =
        player::rsmpeg_receiver::RsmpegWhepReceiver::with_codec_and_sprop(cli::Codec::H265, sprop);
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// Start liveion, create a stream, publish a source via WHIP, and wait for the
/// publish session to reach Connected.
///
/// Returns `(api_addr, http_port, source_handle, whip_cancellation_token, whip_join_handle)`.
async fn start_published_stream<S>(
    source: &S,
    bind_ip: IpAddr,
) -> (
    SocketAddr,
    u16,
    Box<dyn SourceHandle>,
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
)
where
    S: Source,
{
    init_liveion_test_environment();

    let mut cfg = liveion::config::Config::default();
    cfg.http.cors = true;

    let listener = TcpListener::bind(SocketAddr::new(bind_ip, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let api_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let res = reqwest::Client::new()
        .post(format!("http://{api_addr}{}", api::path::streams("-")))
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    let whip_url = format!("http://{api_addr}{}", api::path::whip("-"));

    if source.publishes_directly() {
        return start_direct_published_stream(source, api_addr, port, whip_url).await;
    }

    let whip_ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let whip_video_guard = reserve_udp_port(whip_ip);
    let whip_video_port = whip_video_guard.port();
    let whip_video_addr = SocketAddr::new(whip_ip, whip_video_port);
    let (whip_audio_guard, whip_audio_addr) = if source.has_audio() {
        let guard = reserve_udp_port(whip_ip);
        let port = guard.port();
        (Some(guard), Some(SocketAddr::new(whip_ip, port)))
    } else {
        (None, None)
    };

    // Write the SDP file that liveion will use to receive the source stream.
    let sdp = source.sdp_with_audio(whip_video_addr, whip_audio_addr);
    let _whip_sdp = tempfile::NamedTempFile::new().unwrap();
    let sdp_path = _whip_sdp.path().to_str().unwrap().to_string();
    {
        let mut file = std::fs::File::create(&sdp_path).unwrap();
        file.write_all(sdp.as_bytes()).unwrap();
    }

    // Release the temporary UDP sockets immediately before starting WHIP so the
    // ports are free for liveion to bind. The SDP already contains the selected
    // ports. This minimizes the TOCTOU window; fully eliminating it would
    // require passing pre-bound sockets into the WHIP input path.
    drop(whip_video_guard);
    if let Some(guard) = whip_audio_guard {
        drop(guard);
    }

    let ct = CancellationToken::new();
    let whip_ct = ct.clone();
    let mut handle_whip = tokio::spawn(async move {
        // Keep the temp SDP file alive for the lifetime of the WHIP task so the
        // runner cannot read a deleted path.
        let _whip_sdp = _whip_sdp;
        livetwo::whip::into(whip_ct, sdp_path, whip_url, None, None).await
    });

    wait_for_publish_connected(&api_addr, Some(&mut handle_whip)).await;

    // Start the media source only after the WHIP/RTP listener is bound so that
    // sources which open a connected UDP socket (e.g. the rsmpeg generator)
    // don't hit ICMP errors before the receiver is ready.
    let source_handle = source
        .start_with_audio(whip_video_addr, whip_audio_addr)
        .expect("Failed to start media source");

    (api_addr, port, source_handle, ct, handle_whip)
}

async fn start_direct_published_stream<S>(
    source: &S,
    api_addr: SocketAddr,
    port: u16,
    whip_url: String,
) -> (
    SocketAddr,
    u16,
    Box<dyn SourceHandle>,
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
)
where
    S: Source,
{
    let source_handle = source
        .start_direct(&whip_url)
        .expect("Failed to start direct WHIP source");

    wait_for_publish_connected(&api_addr, None).await;

    // The publisher is already running inside the source handle; return a
    // no-op WHIP handle so callers can keep the same shape.
    let ct = CancellationToken::new();
    let handle_whip = tokio::spawn(async move { Ok(()) });

    (api_addr, port, source_handle, ct, handle_whip)
}

async fn wait_for_publish_connected(
    api_addr: &SocketAddr,
    mut handle_whip: Option<&mut tokio::task::JoinHandle<anyhow::Result<()>>>,
) {
    let mut publish_connected = false;
    for _ in 0..300 {
        let res = reqwest::get(format!("http://{api_addr}{}", api::path::streams("")))
            .await
            .unwrap();
        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();
        if let Some(r) = body.into_iter().find(|i| i.id == "-")
            && !r.publish.sessions.is_empty()
        {
            let s = r.publish.sessions[0].clone();
            if s.state == api::response::RTCPeerConnectionState::Connected {
                publish_connected = true;
                break;
            }
        }

        if let Some(handle) = handle_whip.as_mut()
            && handle.is_finished()
        {
            let result_whip = handle.await.unwrap();
            panic!("WHIP task exited before publish connected: {result_whip:?}");
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(publish_connected, "Publish session did not reach Connected");
}

fn assert_playback_ok(player_name: &str, playback: &PlayResult) {
    assert!(
        playback.success,
        "{player_name} playback did not succeed: {:?}",
        playback.error
    );
    assert!(playback.connected, "{player_name} did not connect");
    assert!(
        playback.video_tracks + playback.audio_tracks > 0,
        "{player_name} reported no media tracks"
    );
    assert!(
        playback.duration_ms > 0,
        "{player_name} reported zero duration"
    );

    // Browser playback additionally checks rendered frame dimensions.
    if player_name.starts_with("playwright") {
        assert!(
            playback.video_width > 0 && playback.video_height > 0,
            "Browser did not render any video frames"
        );
    }
}
