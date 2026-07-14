pub mod ffmpeg;
pub mod gstreamer_vp8;
#[cfg(feature = "rsmpeg")]
pub mod rsmpeg_vp8;
#[cfg(feature = "rtsp")]
pub mod rtsp_ffmpeg;
#[cfg(feature = "rsmpeg")]
pub mod whipsynth;

use std::net::SocketAddr;
use std::time::Duration;

#[async_trait::async_trait]
pub trait SourceHandle: Send {
    async fn stop(self: Box<Self>);
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

    /// Whether this source publishes via RTSP (ANNOUNCE + RECORD) instead of
    /// WHIP. RTSP sources receive the RTSP URL directly via
    /// [`Self::start_rtsp`].
    #[cfg(feature = "rtsp")]
    fn is_rtsp(&self) -> bool {
        false
    }

    /// Start pushing to the liveion RTSP server.
    ///
    /// Only called when [`Self::is_rtsp`] returns `true`.  `rtsp_url` is the
    /// full RTSP URL, e.g. `rtsp://127.0.0.1:8554/-`.
    #[cfg(feature = "rtsp")]
    fn start_rtsp(&self, rtsp_url: &str) -> anyhow::Result<Box<dyn SourceHandle>> {
        let _ = rtsp_url;
        anyhow::bail!("RTSP publishing not supported by this source")
    }

    /// Start direct WHIP publishing. Only called when [`Self::publishes_directly`]
    /// returns `true`.
    fn start_direct(&self, _whip_url: &str) -> anyhow::Result<Box<dyn SourceHandle>> {
        anyhow::bail!("direct publishing not supported by this source")
    }

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

    /// RTP payload name (the encoding name in `a=rtpmap` and the value ffmpeg's
    /// RTP muxer accepts for the `?codec=` query), e.g. `VP8`, `AV1`.
    pub fn rtp_payload_name(&self) -> &'static str {
        match self {
            VideoCodec::Vp8 => "VP8",
            VideoCodec::H264 => "H264",
            VideoCodec::H265 => "H265",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Av1 => "AV1",
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
                "-strict",
                "experimental",
                "-pix_fmt",
                "yuv420p",
                "-deadline",
                "realtime",
                "-speed",
                "4",
            ],
            VideoCodec::Av1 => &[
                "-strict",
                "experimental",
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
        let name = self.rtp_payload_name();
        match self {
            VideoCodec::H264 => format!(
                "a=rtpmap:{payload_type} {name}/90000\r\n\
                 a=fmtp:{payload_type} level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
            ),
            _ => format!("a=rtpmap:{payload_type} {name}/90000"),
        }
    }
}
