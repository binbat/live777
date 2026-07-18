use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::{PlayResult, Player, parse_whep_url, wait_subscribe_connected};
use crate::probe;
use crate::profile::MediaProfile;
use crate::runner::alloc_udp_ports;

/// WHEP player that uses `livetwo::whep::from` (the `whepfrom` conversion
/// path) and validates the RTP output with `ffprobe`: the streams ffprobe
/// can identify from the output SDP — codec, resolution, channels — must
/// match the source's media profile.
#[derive(Debug, Clone, Copy, Default)]
pub struct LivetwoWhepPlayer;

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
        let (connected, last_error) =
            wait_subscribe_connected(&base_url, &stream_id, &mut handle_whep).await;

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
        let probe_result = match probe_output_sdp(&output_sdp_path).await {
            Ok(probe_result) => probe_result,
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
        Ok(probe::into_play_result(
            probe_result,
            profile,
            connected,
            duration_ms,
        ))
    }
}

/// Wait for the output SDP to be populated, then probe it with ffprobe.
async fn probe_output_sdp(sdp_path: &str) -> Result<probe::Ffprobe> {
    for _ in 0..300 {
        if let Ok(contents) = std::fs::read_to_string(sdp_path)
            && contents.contains("m=")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    probe::run(&["-protocol_whitelist", "file,rtp,udp", "-i", sdp_path]).await
}
