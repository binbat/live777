use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::{PlayResult, Player};
use crate::profile::MediaProfile;
use crate::runner::alloc_udp_ports;

/// WHEP player that uses `livetwo::whep::from` (the `whepfrom` conversion
/// path) and validates the RTP output with `ffprobe`: the streams ffprobe
/// can identify from the output SDP — codec, resolution, channels — must
/// match the source's media profile.
#[derive(Debug, Clone, Copy, Default)]
pub struct LivetwoWhepPlayer;

const FFPROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[async_trait]
impl Player for LivetwoWhepPlayer {
    fn name(&self) -> &'static str {
        "livetwo"
    }

    async fn play(&self, whep_url: &str, profile: &MediaProfile) -> Result<PlayResult> {
        let (base_url, stream_id) = parse_whep_url(whep_url)?;
        let whep_url = whep_url.to_string();

        let ip = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

        // Allocate output ports in pairs (RTP + RTCP are consecutive).
        let video_port = profile.video.map(|_| alloc_udp_ports(ip, 2));
        let audio_port = profile.audio.map(|_| alloc_udp_ports(ip, 2));

        let output_url = match (video_port, audio_port) {
            (Some(video), Some(audio)) => {
                format!("rtp://127.0.0.1?video={video}&audio={audio}")
            }
            (Some(video), None) => format!("rtp://127.0.0.1?video={video}"),
            (None, Some(audio)) => format!("rtp://127.0.0.1?audio={audio}"),
            (None, None) => anyhow::bail!("media profile has no tracks"),
        };

        let output_sdp =
            tempfile::NamedTempFile::new().context("Failed to create output SDP temp file")?;
        let output_sdp_path = output_sdp
            .path()
            .to_str()
            .ok_or_else(|| anyhow!("Invalid output SDP path"))?
            .to_string();

        let ct = CancellationToken::new();
        let mut handle_whep = Some(tokio::spawn({
            let ct = ct.clone();
            let output_sdp_path = output_sdp_path.clone();
            async move {
                // Keep the output SDP file alive for the lifetime of the WHEP task.
                let _output_sdp = output_sdp;
                livetwo::whep::from(
                    ct,
                    output_url,
                    whep_url.to_string(),
                    Some(output_sdp_path),
                    None,
                    None,
                    None,
                )
                .await
            }
        }));

        let start = tokio::time::Instant::now();
        let mut connected = false;
        let mut last_error = None;

        for _ in 0..300 {
            let res = reqwest::get(format!("{base_url}{}", api::path::streams("")))
                .await
                .context("Failed to query liveion streams")?;

            if res.status() != http::StatusCode::OK {
                last_error = Some(format!("liveion returned {}", res.status()));
                break;
            }

            let body = res.json::<Vec<api::response::Stream>>().await?;
            if let Some(stream) = body.into_iter().find(|s| s.id == stream_id)
                && stream
                    .subscribe
                    .sessions
                    .iter()
                    .any(|s| s.state == api::response::RTCPeerConnectionState::Connected)
            {
                connected = true;
                break;
            }

            if let Some(handle) = handle_whep.as_ref()
                && handle.is_finished()
            {
                match handle_whep.take().unwrap().await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => last_error = Some(format!("{e:?}")),
                    Err(e) => last_error = Some(format!("{e:?}")),
                }
                break;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        if !connected {
            ct.cancel();
            if let Some(handle) = handle_whep.take() {
                let _ = handle.await;
            }
            return Ok(PlayResult {
                success: false,
                connected: false,
                duration_ms: start.elapsed().as_millis() as u64,
                error: last_error.or_else(|| Some("subscribe did not connect".to_string())),
                ..Default::default()
            });
        }

        // ffprobe validates the media: it binds the output ports, receives the
        // forwarded RTP and reports the streams it can identify.
        let probe = match probe_output_sdp(&output_sdp_path).await {
            Ok(probe) => probe,
            Err(e) => {
                ct.cancel();
                if let Some(handle) = handle_whep.take() {
                    let _ = handle.await;
                }
                return Ok(PlayResult {
                    success: false,
                    connected: true,
                    duration_ms: start.elapsed().as_millis() as u64,
                    error: Some(format!("ffprobe failed: {e:?}")),
                    ..Default::default()
                });
            }
        };

        ct.cancel();
        if let Some(handle) = handle_whep.take() {
            let _ = handle.await;
        }

        let duration_ms = start.elapsed().as_millis() as u64;
        let video = probe.streams.iter().find(|s| s.codec_type == "video");
        let audio = probe.streams.iter().find(|s| s.codec_type == "audio");

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
        .filter(|kind| {
            let expected = *kind;
            !probe.streams.iter().any(|s| s.codec_type == expected)
        })
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

        Ok(PlayResult {
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
        })
    }
}

#[derive(Debug, serde::Deserialize)]
struct FfprobeStream {
    codec_type: String,
    codec_name: Option<String>,
    width: Option<u16>,
    height: Option<u16>,
    channels: Option<u8>,
}

#[derive(Debug, serde::Deserialize)]
struct Ffprobe {
    streams: Vec<FfprobeStream>,
}

/// Wait for the output SDP to be populated, then probe it with ffprobe.
async fn probe_output_sdp(sdp_path: &str) -> Result<Ffprobe> {
    for _ in 0..300 {
        if let Ok(contents) = std::fs::read_to_string(sdp_path)
            && contents.contains("m=")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let mut ffprobe = Command::new("ffprobe");
    ffprobe.args([
        "-v",
        "error",
        "-hide_banner",
        "-protocol_whitelist",
        "file,rtp,udp",
        "-i",
        sdp_path,
        "-show_streams",
        "-of",
        "json",
    ]);
    // Kill the child on timeout; otherwise a no-media probe would leak an
    // ffprobe process per failed case.
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

fn parse_whep_url(whep_url: &str) -> Result<(String, String)> {
    // Expected form: http://host:port/whep/<stream>
    let parsed = url::Url::parse(whep_url).context("Invalid WHEP URL")?;
    // `url::Host` renders IPv6 addresses with the required brackets.
    let host = parsed.host().ok_or_else(|| anyhow!("Missing host"))?;
    let base = format!("{}://{}", parsed.scheme(), host);
    let base = if let Some(port) = parsed.port() {
        format!("{base}:{port}")
    } else {
        base
    };

    let path = parsed.path();
    let stream_id = path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("Failed to parse stream id from WHEP URL"))?
        .to_string();

    Ok((base, stream_id))
}
