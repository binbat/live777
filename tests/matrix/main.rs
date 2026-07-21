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
mod probe;
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
#[cfg(not(target_os = "windows"))]
use player::{gst_rtp::GstRtpPlayer, gst_whep::GstWhepPlayer};
#[cfg(all(feature = "rtsp", not(target_os = "windows")))]
use runner::run_rtsp_roundtrip_gst;
#[cfg(feature = "rtsp")]
use runner::{RtspTransport, run_rtsp_cycle, run_rtsp_push_mediamtx, run_rtsp_roundtrip};
#[cfg(all(feature = "rtsp", not(target_os = "windows")))]
use source::gst_rtsp_server::GstRtspServerSource;
#[cfg(feature = "rtsp")]
use source::mediamtx::MediamtxPullSource;
#[cfg(feature = "rtsp")]
use source::rtsp_ffmpeg::RtspFfmpegSource;
#[cfg(feature = "rsmpeg")]
use source::synth::SynthSource;
#[cfg(not(target_os = "windows"))]
use source::{gst_rtp::GstRtpSource, gst_whip::GstWhipSource};

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
/// Disabled on Windows, matching the RTSP suites' long-standing policy
/// (Windows runners crawl on RTSP push/pull and the suites never ran there).
#[cfg(all(feature = "rtsp", not(target_os = "windows")))]
#[test_matrix(
    [
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::H264)),
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::H265)),
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
#[cfg(all(feature = "rtsp", feature = "whepwright", not(target_os = "windows")))]
#[test_matrix(
    [
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::H264)),
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
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
#[cfg(all(feature = "rtsp", feature = "rsmpeg", not(target_os = "windows")))]
#[test_matrix(
    [
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::H264)),
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

// ============================================================
// RTSP round-trip (ffmpeg → liveion RTSP server → ffprobe pull)
// ============================================================

/// Liveion RTSP server round-trip: the source pushes via ANNOUNCE/RECORD and
/// ffprobe pulls from liveion's own pull side. No WHIP/WHEP involved.
/// Replaces the former tests/rtsp.rs suite (16 cases).
/// Disabled on Windows, same as the old RTSP suites.
#[cfg(all(feature = "rtsp", not(target_os = "windows")))]
#[test_matrix(
    [
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::H264)),
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::H265)),
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp9)),
        RtspFfmpegSource::new(MediaProfile::audio_only(AudioCodec::Opus)),
        RtspFfmpegSource::new(MediaProfile::audio_only(AudioCodec::G722)),
        RtspFfmpegSource::new(MediaProfile::av(VideoCodec::Vp8, AudioCodec::Opus)),
    ],
    [RtspTransport::Udp, RtspTransport::Tcp]
)]
#[tokio::test]
async fn rtsp_roundtrip_matrix_test<S>(source: S, transport: RtspTransport)
where
    S: Source,
{
    run_rtsp_roundtrip(source, transport, IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
}

/// Edge: IPv6 loopback for the RTSP round-trip.
#[cfg(all(feature = "rtsp", not(target_os = "windows")))]
#[test_matrix(
    [RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8))],
    [RtspTransport::Udp, RtspTransport::Tcp]
)]
#[tokio::test]
async fn rtsp_roundtrip_ipv6_matrix_test<S>(source: S, transport: RtspTransport)
where
    S: Source,
{
    run_rtsp_roundtrip(source, transport, IpAddr::V6(Ipv6Addr::LOCALHOST)).await;
}

// ============================================================
// RTSP conversion cycle (former tests/rtsp2.rs)
// ffmpeg → liveion RTSP → whipinto → liveion WHIP → whepfrom → liveion RTSP → ffprobe
// ============================================================

/// Full conversion cycle: the source pushes into liveion's RTSP server,
/// whipinto bridges it to WHIP, whepfrom bridges it back to RTSP, and
/// ffprobe validates the final stream by pulling from liveion.
/// The transport variant applies to both livetwo client hops and the pull.
#[cfg(all(feature = "rtsp", not(target_os = "windows")))]
#[test_matrix(
    [
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::H264)),
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::H265)),
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp9)),
        RtspFfmpegSource::new(MediaProfile::audio_only(AudioCodec::Opus)),
        RtspFfmpegSource::new(MediaProfile::audio_only(AudioCodec::G722)),
        RtspFfmpegSource::new(MediaProfile::av(VideoCodec::Vp8, AudioCodec::Opus)),
    ],
    [RtspTransport::Udp, RtspTransport::Tcp]
)]
#[tokio::test]
async fn rtsp_cycle_matrix_test<S>(source: S, transport: RtspTransport)
where
    S: Source,
{
    run_rtsp_cycle(source, transport, IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
}

/// Edge: IPv6 loopback for the RTSP conversion cycle.
#[cfg(all(feature = "rtsp", not(target_os = "windows")))]
#[test_matrix(
    [RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8))],
    [RtspTransport::Udp, RtspTransport::Tcp]
)]
#[tokio::test]
async fn rtsp_cycle_ipv6_matrix_test<S>(source: S, transport: RtspTransport)
where
    S: Source,
{
    run_rtsp_cycle(source, transport, IpAddr::V6(Ipv6Addr::LOCALHOST)).await;
}

// ============================================================
// mediamtx interop (live777#212): second third-party RTSP server
// after gst-rtsp-server
// ============================================================

/// mediamtx pull interop: ffmpeg pushes into mediamtx, livetwo's RTSP
/// client pulls from mediamtx and publishes via WHIP, played back by the
/// livetwo WHEP player. Covers whipinto's RTSP client against mediamtx's
/// SDP dialect.
#[cfg(all(feature = "rtsp", not(target_os = "windows")))]
#[test_matrix(
    [
        MediamtxPullSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        MediamtxPullSource::new(MediaProfile::video_only(VideoCodec::H264)),
        MediamtxPullSource::new(MediaProfile::video_only(VideoCodec::H265)),
        MediamtxPullSource::new(MediaProfile::video_only(VideoCodec::Vp9)),
        MediamtxPullSource::new(MediaProfile::audio_only(AudioCodec::Opus)),
        MediamtxPullSource::new(MediaProfile::audio_only(AudioCodec::G722)),
        MediamtxPullSource::new(MediaProfile::av(VideoCodec::Vp8, AudioCodec::Opus)),
    ],
    [RtspTransport::Udp, RtspTransport::Tcp],
    [LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whep_mediamtx_pull_matrix_test<P>(
    source: MediamtxPullSource,
    transport: RtspTransport,
    player: P,
) where
    P: Player,
{
    if !source::mediamtx::available() {
        tracing::warn!("skipping: mediamtx not available on this host");
        return;
    }
    run_whep_test_with_host(
        source.with_transport(transport),
        player,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        "127.0.0.1",
    )
    .await;
}

/// mediamtx push interop: whepfrom bridges WHEP back to RTSP by pushing
/// into mediamtx; ffprobe validates by pulling from mediamtx. Covers
/// whepfrom's RTSP ANNOUNCE/RECORD against a third-party server.
#[cfg(all(feature = "rtsp", not(target_os = "windows")))]
#[test_matrix(
    [
        MediaProfile::video_only(VideoCodec::Vp8),
        MediaProfile::video_only(VideoCodec::H264),
        MediaProfile::video_only(VideoCodec::H265),
        MediaProfile::video_only(VideoCodec::Vp9),
        MediaProfile::audio_only(AudioCodec::Opus),
        MediaProfile::audio_only(AudioCodec::G722),
        MediaProfile::av(VideoCodec::Vp8, AudioCodec::Opus),
    ],
    [RtspTransport::Udp, RtspTransport::Tcp]
)]
#[tokio::test]
async fn rtsp_push_mediamtx_matrix_test(profile: MediaProfile, transport: RtspTransport) {
    if !source::mediamtx::available() {
        tracing::warn!("skipping: mediamtx not available on this host");
        return;
    }
    run_rtsp_push_mediamtx(profile, transport, IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
}

// ============================================================
// GStreamer sources and players
// ============================================================

/// GStreamer RTP sources (gst-launch → RTP → whipinto → liveion) played
/// back by the livetwo WHEP player with ffprobe validation.
#[cfg(not(target_os = "windows"))]
#[test_matrix(
    [
        GstRtpSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        GstRtpSource::new(MediaProfile::video_only(VideoCodec::H264)),
        GstRtpSource::new(MediaProfile::av(VideoCodec::Vp8, AudioCodec::Opus)),
    ],
    [LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whep_gst_rtp_matrix_test<P>(source: GstRtpSource, player: P)
where
    P: Player,
{
    if !runner::require_gst(&source.required_elements()) {
        tracing::warn!("skipping: GStreamer not available on this host");
        return;
    }
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// GStreamer whipsink sources (direct WHIP publish) played back by the
/// livetwo WHEP player.
///
/// Ignored: whipsink's WHIP session against live777 never finishes ICE in
/// the loopback-pinned test environment (stays "connecting", no codecs) —
/// the same gst-client interop family as live777#340. Enable when fixed.
#[cfg(not(target_os = "windows"))]
#[ignore = "whipsink cannot complete ICE against loopback-pinned live777 (live777#340 family)"]
#[test_matrix(
    [
        GstWhipSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        GstWhipSource::new(MediaProfile::video_only(VideoCodec::H264)),
    ],
    [LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whep_gst_whip_matrix_test<P>(source: GstWhipSource, player: P)
where
    P: Player,
{
    if !runner::require_gst(&source.required_elements()) {
        tracing::warn!("skipping: GStreamer whipsink not available on this host");
        return;
    }
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// GStreamer rtsp-server hosted source pulled by livetwo's RTSP client and
/// published via WHIP, played back by the livetwo WHEP player.
///
/// Covers video-only, audio-only and A/V profiles so the livetwo RTSP client
/// is exercised against gst-rtsp-server's SDP dialect for every codec the
/// ffmpeg RTSP suites use (except AV1: `av1enc` is not packaged widely
/// enough to run anywhere but a skip).
#[cfg(all(feature = "rtsp", not(target_os = "windows")))]
#[test_matrix(
    [
        GstRtspServerSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        GstRtspServerSource::new(MediaProfile::video_only(VideoCodec::H264)),
        GstRtspServerSource::new(MediaProfile::video_only(VideoCodec::H265)),
        GstRtspServerSource::new(MediaProfile::video_only(VideoCodec::Vp9)),
        GstRtspServerSource::new(MediaProfile::audio_only(AudioCodec::Opus)),
        GstRtspServerSource::new(MediaProfile::audio_only(AudioCodec::G722)),
        GstRtspServerSource::new(MediaProfile::av(VideoCodec::Vp8, AudioCodec::Opus)),
        GstRtspServerSource::new(MediaProfile::av(VideoCodec::H264, AudioCodec::Opus)),
    ],
    [LivetwoWhepPlayer]
)]
#[tokio::test]
async fn whep_gst_rtsp_matrix_test<P>(source: GstRtspServerSource, player: P)
where
    P: Player,
{
    if !GstRtspServerSource::available() || !runner::require_gst(&source.required_elements()) {
        tracing::warn!("skipping: gst-rtsp-server not available on this host");
        return;
    }
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// FFmpeg sources played back by the GStreamer WHEP player
/// (whepfrom → RTP → udpsrc → depay → dec → fakesink).
#[cfg(not(target_os = "windows"))]
#[test_matrix(
    [
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::H264)),
        FfmpegSource::new(MediaProfile::av(VideoCodec::Vp8, AudioCodec::Opus)),
    ],
    [GstRtpPlayer]
)]
#[tokio::test]
async fn whep_ffmpeg_gst_rtp_matrix_test<S, P>(source: S, player: P)
where
    S: Source,
    P: Player,
{
    if !runner::require_gst(&GstRtpPlayer::required_elements(&source.profile())) {
        tracing::warn!("skipping: GStreamer not available on this host");
        return;
    }
    run_whep_test_with_host(source, player, IpAddr::V4(Ipv4Addr::LOCALHOST), "127.0.0.1").await;
}

/// RTSP round-trip validated by a GStreamer rtspsrc pull from liveion
/// (gst as the RTSP consumer instead of ffprobe).
///
/// UDP only: gst rtspsrc over TCP fails against live777's RTSP server even
/// though ffprobe's TCP pull works — same interop family as the whepsrc
/// issue (live777#340). The ffprobe round-trip covers TCP.
#[cfg(all(feature = "rtsp", not(target_os = "windows")))]
#[test_matrix(
    [
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        RtspFfmpegSource::new(MediaProfile::video_only(VideoCodec::H264)),
        RtspFfmpegSource::new(MediaProfile::audio_only(AudioCodec::Opus)),
    ],
    [RtspTransport::Udp]
)]
#[tokio::test]
async fn rtsp_roundtrip_gst_matrix_test<S>(source: S, transport: RtspTransport)
where
    S: Source,
{
    if !runner::require_gst(&[
        "rtspsrc",
        "rtpjitterbuffer",
        "fakesink",
        "udpsink",
        "videotestsrc",
    ]) {
        tracing::warn!("skipping: GStreamer not available on this host");
        return;
    }
    run_rtsp_roundtrip_gst(source, transport, IpAddr::V4(Ipv4Addr::LOCALHOST)).await;
}

/// Placeholder for the gst whepsrc player: whepsrc+live777 has known issues
/// (live777#340); enable once fixed.
#[cfg(not(target_os = "windows"))]
#[ignore = "whepsrc + live777 known issues: live777#340"]
#[tokio::test]
async fn whep_ffmpeg_gst_whep_placeholder_test() {
    if !runner::require_gst(&GstWhepPlayer::required_elements(
        &MediaProfile::video_only(VideoCodec::Vp8),
    )) {
        tracing::warn!("skipping: GStreamer whepsrc not available on this host");
        return;
    }
    run_whep_test_with_host(
        FfmpegSource::new(MediaProfile::video_only(VideoCodec::Vp8)),
        GstWhepPlayer,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        "127.0.0.1",
    )
    .await;
}
