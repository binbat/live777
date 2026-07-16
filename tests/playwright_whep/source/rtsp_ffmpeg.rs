use std::{
    net::SocketAddr,
    process::{Child, Command},
};

use anyhow::{Context, Result};

use super::{Source, SourceHandle, VideoCodec};

/// RTSP source implemented by spawning an external FFmpeg process.
///
/// Unlike [`super::ffmpeg::FfmpegSource`] which pushes raw RTP to a
/// UDP socket and relies on WHIP for stream registration, this source
/// pushes directly to liveion's RTSP server (ANNOUNCE + RECORD).
#[derive(Debug, Clone, Copy)]
pub struct RtspFfmpegSource {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl RtspFfmpegSource {
    pub fn new(codec: VideoCodec) -> Self {
        Self {
            codec,
            width: 640,
            height: 480,
            fps: 30,
        }
    }
}

impl Source for RtspFfmpegSource {
    fn name(&self) -> &'static str {
        match self.codec {
            VideoCodec::Vp8 => "rtsp-ffmpeg-vp8",
            VideoCodec::H264 => "rtsp-ffmpeg-h264",
            VideoCodec::H265 => "rtsp-ffmpeg-h265",
            VideoCodec::Vp9 => "rtsp-ffmpeg-vp9",
            VideoCodec::Av1 => "rtsp-ffmpeg-av1",
        }
    }

    fn is_rtsp(&self) -> bool {
        true
    }

    fn start_rtsp(&self, rtsp_url: &str) -> Result<Box<dyn SourceHandle>> {
        let width = self.width;
        let height = self.height;
        let fps = self.fps;
        let codec = self.codec;
        let encoder = codec.ffmpeg_encoder();

        let mut cmd = Command::new("ffmpeg");
        cmd.arg("-re")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg(format!("testsrc=size={width}x{height}:rate={fps}"))
            .arg("-vcodec")
            .arg(encoder);
        for arg in codec.ffmpeg_extra_args() {
            cmd.arg(arg);
        }
        let keyframe_interval = fps.min(5);
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

        Ok(Box::new(RtspFfmpegHandle { child }))
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
    child: Child,
}

#[async_trait::async_trait]
impl SourceHandle for RtspFfmpegHandle {
    async fn stop(mut self: Box<Self>) {
        let _ = self.child.kill();
        let mut child = self.child;
        let _ = tokio::task::spawn_blocking(move || child.wait()).await;
    }
}
