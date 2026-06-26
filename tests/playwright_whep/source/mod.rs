pub mod ffmpeg;
pub mod gstreamer_vp8;
pub mod rsmpeg_vp8;
#[cfg(feature = "rsmpeg")]
pub mod whipsynth;

use std::net::SocketAddr;

pub trait SourceHandle: Send {
    fn stop(self: Box<Self>);
}

pub trait Source: Send + Sync {
    fn name(&self) -> &'static str;
    fn start(&self, target_addr: SocketAddr) -> anyhow::Result<Box<dyn SourceHandle>>;
    fn sdp(&self, listen_addr: SocketAddr) -> String;

    /// Whether this source produces an audio track in addition to video.
    fn has_audio(&self) -> bool {
        false
    }

    /// Start the source with separate video and optional audio destinations.
    ///
    /// Defaults to [`Self::start`] for video-only sources.
    fn start_with_audio(
        &self,
        video_addr: SocketAddr,
        _audio_addr: Option<SocketAddr>,
    ) -> anyhow::Result<Box<dyn SourceHandle>> {
        self.start(video_addr)
    }

    /// Build an SDP with separate video and optional audio ports.
    ///
    /// Defaults to [`Self::sdp`] for video-only sources.
    fn sdp_with_audio(&self, video_addr: SocketAddr, _audio_addr: Option<SocketAddr>) -> String {
        self.sdp(video_addr)
    }

    /// Whether this source publishes directly to a WHIP endpoint instead of
    /// emitting RTP to a local address.
    fn publishes_directly(&self) -> bool {
        false
    }

    /// Start direct WHIP publishing. Only called when [`Self::publishes_directly`]
    /// returns `true`.
    fn start_direct(&self, _whip_url: &str) -> anyhow::Result<Box<dyn SourceHandle>> {
        anyhow::bail!("direct publishing not supported by this source")
    }
}

/// Supported video codecs for the RTP test sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum VideoCodec {
    Vp8,
    H264,
    H265,
    Vp9,
    Av1,
}

#[allow(dead_code)]
impl VideoCodec {
    pub fn as_str(&self) -> &'static str {
        match self {
            VideoCodec::Vp8 => "VP8",
            VideoCodec::H264 => "H264",
            VideoCodec::H265 => "H265",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Av1 => "AV1",
        }
    }

    pub fn mime_type(&self) -> &'static str {
        match self {
            VideoCodec::Vp8 => "video/VP8",
            VideoCodec::H264 => "video/H264",
            VideoCodec::H265 => "video/H265",
            VideoCodec::Vp9 => "video/VP9",
            VideoCodec::Av1 => "video/AV1",
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
            VideoCodec::H265 => 126,
            VideoCodec::Vp9 => 98,
            VideoCodec::Av1 => 41,
        }
    }

    /// FFmpeg encoder name for this codec.
    pub fn ffmpeg_encoder(&self) -> &'static str {
        match self {
            VideoCodec::Vp8 => "libvpx",
            VideoCodec::H264 => "libx264",
            VideoCodec::H265 => "libx265",
            VideoCodec::Vp9 => "libvpx-vp9",
            VideoCodec::Av1 => "libsvtav1",
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
            VideoCodec::H265 => &[
                "-pix_fmt",
                "yuv420p",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
            ],
            VideoCodec::Vp9 => &[
                "-pix_fmt",
                "yuv420p",
                "-deadline",
                "realtime",
                "-speed",
                "4",
            ],
            VideoCodec::Av1 => &[
                "-pix_fmt",
                "yuv420p",
                "-preset",
                "8",
                "-svtav1-params",
                "tune=0",
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
            VideoCodec::H265 => format!("a=rtpmap:{payload_type} H265/90000"),
            VideoCodec::Vp9 => format!("a=rtpmap:{payload_type} VP9/90000"),
            VideoCodec::Av1 => format!("a=rtpmap:{payload_type} AV1/90000"),
        }
    }
}
