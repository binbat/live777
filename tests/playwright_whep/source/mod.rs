pub mod ffmpeg;

use std::net::SocketAddr;
use std::time::Duration;

pub trait SourceHandle: Send {
    fn stop(self: Box<Self>);
}

pub trait Source: Send + Sync {
    fn name(&self) -> &'static str;
    fn start(&self, target_addr: SocketAddr) -> anyhow::Result<Box<dyn SourceHandle>>;
    fn sdp(&self, listen_addr: SocketAddr) -> String;

    /// Wait until the source is producing frames and it is safe to subscribe.
    ///
    /// The default implementation sleeps briefly to give encoders time to emit
    /// the first keyframe. Individual sources may override this with a more
    /// deterministic readiness check.
    async fn wait_for_ready(&self) {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Supported video codecs for the RTP test sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Vp8,
    H264,
}

impl VideoCodec {
    pub fn as_str(&self) -> &'static str {
        match self {
            VideoCodec::Vp8 => "VP8",
            VideoCodec::H264 => "H264",
        }
    }

    /// RTP payload type used in the source SDP and FFmpeg output.
    ///
    /// These values are taken from the `rtc` media engine defaults so that
    /// liveion can match the incoming RTP stream without renegotiation.
    pub fn payload_type(&self) -> u8 {
        match self {
            VideoCodec::Vp8 => 96,
            VideoCodec::H264 => 102,
        }
    }

    /// FFmpeg encoder name for this codec.
    pub fn ffmpeg_encoder(&self) -> &'static str {
        match self {
            VideoCodec::Vp8 => "libvpx",
            VideoCodec::H264 => "libx264",
        }
    }

    /// Extra FFmpeg arguments required for a stable RTP stream.
    pub fn ffmpeg_extra_args(&self) -> &'static [&'static str] {
        match self {
            VideoCodec::Vp8 => &[
                "-pix_fmt",
                "yuv420p",
                "-deadline",
                "realtime",
                "-speed",
                "4",
            ],
            VideoCodec::H264 => &[
                "-pix_fmt",
                "yuv420p",
                "-profile:v",
                "baseline",
                "-level",
                "3.1",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
            ],
        }
    }

    /// SDP `a=rtpmap:` line for this codec.
    pub fn sdp_rtpmap(&self, payload_type: u8) -> String {
        match self {
            VideoCodec::Vp8 => format!("a=rtpmap:{payload_type} VP8/90000"),
            VideoCodec::H264 => format!(
                "a=rtpmap:{payload_type} H264/90000\r\n\
                 a=fmtp:{payload_type} level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
            ),
        }
    }
}
