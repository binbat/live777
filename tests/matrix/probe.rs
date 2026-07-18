//! Shared ffprobe invocation and stream validation for the matrix players.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tokio::process::Command;

use crate::player::PlayResult;
use crate::profile::MediaProfile;

pub const FFPROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, serde::Deserialize)]
pub struct FfprobeStream {
    pub codec_type: String,
    pub codec_name: Option<String>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub channels: Option<u8>,
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
pub async fn run(input_args: &[&str]) -> Result<Ffprobe> {
    let mut ffprobe = Command::new("ffprobe");
    ffprobe
        .args(["-v", "error", "-hide_banner"])
        .args(input_args)
        .args(["-show_streams", "-of", "json"]);
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
pub fn into_play_result(
    probe: Ffprobe,
    profile: &MediaProfile,
    connected: bool,
    duration_ms: u64,
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

    let missing: Vec<&str> = [
        profile.video.is_some().then_some("video"),
        profile.audio.is_some().then_some("audio"),
    ]
    .into_iter()
    .flatten()
    .filter(|kind| !probe.streams.iter().any(|s| s.codec_type == *kind))
    .collect();

    let success = missing.is_empty();
    let error = if success {
        None
    } else {
        Some(format!(
            "ffprobe found no {} stream(s) in the WHEP output",
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
