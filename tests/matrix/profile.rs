//! Media profiles: declarative audio/video codec combinations shared by all
//! matrix sources and players.
//!
//! Every piece of codec knowledge (FFmpeg encoder names and arguments, RTP
//! payload types, SDP rtpmap/fmtp lines) lives here exactly once, so adding a
//! codec or a combination is a one-line change for every source kind.

use std::fmt;

/// Supported video codecs for the test sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// livetwo's synthetic-source codec, when the `rsmpeg` feature is enabled.
    #[cfg(feature = "rsmpeg")]
    pub fn to_livetwo(self) -> livetwo::source::VideoCodec {
        match self {
            VideoCodec::Vp8 => livetwo::source::VideoCodec::Vp8,
            VideoCodec::H264 => livetwo::source::VideoCodec::H264,
            VideoCodec::H265 => livetwo::source::VideoCodec::H265,
            VideoCodec::Vp9 => livetwo::source::VideoCodec::Vp9,
            VideoCodec::Av1 => livetwo::source::VideoCodec::Av1,
        }
    }

    /// Codec name as reported by ffprobe (H265 is reported as `hevc`).
    pub fn ffprobe_name(&self) -> &'static str {
        match self {
            VideoCodec::Vp8 => "vp8",
            VideoCodec::H264 => "h264",
            VideoCodec::H265 => "hevc",
            VideoCodec::Vp9 => "vp9",
            VideoCodec::Av1 => "av1",
        }
    }
}

/// Supported audio codecs for the test sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Opus,
    G722,
}

#[allow(dead_code)]
impl AudioCodec {
    pub fn as_str(&self) -> &'static str {
        match self {
            AudioCodec::Opus => "opus",
            AudioCodec::G722 => "g722",
        }
    }

    pub fn payload_type(&self) -> u8 {
        match self {
            AudioCodec::Opus => 111,
            AudioCodec::G722 => 9,
        }
    }

    pub fn ffmpeg_encoder(&self) -> &'static str {
        match self {
            AudioCodec::Opus => "libopus",
            AudioCodec::G722 => "g722",
        }
    }

    /// RTP payload name accepted by ffmpeg's RTP muxer `?codec=` query.
    pub fn rtp_payload_name(&self) -> &'static str {
        match self {
            AudioCodec::Opus => "OPUS",
            AudioCodec::G722 => "G722",
        }
    }

    /// Extra FFmpeg arguments for a stable audio stream.
    pub fn ffmpeg_extra_args(&self) -> &'static [&'static str] {
        match self {
            AudioCodec::Opus => &["-ar", "48000", "-ac", "2", "-b:a", "48k"],
            AudioCodec::G722 => &[],
        }
    }

    /// SDP `a=rtpmap:` line for this codec.
    pub fn sdp_rtpmap(&self, payload_type: u8) -> String {
        match self {
            AudioCodec::Opus => format!("a=rtpmap:{payload_type} opus/48000/2"),
            AudioCodec::G722 => format!("a=rtpmap:{payload_type} G722/8000"),
        }
    }

    /// Expected channel count of the decoded stream (used by ffprobe checks).
    pub fn channels(&self) -> u8 {
        match self {
            AudioCodec::Opus => 2,
            AudioCodec::G722 => 1,
        }
    }

    /// livetwo's synthetic-source codec, when the `rsmpeg` feature is enabled.
    #[cfg(feature = "rsmpeg")]
    pub fn to_livetwo(self) -> livetwo::source::AudioCodec {
        match self {
            AudioCodec::Opus => livetwo::source::AudioCodec::Opus,
            AudioCodec::G722 => livetwo::source::AudioCodec::G722,
        }
    }

    /// Codec name as reported by ffprobe (G722 is reported as `adpcm_g722`).
    pub fn ffprobe_name(&self) -> &'static str {
        match self {
            AudioCodec::Opus => "opus",
            AudioCodec::G722 => "adpcm_g722",
        }
    }
}

/// Video track parameters of a [`MediaProfile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoSpec {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

/// A declarative audio/video codec combination.
///
/// `None` means the track kind is absent: `MediaProfile { video: Some(..),
/// audio: None }` is video-only, `{ video: None, audio: Some(..) }` is
/// audio-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaProfile {
    pub video: Option<VideoSpec>,
    pub audio: Option<AudioCodec>,
}

impl MediaProfile {
    pub const DEFAULT_WIDTH: u32 = 640;
    pub const DEFAULT_HEIGHT: u32 = 480;
    pub const DEFAULT_FPS: u32 = 30;

    pub fn video_only(codec: VideoCodec) -> Self {
        Self {
            video: Some(VideoSpec {
                codec,
                width: Self::DEFAULT_WIDTH,
                height: Self::DEFAULT_HEIGHT,
                fps: Self::DEFAULT_FPS,
            }),
            audio: None,
        }
    }

    pub fn audio_only(codec: AudioCodec) -> Self {
        Self {
            video: None,
            audio: Some(codec),
        }
    }

    pub fn av(video: VideoCodec, audio: AudioCodec) -> Self {
        Self {
            video: Self::video_only(video).video,
            audio: Some(audio),
        }
    }

    /// Override the video resolution/fps (e.g. 4K stress variants).
    pub fn with_video_spec(mut self, width: u32, height: u32, fps: u32) -> Self {
        if let Some(ref mut video) = self.video {
            video.width = width;
            video.height = height;
            video.fps = fps;
        }
        self
    }

    /// Matrix case name, e.g. `vp8`, `opus`, `h264_opus`, `vp9_4k`.
    pub fn name(&self) -> String {
        let mut parts = Vec::new();
        if let Some(video) = self.video {
            let mut name = video.codec.as_str().to_lowercase();
            if video.width != Self::DEFAULT_WIDTH || video.height != Self::DEFAULT_HEIGHT {
                name = format!("{}_{}x{}", name, video.width, video.height);
            }
            parts.push(name);
        }
        if let Some(audio) = self.audio {
            parts.push(audio.as_str().to_string());
        }
        parts.join("_")
    }
}

impl fmt::Display for MediaProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}
