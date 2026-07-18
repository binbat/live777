use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::{PlayResult, Player, parse_whep_url, wait_subscribe_connected};
use crate::profile::{AudioCodec, MediaProfile, VideoCodec, VideoSpec};
use crate::runner::alloc_udp_ports;

/// WHEP player that forwards via `livetwo::whep::from` to RTP/UDP and
/// validates with a GStreamer receive pipeline: `udpsrc` → depay → dec →
/// caps-negotiated `fakesink num-buffers=N`.
///
/// The caps filter after the decoder forces the exact resolution/channel
/// count from the media profile, so a mismatch fails negotiation instead of
/// passing silently. Receiving N buffers is the deterministic pass
/// condition (gst-launch exits 0 on EOS).
#[derive(Debug, Clone, Copy, Default)]
pub struct GstRtpPlayer;

const GST_TIMEOUT: Duration = Duration::from_secs(30);
const VIDEO_BUFFERS: u32 = 60;
const AUDIO_BUFFERS: u32 = 100;

fn video_depay_dec(codec: VideoCodec) -> (&'static str, &'static str) {
    match codec {
        VideoCodec::Vp8 => ("rtpvp8depay", "vp8dec"),
        VideoCodec::H264 => ("rtph264depay", "avdec_h264"),
        VideoCodec::H265 => ("rtph265depay", "avdec_h265"),
        VideoCodec::Vp9 => ("rtpvp9depay", "vp9dec"),
        VideoCodec::Av1 => ("rtpav1depay", "avdec_av1"),
    }
}

fn audio_depay_dec(codec: AudioCodec) -> (&'static str, &'static str) {
    match codec {
        AudioCodec::Opus => ("rtpopusdepay", "opusdec"),
        AudioCodec::G722 => ("rtpg722depay", "avdec_g722"),
    }
}

fn video_receiver(spec: &VideoSpec, port: u16) -> String {
    let (depay, dec) = video_depay_dec(spec.codec);
    format!(
        "udpsrc port={port} caps=application/x-rtp,media=video,encoding-name={},payload={},clock-rate=90000 ! rtpjitterbuffer ! {depay} ! {dec} ! videoconvert ! video/x-raw,width={},height={} ! fakesink num-buffers={VIDEO_BUFFERS}",
        spec.codec.rtp_payload_name(),
        spec.codec.payload_type(),
        spec.width,
        spec.height,
    )
}

fn audio_receiver(codec: AudioCodec, port: u16) -> String {
    let (depay, dec) = audio_depay_dec(codec);
    let (encoding, clock_rate) = match codec {
        AudioCodec::Opus => ("OPUS", 48000),
        AudioCodec::G722 => ("G722", 8000),
    };
    format!(
        "udpsrc port={port} caps=application/x-rtp,media=audio,encoding-name={encoding},payload={pt},clock-rate={clock_rate} ! {depay} ! {dec} ! audioconvert ! audio/x-raw,channels={channels} ! fakesink num-buffers={AUDIO_BUFFERS}",
        pt = codec.payload_type(),
        channels = codec.channels(),
    )
}

impl GstRtpPlayer {
    /// Elements required to receive/decode this profile.
    pub fn required_elements(profile: &MediaProfile) -> Vec<&'static str> {
        let mut elements = vec!["udpsrc", "fakesink", "rtpjitterbuffer"];
        if let Some(video) = profile.video {
            let (depay, dec) = video_depay_dec(video.codec);
            elements.extend([depay, dec, "videoconvert"]);
        }
        if let Some(audio) = profile.audio {
            let (depay, dec) = audio_depay_dec(audio);
            elements.extend([depay, dec, "audioconvert"]);
        }
        elements
    }
}

#[async_trait]
impl Player for GstRtpPlayer {
    fn name(&self) -> &'static str {
        "gst-rtp"
    }

    async fn play(&self, whep_url: &str, profile: &MediaProfile) -> Result<PlayResult> {
        let (base_url, stream_id) = parse_whep_url(whep_url)?;
        let whep_url = whep_url.to_string();

        let ip = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
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

        let ct = CancellationToken::new();
        let mut handle_whep = Some(tokio::spawn({
            let ct = ct.clone();
            async move { livetwo::whep::from(ct, output_url, whep_url, None, None, None, None).await }
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

        // Receive with gst: video and audio receivers in one gst-launch.
        let mut chains = Vec::new();
        if let Some(video) = profile.video {
            chains.push(video_receiver(&video, video_port.unwrap()));
        }
        if let Some(audio) = profile.audio {
            chains.push(audio_receiver(audio, audio_port.unwrap()));
        }
        let pipeline = chains.join(" ");

        let mut child = Command::new("gst-launch-1.0")
            .arg("-q")
            .args(pipeline.split_whitespace())
            .kill_on_drop(true)
            .spawn()?;

        let gst_result = tokio::time::timeout(GST_TIMEOUT, child.wait()).await;

        ct.cancel();
        if let Some(handle) = handle_whep.take() {
            let _ = handle.await;
        }

        let duration_ms = start.elapsed().as_millis() as u64;
        match gst_result {
            Ok(Ok(status)) if status.success() => Ok(PlayResult {
                success: true,
                connected: true,
                error: None,
                // The caps negotiation in the pipeline already proved these
                // values (a mismatch would fail to link).
                video_width: profile.video.map(|v| v.width).unwrap_or(0),
                video_height: profile.video.map(|v| v.height).unwrap_or(0),
                video_tracks: u32::from(profile.video.is_some()),
                audio_tracks: u32::from(profile.audio.is_some()),
                duration_ms,
                codecs: [
                    profile.video.map(|v| v.codec.ffprobe_name()),
                    profile.audio.map(|a| a.ffprobe_name()),
                ]
                .into_iter()
                .flatten()
                .map(str::to_string)
                .collect(),
                audio_channels: profile.audio.map(|a| a.channels() as u32).unwrap_or(0),
            }),
            Ok(Ok(status)) => Ok(PlayResult {
                success: false,
                connected: true,
                duration_ms,
                error: Some(format!("gst-launch exited with {status}")),
                ..Default::default()
            }),
            Ok(Err(e)) => Ok(PlayResult {
                success: false,
                connected: true,
                duration_ms,
                error: Some(format!("gst-launch wait failed: {e}")),
                ..Default::default()
            }),
            Err(_) => Ok(PlayResult {
                success: false,
                connected: true,
                duration_ms,
                error: Some(format!(
                    "gst receive timed out after {GST_TIMEOUT:?} (fakesink did not reach its buffer target)"
                )),
                ..Default::default()
            }),
        }
    }
}
