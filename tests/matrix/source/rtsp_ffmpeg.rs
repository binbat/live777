use std::{net::SocketAddr, process::Command};

use anyhow::{Context, Result};

use super::{ProcessHandle, Source, SourceHandle};
use crate::profile::{MediaProfile, VideoCodec};
use crate::runner::RtspTransport;

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
    pub fn new(profile: MediaProfile) -> Self {
        Self { profile }
    }

    /// Push to `rtsp_url` with an explicit RTSP transport. The default
    /// [`Source::start_rtsp`] uses TCP.
    pub fn start_rtsp_with_transport(
        &self,
        rtsp_url: &str,
        transport: RtspTransport,
    ) -> Result<Box<dyn SourceHandle>> {
        let mut cmd = Command::new("ffmpeg");

        // Input 0: synthetic video (when the profile has one).
        if let Some(video) = self.profile.video {
            cmd.arg("-re").arg("-f").arg("lavfi").arg("-i").arg(format!(
                "testsrc=size={}x{}:rate={}",
                video.width, video.height, video.fps
            ));
        }

        // Next input: synthetic audio (when the profile has one).
        let audio_input_index = if self.profile.audio.is_some() {
            let index = u8::from(self.profile.video.is_some());
            cmd.arg("-re")
                .arg("-f")
                .arg("lavfi")
                .arg("-i")
                .arg("sine=frequency=1000");
            Some(index)
        } else {
            None
        };

        if let Some(video) = self.profile.video {
            let codec = video.codec;
            cmd.arg("-map")
                .arg("0:v")
                .arg("-vcodec")
                .arg(codec.ffmpeg_encoder());
            for arg in codec.ffmpeg_extra_args() {
                cmd.arg(arg);
            }
            // Use a short GOP so subscribers that connect after the first
            // keyframe get another keyframe quickly, even under load.
            let keyframe_interval = video.fps.min(5);
            cmd.arg("-g")
                .arg(keyframe_interval.to_string())
                .arg("-keyint_min")
                .arg(keyframe_interval.to_string());
            // SVT-AV1 rejects -maxrate outside CRF mode.
            match codec {
                VideoCodec::Av1 => {
                    cmd.arg("-crf").arg("35");
                }
                _ => {
                    cmd.arg("-b:v")
                        .arg("1000k")
                        .arg("-maxrate")
                        .arg("1200k")
                        .arg("-bufsize")
                        .arg("2400k");
                }
            }
        }

        if let (Some(audio), Some(input)) = (self.profile.audio, audio_input_index) {
            cmd.arg("-map")
                .arg(format!("{input}:a"))
                .arg("-acodec")
                .arg(audio.ffmpeg_encoder());
            for arg in audio.ffmpeg_extra_args() {
                cmd.arg(arg);
            }
        }

        for arg in transport.ffmpeg_args() {
            cmd.arg(arg);
        }
        cmd.arg("-f").arg("rtsp").arg(rtsp_url);

        let child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn RTSP FFmpeg source: {cmd:?}"))?;

        Ok(Box::new(ProcessHandle::new(child)))
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
        // Use TCP transport for the push — avoids UDP port conflicts in test
        // environments and ensures the RTP stream is interleaved inside the
        // RTSP connection.
        self.start_rtsp_with_transport(rtsp_url, RtspTransport::Tcp)
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
