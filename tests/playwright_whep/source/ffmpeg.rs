use std::{
    net::SocketAddr,
    process::{Child, Command},
};

use anyhow::{Context, Result};

use super::{Source, SourceHandle, VideoCodec};

/// RTP source implemented by spawning an external FFmpeg process.
///
/// Supports multiple video codecs via [`VideoCodec`].
#[derive(Debug, Clone, Copy)]
pub struct FfmpegSource {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl FfmpegSource {
    pub fn new(codec: VideoCodec) -> Self {
        Self {
            codec,
            width: 640,
            height: 480,
            fps: 30,
        }
    }
}

impl Source for FfmpegSource {
    fn name(&self) -> &'static str {
        match self.codec {
            VideoCodec::Vp8 => "ffmpeg-vp8",
            VideoCodec::H264 => "ffmpeg-h264",
            VideoCodec::H265 => "ffmpeg-h265",
            VideoCodec::Vp9 => "ffmpeg-vp9",
            VideoCodec::Av1 => "ffmpeg-av1",
        }
    }

    fn start(&self, target_addr: SocketAddr) -> Result<Box<dyn SourceHandle>> {
        let width = self.width;
        let height = self.height;
        let fps = self.fps;
        let codec = self.codec;
        let payload_type = codec.payload_type();
        let encoder = codec.ffmpeg_encoder();

        // FFmpeg's RTP muxer names VP9 `VP9` and AV1 `AV1X` in the SDP.
        let rtp_codec_name = match codec {
            VideoCodec::Av1 => "av1x",
            _ => encoder,
        };

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
        // Use a short GOP so subscribers that connect after the first keyframe
        // get another keyframe quickly, even when the encoder is under load.
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
            .arg("-payload_type")
            .arg(payload_type.to_string())
            .arg("-f")
            .arg("rtp")
            .arg(format!("rtp://{target_addr}?codec={rtp_codec_name}"));

        let child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn FFmpeg source: {cmd:?}"))?;

        Ok(Box::new(FfmpegHandle { child }))
    }

    fn sdp(&self, listen_addr: SocketAddr) -> String {
        let pt = self.codec.payload_type();
        format!(
            "v=0\r\n\
             o=- 0 0 IN IP4 127.0.0.1\r\n\
             s=ffmpeg {} test stream\r\n\
             c=IN IP4 127.0.0.1\r\n\
             t=0 0\r\n\
             m=video {} RTP/AVP {pt}\r\n\
             {}\r\n",
            self.codec.as_str(),
            listen_addr.port(),
            self.codec.sdp_rtpmap(pt),
        )
    }
}

struct FfmpegHandle {
    child: Child,
}

#[async_trait::async_trait]
impl SourceHandle for FfmpegHandle {
    async fn stop(mut self: Box<Self>) {
        let _ = self.child.kill();
        let mut child = self.child;
        let _ = tokio::task::spawn_blocking(move || child.wait()).await;
    }
}
