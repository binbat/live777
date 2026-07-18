use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tokio::process::Command;

use super::{PlayResult, Player};
use crate::profile::{MediaProfile, VideoCodec, VideoSpec};

/// WHEP player driven by GStreamer's `whepsrc` element (gst-plugins-rs
/// webrtchttp) receiving directly from the WHIP/WHEP endpoint — no whepfrom
/// in the loop.
///
/// Currently exercised only as a placeholder: whepsrc+live777 has known
/// issues (live777#340), so the matrix row is `#[ignore]`d until fixed.
#[derive(Debug, Clone, Copy, Default)]
pub struct GstWhepPlayer;

const GST_TIMEOUT: Duration = Duration::from_secs(30);
const VIDEO_BUFFERS: u32 = 60;

fn decode_chain(codec: VideoCodec) -> (&'static str, &'static str) {
    match codec {
        VideoCodec::Vp8 => ("rtpvp8depay", "vp8dec"),
        VideoCodec::H264 => ("rtph264depay", "avdec_h264"),
        VideoCodec::H265 => ("rtph265depay", "avdec_h265"),
        VideoCodec::Vp9 => ("rtpvp9depay", "vp9dec"),
        VideoCodec::Av1 => ("rtpav1depay", "avdec_av1"),
    }
}

impl GstWhepPlayer {
    pub fn required_elements(profile: &MediaProfile) -> Vec<&'static str> {
        let mut elements = vec!["whepsrc", "fakesink", "rtpjitterbuffer"];
        if let Some(video) = profile.video {
            let (depay, dec) = decode_chain(video.codec);
            elements.extend([depay, dec, "videoconvert"]);
        }
        elements
    }

    fn pipeline(whep_url: &str, spec: &VideoSpec) -> String {
        let (depay, dec) = decode_chain(spec.codec);
        // whepsrc outputs RTP; explicit caps keep the decoder chain simple.
        format!(
            "whepsrc whep-endpoint={whep_url} video-caps=application/x-rtp,payload={},encoding-name={},media=video,clock-rate=90000 ! rtpjitterbuffer ! {depay} ! {dec} ! videoconvert ! video/x-raw,width={},height={} ! fakesink num-buffers={VIDEO_BUFFERS}",
            spec.codec.payload_type(),
            spec.codec.rtp_payload_name(),
            spec.width,
            spec.height,
        )
    }
}

#[async_trait]
impl Player for GstWhepPlayer {
    fn name(&self) -> &'static str {
        "gst-whep"
    }

    async fn play(&self, whep_url: &str, profile: &MediaProfile) -> Result<PlayResult> {
        let Some(video) = profile.video else {
            anyhow::bail!("GstWhepPlayer requires a video track");
        };

        let start = tokio::time::Instant::now();
        let pipeline = Self::pipeline(whep_url, &video);
        let mut child = Command::new("gst-launch-1.0")
            .arg("-q")
            .args(pipeline.split_whitespace())
            .kill_on_drop(true)
            .spawn()?;

        let gst_result = tokio::time::timeout(GST_TIMEOUT, child.wait()).await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match gst_result {
            Ok(Ok(status)) if status.success() => Ok(PlayResult {
                success: true,
                connected: true,
                error: None,
                video_width: video.width,
                video_height: video.height,
                video_tracks: 1,
                audio_tracks: 0,
                duration_ms,
                codecs: vec![video.codec.ffprobe_name().to_string()],
                ..Default::default()
            }),
            Ok(Ok(status)) => Ok(PlayResult {
                success: false,
                connected: false,
                duration_ms,
                error: Some(format!("gst-launch exited with {status}")),
                ..Default::default()
            }),
            Ok(Err(e)) => Ok(PlayResult {
                success: false,
                connected: false,
                duration_ms,
                error: Some(format!("gst-launch wait failed: {e}")),
                ..Default::default()
            }),
            Err(_) => Ok(PlayResult {
                success: false,
                connected: false,
                duration_ms,
                error: Some(format!("gst-whep timed out after {GST_TIMEOUT:?}")),
                ..Default::default()
            }),
        }
    }
}
