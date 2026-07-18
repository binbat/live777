//! End-to-end WHEP test matrix: sources × media profiles × players.
//!
//! Every case publishes a source through liveion and validates playback with
//! one of the players. The media profile (codec combination) is declared once
//! per row; the runner validates codecs, dimensions and channels against it.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use test_case::test_matrix;

#[path = "../common.rs"]
mod common;
mod player;
mod profile;
mod runner;
mod source;

use player::{Player, livetwo::LivetwoWhepPlayer};
use profile::{AudioCodec, MediaProfile, VideoCodec};
use runner::run_whep_test_with_host;
use source::{Source, ffmpeg::FfmpegSource};

#[cfg(feature = "whepwright")]
use player::playwright::PlaywrightWhepPlayer;
#[cfg(feature = "rsmpeg")]
use player::rsmpeg_receiver::RsmpegWhepReceiver;
#[cfg(feature = "rtsp")]
use source::rtsp_ffmpeg::RtspFfmpegSource;
#[cfg(feature = "rsmpeg")]
use source::synth::SynthSource;

// ============================================================
// FFmpeg RTP sources (ffmpeg CLI → RTP → whipinto → liveion)
// ============================================================

/// Core matrix: every codec combination played back by the in-process
/// livetwo WHEP player with ffprobe validation. No browser required.
#[test_matrix(
    [
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::H264)),
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::H265)),
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp9)),
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::Av1)),
        FfmpegSource::new(MediaProfile::audio_only(AudioCodec::Opus)),
        FfmpegSource::new(MediaProfile::audio_only(AudioCodec::G722)),
        FfmpegSource::new(MediaProfile::av(VideoCodec::Vp8, AudioCodec::Opus)),
        FfmpegSource::new(MediaProfile::av(VideoCodec::H264, AudioCodec::Opus)),
        FfmpegSource::new(MediaProfile::av(VideoCodec::H264, AudioCodec::G722)),
    ],
    [LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whep_ffmpeg_livetwo_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// FFmpeg RTP sources played back in a real browser via Playwright.
/// Only browser-compatible combinations: VP8/H264 video, Opus audio.
#[cfg(feature = "whepwright")]
#[test_matrix(
    [
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::H264)),
        FfmpegSource::new(MediaProfile::av(VideoCodec::Vp8, AudioCodec::Opus)),
        FfmpegSource::new(MediaProfile::av(VideoCodec::H264, AudioCodec::Opus)),
    ],
    [PlaywrightWhepPlayer::default()]
)]
#[tokio::test]
async fn whep_ffmpeg_playwright_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// FFmpeg RTP sources decoded in-process with rsmpeg (no ICE/browser issues).
#[cfg(feature = "rsmpeg")]
#[test_matrix(
    [
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::H264)),
    ],
    [RsmpegWhepReceiver::default()]
)]
#[tokio::test]
async fn whep_ffmpeg_rsmpeg_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// Edge: IPv6 loopback for the WHIP publish and WHEP subscribe endpoints.
#[tokio::test]
async fn whep_ffmpeg_vp8_ipv6_test() {
    run_whep_test_with_host(
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        LivetwoWhepPlayer,
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        "[::1]",
    )
    .await;
}

/// Edge: 4K VP9 stress case.
#[tokio::test]
async fn whep_ffmpeg_vp9_4k_test() {
    run_whep_test_with_host(
        FfmpegSource::new(
            MediaProfile::video_only(VideoCodec::Vp9).with_video_spec(3840, 2160, 30),
        ),
        LivetwoWhepPlayer,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        "127.0.0.1",
    )
    .await;
}

// ============================================================
// Synthetic sources (livetwo::whipsynth → WHIP direct, rsmpeg feature)
// ============================================================

/// Synthetic sources validated by the livetwo WHEP player with ffprobe.
#[cfg(feature = "rsmpeg")]
#[test_matrix(
    [
        SynthSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        SynthSource::new(MediaProfile::video_only(VideoCodec::H264)),
        SynthSource::new(MediaProfile::video_only(VideoCodec::Av1)),
        SynthSource::new(MediaProfile::av(VideoCodec::Vp8, AudioCodec::Opus)),
        SynthSource::new(MediaProfile::av(VideoCodec::Vp8, AudioCodec::G722)),
        SynthSource::new(MediaProfile::av(VideoCodec::H264, AudioCodec::Opus)),
    ],
    [LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whep_synth_livetwo_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// Synthetic H265 sources validated by the livetwo WHEP player with ffprobe.
///
/// Excluded on Windows: the vcpkg FFmpeg's x265 produces a stream whose
/// parameter sets ffprobe and the rsmpeg receiver cannot decode ("Error
/// constructing the frame RPS"), although the packets arrive fine. The
/// ffmpeg-RTP H265 row passes on Windows, so this tracks the whipsynth
/// encoder/sprop layer specifically.
#[cfg(all(feature = "rsmpeg", not(target_os = "windows")))]
#[test_matrix(
    [
        SynthSource::new(MediaProfile::video_only(VideoCodec::H265)),
        SynthSource::new(MediaProfile::av(VideoCodec::H265, AudioCodec::Opus)),
    ],
    [LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whep_synth_h265_livetwo_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// Synthetic sources decoded in-process with rsmpeg (video tracks only —
/// the rsmpeg probe does not decode audio).
#[cfg(feature = "rsmpeg")]
#[test_matrix(
    [
        SynthSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        SynthSource::new(MediaProfile::video_only(VideoCodec::H264)),
    ],
    [RsmpegWhepReceiver::default()]
)]
#[tokio::test]
async fn whep_synth_rsmpeg_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// Synthetic sources played back in a real browser via Playwright.
#[cfg(all(feature = "rsmpeg", feature = "whepwright"))]
#[test_matrix(
    [
        SynthSource::new(MediaProfile::av(VideoCodec::Vp8, AudioCodec::Opus)),
        SynthSource::new(MediaProfile::video_only(VideoCodec::H264)),
        SynthSource::new(MediaProfile::av(VideoCodec::H264, AudioCodec::Opus)),
    ],
    [PlaywrightWhepPlayer::default()]
)]
#[tokio::test]
async fn whep_synth_playwright_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// H265 synthetic coverage using the in-process rsmpeg receiver; the H265
/// decoder needs the source's sprop parameters.
///
/// Excluded on Windows, same as the synth H265 livetwo rows above.
#[cfg(all(feature = "rsmpeg", not(target_os = "windows")))]
#[tokio::test]
async fn whep_synth_h265_rsmpeg_receiver_test() {
    let source = SynthSource::new(MediaProfile::video_only(VideoCodec::H265));
    let sprop = match source.sprop_params() {
        Some(s) => s,
        None => {
            tracing::warn!(
                "skipping H265 test: libx265 encoder not available for sprop extraction"
            );
            return;
        }
    };
    let player = RsmpegWhepReceiver::with_codec_and_sprop(cli::Codec::H265, sprop);
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

// ============================================================
// RTSP sources (ffmpeg → liveion RTSP server push)
// ============================================================

/// RTSP FFmpeg sources verified with the in-process livetwo WHEP player.
/// Covers the core RTSP push (ANNOUNCE + RECORD) → WHEP subscribe path.
#[cfg(feature = "rtsp")]
#[test_matrix(
    [
        RtspFfmpegSource::new(VideoCodec::Vp8),
        RtspFfmpegSource::new(VideoCodec::H264),
        RtspFfmpegSource::new(VideoCodec::H265),
    ],
    [LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whep_rtsp_livetwo_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// RTSP FFmpeg sources verified in a real browser via Playwright.
#[cfg(all(feature = "rtsp", feature = "whepwright"))]
#[test_matrix(
    [
        RtspFfmpegSource::new(VideoCodec::H264),
        RtspFfmpegSource::new(VideoCodec::Vp8),
    ],
    [PlaywrightWhepPlayer::default()]
)]
#[tokio::test]
async fn whep_rtsp_playwright_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// RTSP FFmpeg sources verified with the in-process rsmpeg receiver.
#[cfg(all(feature = "rtsp", feature = "rsmpeg"))]
#[test_matrix(
    [
        RtspFfmpegSource::new(VideoCodec::Vp8),
        RtspFfmpegSource::new(VideoCodec::H264),
    ],
    [RsmpegWhepReceiver::default()]
)]
#[tokio::test]
async fn whep_rtsp_rsmpeg_baseline_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}
