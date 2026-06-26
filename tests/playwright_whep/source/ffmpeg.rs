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
        let extra = codec.ffmpeg_extra_args().join(" ");

        // FFmpeg's RTP muxer names VP9 `VP9` and AV1 `AV1X` in the SDP.
        let rtp_codec_name = match codec {
            VideoCodec::Av1 => "av1x",
            _ => encoder,
        };

        let command = format!(
            "ffmpeg -re -f lavfi -i testsrc=size={width}x{height}:rate={fps} \
             -vcodec {encoder} {extra} \
             -g {fps} -keyint_min {fps} \
             -b:v 1000k -maxrate 1200k -bufsize 2400k \
             -payload_type {payload_type} \
             -f rtp 'rtp://{target_addr}?codec={rtp_codec_name}'"
        );

        let child = spawn_shell_command(&command)
            .with_context(|| format!("Failed to spawn FFmpeg source: {command}"))?;

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

impl SourceHandle for FfmpegHandle {
    fn stop(mut self: Box<Self>) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "windows")]
fn spawn_shell_command(command: &str) -> Result<Child> {
    Command::new("cmd")
        .args(["/C", command])
        .spawn()
        .context("Failed to spawn cmd")
}

#[cfg(not(target_os = "windows"))]
fn spawn_shell_command(command: &str) -> Result<Child> {
    Command::new("sh")
        .args(["-c", command])
        .spawn()
        .context("Failed to spawn sh")
}
