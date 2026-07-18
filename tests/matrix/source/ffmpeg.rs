use std::{
    net::SocketAddr,
    process::{Child, Command},
};

use anyhow::{Context, Result};

use super::{Source, SourceHandle};
use crate::profile::MediaProfile;

/// RTP source implemented by spawning an external FFmpeg process.
///
/// The media profile selects the codecs; audio (when present) is a `sine`
/// generator published to a second UDP port.
#[derive(Debug, Clone, Copy)]
pub struct FfmpegSource {
    pub profile: MediaProfile,
}

impl FfmpegSource {
    pub fn new(profile: MediaProfile) -> Self {
        Self { profile }
    }
}

impl Source for FfmpegSource {
    fn name(&self) -> String {
        format!("ffmpeg-{}", self.profile.name())
    }

    fn profile(&self) -> MediaProfile {
        self.profile
    }

    fn start(&self, target_addr: SocketAddr) -> Result<Box<dyn SourceHandle>> {
        self.start_with_audio(Some(target_addr), None)
    }

    fn start_with_audio(
        &self,
        video_addr: Option<SocketAddr>,
        audio_addr: Option<SocketAddr>,
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

        // Video RTP output. `?codec=` takes the RTP payload name; AV1/VP9 RTP
        // packetization is experimental, so their extra args pass `-strict
        // experimental` (without it ffmpeg refuses to write the header).
        if let Some(video) = self.profile.video {
            let video_addr = video_addr.context("video address required")?;
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
            // SVT-AV1 rejects -maxrate outside CRF mode ("Max Bitrate only
            // supported with CRF mode"), so rate control is codec-specific.
            match codec {
                crate::profile::VideoCodec::Av1 => {
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
            cmd.arg("-payload_type")
                .arg(codec.payload_type().to_string())
                .arg("-f")
                .arg("rtp")
                .arg(format!(
                    "rtp://{video_addr}?codec={}",
                    codec.rtp_payload_name()
                ));
        }

        // Audio RTP output.
        if let (Some(audio), Some(addr), Some(input)) =
            (self.profile.audio, audio_addr, audio_input_index)
        {
            cmd.arg("-map")
                .arg(format!("{input}:a"))
                .arg("-acodec")
                .arg(audio.ffmpeg_encoder());
            for arg in audio.ffmpeg_extra_args() {
                cmd.arg(arg);
            }
            cmd.arg("-payload_type")
                .arg(audio.payload_type().to_string())
                .arg("-f")
                .arg("rtp")
                .arg(format!("rtp://{addr}?codec={}", audio.rtp_payload_name()));
        }

        let child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn FFmpeg source: {cmd:?}"))?;

        Ok(Box::new(FfmpegHandle { child: Some(child) }))
    }

    fn sdp(&self, listen_addr: SocketAddr) -> String {
        self.sdp_with_audio(Some(listen_addr), None)
    }

    fn sdp_with_audio(
        &self,
        video_addr: Option<SocketAddr>,
        audio_addr: Option<SocketAddr>,
    ) -> String {
        let mut sdp = String::from(
            "v=0\r\n\
             o=- 0 0 IN IP4 127.0.0.1\r\n\
             s=ffmpeg test stream\r\n\
             c=IN IP4 127.0.0.1\r\n\
             t=0 0\r\n",
        );

        if let Some(video) = self.profile.video {
            let pt = video.codec.payload_type();
            let port = video_addr.expect("video address required").port();
            sdp.push_str(&format!(
                "m=video {port} RTP/AVP {pt}\r\n\
                 {}\r\n",
                video.codec.sdp_rtpmap(pt),
            ));
        }

        if let (Some(audio), Some(addr)) = (self.profile.audio, audio_addr) {
            let pt = audio.payload_type();
            sdp.push_str(&format!(
                "m=audio {} RTP/AVP {pt}\r\n\
                 {}\r\n",
                addr.port(),
                audio.sdp_rtpmap(pt),
            ));
        }

        sdp
    }
}

struct FfmpegHandle {
    child: Option<Child>,
}

// Tests that panic mid-case would otherwise leak the encoder process.
impl Drop for FfmpegHandle {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
    }
}

#[async_trait::async_trait]
impl SourceHandle for FfmpegHandle {
    async fn stop(mut self: Box<Self>) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = tokio::task::spawn_blocking(move || child.wait()).await;
        }
    }
}
