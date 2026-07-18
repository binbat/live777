use std::{
    net::SocketAddr,
    process::{Child, Command},
};

use anyhow::{Context, Result};

use super::{Source, SourceHandle};
use crate::profile::{MediaProfile, VideoCodec};

/// RTSP source implemented by spawning an external FFmpeg process.
///
/// Unlike [`super::ffmpeg::FfmpegSource`] which pushes raw RTP to a
/// UDP socket and relies on WHIP for stream registration, this source
/// pushes directly to liveion's RTSP server (ANNOUNCE + RECORD).
#[derive(Debug, Clone, Copy)]
pub struct RtspFfmpegSource {
    pub profile: MediaProfile,
}

impl RtspFfmpegSource {
    pub fn new(codec: VideoCodec) -> Self {
        Self {
            profile: MediaProfile::video_only(codec),
        }
    }
}

impl Source for RtspFfmpegSource {
    fn name(&self) -> String {
        format!("rtsp-ffmpeg-{}", self.profile.name())
    }

    fn profile(&self) -> MediaProfile {
        self.profile
    }

    fn is_rtsp(&self) -> bool {
        true
    }

    fn start_rtsp(&self, rtsp_url: &str) -> Result<Box<dyn SourceHandle>> {
        let Some(video) = self.profile.video else {
            anyhow::bail!("RtspFfmpegSource requires a video track in its media profile");
        };
        let codec = video.codec;
        let encoder = codec.ffmpeg_encoder();

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-re").arg("-f").arg("lavfi").arg("-i").arg(format!(
            "testsrc=size={}x{}:rate={}",
            video.width, video.height, video.fps
        ));
        cmd.arg("-vcodec").arg(encoder);
        for arg in codec.ffmpeg_extra_args() {
            cmd.arg(arg);
        }
        let keyframe_interval = video.fps.min(5);
        cmd.arg("-g")
            .arg(keyframe_interval.to_string())
            .arg("-keyint_min")
            .arg(keyframe_interval.to_string())
            .arg("-b:v")
            .arg("1000k")
            .arg("-maxrate")
            .arg("1200k")
            .arg("-bufsize")
            .arg("2400k")
            // Use TCP transport — avoids UDP port conflicts in test
            // environments and ensures the RTP stream is interleaved
            // inside the RTSP connection.
            .arg("-rtsp_transport")
            .arg("tcp")
            .arg("-f")
            .arg("rtsp")
            .arg(rtsp_url);

        let child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn RTSP FFmpeg source: {cmd:?}"))?;

        Ok(Box::new(RtspFfmpegHandle { child: Some(child) }))
    }

    fn start(&self, _target_addr: SocketAddr) -> Result<Box<dyn SourceHandle>> {
        anyhow::bail!("RtspFfmpegSource must be used via start_rtsp()")
    }

    fn sdp(&self, _listen_addr: SocketAddr) -> String {
        // RTSP sources don't need an SDP file — the RTSP ANNOUNCE
        // carries the stream description.
        String::new()
    }

    /// RTSP sources can carry their own keyframes — shorter warm-up.
    async fn wait_for_ready(&self) {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}

struct RtspFfmpegHandle {
    child: Option<Child>,
}

// Tests that panic mid-case would otherwise leak the encoder process.
impl Drop for RtspFfmpegHandle {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
    }
}

#[async_trait::async_trait]
impl SourceHandle for RtspFfmpegHandle {
    async fn stop(mut self: Box<Self>) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = tokio::task::spawn_blocking(move || child.wait()).await;
        }
    }
}
