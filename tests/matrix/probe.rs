//! Shared ffprobe invocation and stream validation for the matrix players.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::process::Command;

use crate::player::PlayResult;
use crate::profile::MediaProfile;

pub const FFPROBE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, serde::Deserialize)]
pub struct FfprobeStream {
    pub codec_type: String,
    pub codec_name: Option<String>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub channels: Option<u8>,
    /// Packet count ffprobe actually received (`-count_packets`); ffprobe
    /// reports it as a JSON string.
    pub nb_read_packets: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct Ffprobe {
    pub streams: Vec<FfprobeStream>,
}

/// Run ffprobe with the given input arguments and parse its JSON stream dump.
///
/// `input_args` selects the input (e.g. an SDP file, or an RTSP URL plus
/// transport flags). The child is killed on timeout so a stalled probe can
/// never leak a process.
#[cfg(feature = "rtsp")]
pub async fn run(input_args: &[&str]) -> Result<Ffprobe> {
    run_inner(input_args, false).await
}

/// Like [`run`], but also counts received packets per stream
/// (`nb_read_packets`). Bounds the read window with `-read_intervals`, since
/// counting packets on a live RTP stream (no EOF) would otherwise never
/// return. Needed for SDP-based players where a declared-but-silent track
/// must be distinguishable from a real one.
pub async fn run_counting(input_args: &[&str]) -> Result<Ffprobe> {
    run_inner(input_args, true).await
}

async fn run_inner(input_args: &[&str], count_packets: bool) -> Result<Ffprobe> {
    let mut ffprobe = Command::new("ffprobe");
    ffprobe.args(["-v", "error", "-hide_banner"]);
    if count_packets {
        // The window counts *stream* time, not wall time — a slow encoder
        // (e.g. debug libx264) stretches it, so keep it short and let
        // FFPROBE_TIMEOUT carry the slack.
        ffprobe.args(["-read_intervals", "%+3"]);
    }
    ffprobe
        .args(input_args)
        .args(["-show_streams", "-of", "json"]);
    if count_packets {
        ffprobe.arg("-count_packets");
    }
    ffprobe.kill_on_drop(true);

    let output = tokio::time::timeout(FFPROBE_TIMEOUT, ffprobe.output())
        .await
        .map_err(|_| anyhow!("ffprobe timed out after {FFPROBE_TIMEOUT:?}"))?
        .context("Failed to execute ffprobe")?;

    if !output.status.success() {
        anyhow::bail!(
            "ffprobe failed: stdout: {}\nstderr: {}",
            std::str::from_utf8(output.stdout.as_slice()).unwrap_or("<non-utf8>"),
            std::str::from_utf8(output.stderr.as_slice()).unwrap_or("<non-utf8>")
        );
    }

    let probe: Ffprobe = serde_json::from_slice(output.stdout.as_slice())
        .context("Failed to parse ffprobe JSON output")?;
    Ok(probe)
}

/// Build a [`PlayResult`] from a probe, validating that every track kind in
/// the profile is present in the probed streams.
///
/// With `require_packets`, each expected kind must also have produced at
/// least one received packet: ffprobe can report a stream from the SDP
/// declaration alone (especially audio) without a single RTP packet
/// arriving, so stream presence is not sufficient for SDP-based players.
/// RTSP pull verification passes `false` — a missing video stream already
/// fails the width/height assertions, and a fixed read window would only
/// add latency there.
pub fn into_play_result(
    probe: Ffprobe,
    profile: &MediaProfile,
    connected: bool,
    duration_ms: u64,
    require_packets: bool,
) -> PlayResult {
    let video_tracks = probe
        .streams
        .iter()
        .filter(|s| s.codec_type == "video")
        .count() as u32;
    let audio_tracks = probe
        .streams
        .iter()
        .filter(|s| s.codec_type == "audio")
        .count() as u32;

    let has_packets = |kind: &str| {
        probe.streams.iter().any(|s| {
            s.codec_type == kind
                && s.nb_read_packets
                    .as_deref()
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(0)
                    > 0
        })
    };

    let present = |kind: &str| {
        probe.streams.iter().any(|s| {
            if s.codec_type != kind {
                return false;
            }
            // Only the audio check may require packets: a silent video stream
            // is already caught by the width/height assertions, and on
            // dual-track RTP inputs (independent per-track RTP clocks, e.g.
            // gst-launch sources) the -read_intervals window can expire
            // against the audio clock and wrongly report zero video packets.
            kind != "audio" || !require_packets || has_packets(kind)
        })
    };

    let missing: Vec<&str> = [
        profile.video.is_some().then_some("video"),
        profile.audio.is_some().then_some("audio"),
    ]
    .into_iter()
    .flatten()
    .filter(|kind| !present(kind))
    .collect();

    let success = missing.is_empty();
    let error = if success {
        None
    } else if require_packets {
        Some(format!(
            "ffprobe received no {} packets in the WHEP output",
            missing.join("/")
        ))
    } else {
        Some(format!(
            "ffprobe found no {} stream(s) in the output",
            missing.join("/")
        ))
    };

    let video = probe.streams.iter().find(|s| s.codec_type == "video");
    let audio = probe.streams.iter().find(|s| s.codec_type == "audio");

    PlayResult {
        success,
        connected,
        error,
        video_width: video.and_then(|s| s.width).unwrap_or(0) as u32,
        video_height: video.and_then(|s| s.height).unwrap_or(0) as u32,
        video_tracks,
        audio_tracks,
        duration_ms,
        codecs: probe
            .streams
            .iter()
            .filter_map(|s| s.codec_name.clone())
            .collect(),
        audio_channels: audio.and_then(|s| s.channels).unwrap_or(0) as u32,
    }
}
