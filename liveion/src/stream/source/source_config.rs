//! Native source configuration types.
//!
//! Structured TOML config under `[[stream.sources]]`:
//!
//! ```toml
//! [[stream.sources]]
//! stream_id = "usbcam"
//! kind = "v4l2"
//!
//! [stream.sources.capture]
//! backend = "v4l2"
//! device = "/dev/video0"
//! width = 640
//! height = 480
//! fps = 30
//! pixel_format = "yuyv"
//!
//! [stream.sources.encoder]
//! backend = "rdk"
//! codec = "h264"
//! bitrate = 1_500_000
//! profile = "42001f"
//! gop = 60
//!
//! [stream.sources.output]
//! payload_type = 96
//! clock_rate = 90000
//! ```

use serde::{Deserialize, Serialize};

#[cfg(feature = "native-source")]
use livesrc::NativeSourceParams;

// ---------------------------------------------------------------------------
// Structured source configuration types (v2 — recommended)
// ---------------------------------------------------------------------------

/// Identifies the type of media source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    V4l2,
    Libcamera,
}

/// Capture (input device) specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureSpec {
    /// Capture backend: `"libcamera"` or `"v4l2"`.
    pub backend: String,
    /// Device path, e.g. `"/dev/video0"`.
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Raw pixel format: `"yuyv"`, `"nv12"`, `"yuv420"`, `"mjpeg"`.
    pub pixel_format: String,
    /// Prefer DMA-BUF zero-copy path (default `false`).
    #[serde(default)]
    pub prefer_dmabuf: bool,
}

/// Encoder specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncoderSpec {
    /// Encoder backend: `"v4l2-m2m"` or `"rdk"`.
    pub backend: String,
    /// Video codec: `"h264"` or `"h265"`.
    pub codec: String,
    /// Target bitrate in bits per second.
    pub bitrate: u32,
    /// H.264 profile-level-id, e.g. `"42001f"`.
    pub profile: String,
    /// GOP size (keyframe interval).
    #[serde(default = "default_gop")]
    pub gop: u32,
    /// Prefer DMA-BUF zero-copy path (default `false`).
    #[serde(default)]
    pub prefer_dmabuf: bool,
}

fn default_gop() -> u32 {
    60
}

/// RTP output specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputSpec {
    /// RTP payload type (default `96`).
    #[serde(default = "default_payload_type")]
    pub payload_type: u8,
    /// RTP clock rate in Hz (default `90000`).
    #[serde(default = "default_clock_rate")]
    pub clock_rate: u32,
}

fn default_payload_type() -> u8 {
    96
}

fn default_clock_rate() -> u32 {
    90000
}

/// Full specification for a single media source.
///
/// This is the recommended structured config format replacing the
/// URL query-string approach.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSpec {
    /// Unique stream identifier.
    pub stream_id: String,
    /// Type of media source.
    pub kind: SourceKind,
    /// Capture / input device configuration.
    pub capture: CaptureSpec,
    /// Encoder configuration.
    pub encoder: EncoderSpec,
    /// RTP output parameters.
    #[serde(default)]
    pub output: OutputSpec,
}

impl Default for OutputSpec {
    fn default() -> Self {
        Self {
            payload_type: default_payload_type(),
            clock_rate: default_clock_rate(),
        }
    }
}

impl SourceSpec {
    /// Validate the source specification.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.stream_id.trim().is_empty() {
            anyhow::bail!("stream_id cannot be empty");
        }
        if self.capture.device.trim().is_empty() {
            anyhow::bail!("capture.device cannot be empty");
        }
        if self.capture.width == 0 || self.capture.height == 0 {
            anyhow::bail!("capture width/height must be non-zero");
        }
        if self.capture.fps == 0 {
            anyhow::bail!("capture.fps must be non-zero");
        }
        if self.encoder.bitrate == 0 {
            anyhow::bail!("encoder.bitrate must be non-zero");
        }
        if self.encoder.gop == 0 {
            anyhow::bail!("encoder.gop must be non-zero");
        }

        let capture_backend = self.capture.backend.to_lowercase();
        if capture_backend != "v4l2" && capture_backend != "libcamera" {
            anyhow::bail!(
                "capture.backend must be 'v4l2' or 'libcamera', got '{}'",
                self.capture.backend
            );
        }

        let encoder_backend = self.encoder.backend.to_lowercase();
        if encoder_backend != "v4l2-m2m" && encoder_backend != "rdk" {
            anyhow::bail!(
                "encoder.backend must be 'v4l2-m2m' or 'rdk', got '{}'",
                self.encoder.backend
            );
        }

        // H.264 profile-level-id is a 6-digit hex string such as "42001f".
        if self.encoder.profile.len() != 6
            || !self.encoder.profile.chars().all(|c| c.is_ascii_hexdigit())
        {
            anyhow::bail!(
                "encoder.profile must be a 6-digit hex string, got '{}'",
                self.encoder.profile
            );
        }

        // Validate pixel_format and codec strings early so config errors
        // surface during validation rather than at source creation time.
        pixel_format_to_u32(&self.capture.pixel_format)
            .map_err(|e| anyhow::anyhow!("capture.pixel_format: {}", e))?;
        codec_to_u32(&self.encoder.codec).map_err(|e| anyhow::anyhow!("encoder.codec: {}", e))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Structured → NativeSourceParams conversion
// ---------------------------------------------------------------------------

/// Raw pixel formats understood by the native capture backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PixelFormat {
    Yuyv422 = 0,
    Nv12 = 1,
    Yuv420p = 2,
    Mjpeg = 3,
    Rgb888 = 4,
}

impl From<PixelFormat> for u32 {
    fn from(p: PixelFormat) -> Self {
        p as u32
    }
}

/// Video codecs understood by the native encoder backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VideoCodec {
    H264 = 100,
    H265 = 101,
    Av1 = 102,
    Vp8 = 103,
    Vp9 = 104,
}

impl From<VideoCodec> for u32 {
    fn from(c: VideoCodec) -> Self {
        c as u32
    }
}

impl TryFrom<u32> for VideoCodec {
    type Error = anyhow::Error;

    fn try_from(value: u32) -> anyhow::Result<Self> {
        match value {
            100 => Ok(VideoCodec::H264),
            101 => Ok(VideoCodec::H265),
            102 => Ok(VideoCodec::Av1),
            103 => Ok(VideoCodec::Vp8),
            104 => Ok(VideoCodec::Vp9),
            other => anyhow::bail!("unsupported video codec value: {}", other),
        }
    }
}

/// Map a pixel format string to its `RawPixelFormat` numeric value.
///
/// Used when converting structured `CaptureSpec` into `NativeSourceParams`.
pub fn pixel_format_to_u32(s: &str) -> anyhow::Result<u32> {
    pixel_format_from_str(s).map(Into::into)
}

/// Parse a pixel format string into a typed enum.
pub fn pixel_format_from_str(s: &str) -> anyhow::Result<PixelFormat> {
    match s.to_lowercase().as_str() {
        "yuyv" | "yuyv422" => Ok(PixelFormat::Yuyv422),
        "nv12" => Ok(PixelFormat::Nv12),
        "yuv420" | "yuv420p" => Ok(PixelFormat::Yuv420p),
        "mjpeg" => Ok(PixelFormat::Mjpeg),
        "rgb888" | "rgb" => Ok(PixelFormat::Rgb888),
        other => anyhow::bail!(
            "unsupported pixel_format: '{}'. Supported: yuyv, nv12, yuv420, mjpeg, rgb888",
            other
        ),
    }
}

/// Map a codec string to its `VideoCodec` numeric value.
///
/// Used when converting structured `EncoderSpec` into `NativeSourceParams`.
pub fn codec_to_u32(s: &str) -> anyhow::Result<u32> {
    video_codec_from_str(s).map(Into::into)
}

/// Parse a codec string into a typed enum.
pub fn video_codec_from_str(s: &str) -> anyhow::Result<VideoCodec> {
    match s.to_lowercase().as_str() {
        "h264" => Ok(VideoCodec::H264),
        "h265" | "hevc" => Ok(VideoCodec::H265),
        "av1" => Ok(VideoCodec::Av1),
        "vp8" => Ok(VideoCodec::Vp8),
        "vp9" => Ok(VideoCodec::Vp9),
        other => anyhow::bail!(
            "unsupported codec: '{}'. Supported: h264, h265, av1, vp8, vp9",
            other
        ),
    }
}

impl SourceSpec {
    /// Build `NativeSourceParams` directly from a structured `SourceSpec`.
    ///
    /// This is the direct path — no URL-based roundtrip.
    /// Returns an error if `pixel_format` or `codec` strings are unrecognised.
    #[cfg(feature = "native-source")]
    pub fn to_native_params(&self) -> anyhow::Result<NativeSourceParams> {
        Ok(NativeSourceParams {
            capture_backend: self.capture.backend.clone(),
            capture_device: self.capture.device.clone(),
            width: self.capture.width,
            height: self.capture.height,
            fps: self.capture.fps,
            capture_pixel_format: pixel_format_to_u32(&self.capture.pixel_format)?,
            encoder_backend: self.encoder.backend.clone(),
            codec: codec_to_u32(&self.encoder.codec)?,
            bitrate: self.encoder.bitrate,
            profile: self.encoder.profile.clone(),
            gop: self.encoder.gop,
            payload_type: self.output.payload_type as u32,
            clock_rate: self.output.clock_rate,
            capture_prefer_dmabuf: self.capture.prefer_dmabuf as u8,
            encoder_prefer_dmabuf: self.encoder.prefer_dmabuf as u8,
            codec_name: self.encoder.codec.to_uppercase(),
            default_profile: self.encoder.profile.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- SourceSpec (structured) tests ---

    #[test]
    fn test_source_spec_validate_ok() {
        let spec = SourceSpec {
            stream_id: "cam1".into(),
            kind: SourceKind::V4l2,
            capture: CaptureSpec {
                backend: "v4l2".into(),
                device: "/dev/video0".into(),
                width: 640,
                height: 480,
                fps: 30,
                pixel_format: "yuyv".into(),
                prefer_dmabuf: false,
            },
            encoder: EncoderSpec {
                backend: "v4l2-m2m".into(),
                codec: "h264".into(),
                bitrate: 1_500_000,
                profile: "42001f".into(),
                gop: 60,
                prefer_dmabuf: false,
            },
            output: OutputSpec::default(),
        };
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_source_spec_validate_empty_id() {
        let spec = SourceSpec {
            stream_id: "  ".into(),
            kind: SourceKind::V4l2,
            capture: CaptureSpec {
                backend: "v4l2".into(),
                device: "/dev/video0".into(),
                width: 640,
                height: 480,
                fps: 30,
                pixel_format: "yuyv".into(),
                prefer_dmabuf: false,
            },
            encoder: EncoderSpec {
                backend: "v4l2-m2m".into(),
                codec: "h264".into(),
                bitrate: 1_000_000,
                profile: "42001f".into(),
                gop: 60,
                prefer_dmabuf: false,
            },
            output: OutputSpec::default(),
        };
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_spec_validate_zero_size() {
        let spec = SourceSpec {
            stream_id: "cam1".into(),
            kind: SourceKind::V4l2,
            capture: CaptureSpec {
                backend: "v4l2".into(),
                device: "/dev/video0".into(),
                width: 0,
                height: 0,
                fps: 30,
                pixel_format: "yuyv".into(),
                prefer_dmabuf: false,
            },
            encoder: EncoderSpec {
                backend: "v4l2-m2m".into(),
                codec: "h264".into(),
                bitrate: 1_000_000,
                profile: "42001f".into(),
                gop: 60,
                prefer_dmabuf: false,
            },
            output: OutputSpec::default(),
        };
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_kind_serde() {
        let json = r#""v4l2""#;
        let kind: SourceKind = serde_json::from_str(json).unwrap();
        assert_eq!(kind, SourceKind::V4l2);
        assert_eq!(serde_json::to_string(&kind).unwrap(), r#""v4l2""#);

        let json = r#""libcamera""#;
        let kind: SourceKind = serde_json::from_str(json).unwrap();
        assert_eq!(kind, SourceKind::Libcamera);
    }

    // --- pixel_format / codec mapping tests ---

    #[test]
    fn test_pixel_format_to_u32_valid() {
        assert_eq!(pixel_format_to_u32("yuyv").unwrap(), 0);
        assert_eq!(pixel_format_to_u32("YUYV422").unwrap(), 0);
        assert_eq!(pixel_format_to_u32("nv12").unwrap(), 1);
        assert_eq!(pixel_format_to_u32("yuv420").unwrap(), 2);
        assert_eq!(pixel_format_to_u32("yuv420p").unwrap(), 2);
        assert_eq!(pixel_format_to_u32("mjpeg").unwrap(), 3);
        assert_eq!(pixel_format_to_u32("rgb888").unwrap(), 4);
        assert_eq!(pixel_format_to_u32("rgb").unwrap(), 4);
    }

    #[test]
    fn test_pixel_format_to_u32_invalid() {
        assert!(pixel_format_to_u32("yyyv").is_err());
        assert!(pixel_format_to_u32("").is_err());
        assert!(pixel_format_to_u32("h264").is_err());
    }

    #[test]
    fn test_codec_to_u32_valid() {
        assert_eq!(codec_to_u32("h264").unwrap(), 100);
        assert_eq!(codec_to_u32("H264").unwrap(), 100);
        assert_eq!(codec_to_u32("h265").unwrap(), 101);
        assert_eq!(codec_to_u32("hevc").unwrap(), 101);
        assert_eq!(codec_to_u32("av1").unwrap(), 102);
        assert_eq!(codec_to_u32("vp8").unwrap(), 103);
        assert_eq!(codec_to_u32("vp9").unwrap(), 104);
    }

    #[test]
    fn test_codec_to_u32_invalid() {
        assert!(codec_to_u32("h266").is_err());
        assert!(codec_to_u32("").is_err());
        assert!(codec_to_u32("mjpeg").is_err());
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_valid() {
        let spec = SourceSpec {
            stream_id: "cam1".into(),
            kind: SourceKind::V4l2,
            capture: CaptureSpec {
                backend: "v4l2".into(),
                device: "/dev/video0".into(),
                width: 1920,
                height: 1080,
                fps: 60,
                pixel_format: "nv12".into(),
                prefer_dmabuf: true,
            },
            encoder: EncoderSpec {
                backend: "rdk".into(),
                codec: "h265".into(),
                bitrate: 4_000_000,
                profile: "42001f".into(),
                gop: 30,
                prefer_dmabuf: false,
            },
            output: OutputSpec {
                payload_type: 97,
                clock_rate: 90000,
            },
        };

        let params = spec.to_native_params().unwrap();
        assert_eq!(params.capture_backend, "v4l2");
        assert_eq!(params.capture_device, "/dev/video0");
        assert_eq!(params.width, 1920);
        assert_eq!(params.height, 1080);
        assert_eq!(params.fps, 60);
        assert_eq!(params.capture_pixel_format, 1); // nv12
        assert_eq!(params.encoder_backend, "rdk");
        assert_eq!(params.codec, 101); // h265
        assert_eq!(params.bitrate, 4_000_000);
        assert_eq!(params.profile, "42001f");
        assert_eq!(params.gop, 30);
        assert_eq!(params.payload_type, 97);
        assert_eq!(params.clock_rate, 90000);
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_invalid_pixel_format() {
        let spec = SourceSpec {
            stream_id: "bad".into(),
            kind: SourceKind::V4l2,
            capture: CaptureSpec {
                backend: "v4l2".into(),
                device: "/dev/video0".into(),
                width: 640,
                height: 480,
                fps: 30,
                pixel_format: "bad_format".into(),
                prefer_dmabuf: false,
            },
            encoder: EncoderSpec {
                backend: "v4l2-m2m".into(),
                codec: "h264".into(),
                bitrate: 1_000_000,
                profile: "42001f".into(),
                gop: 60,
                prefer_dmabuf: false,
            },
            output: OutputSpec::default(),
        };

        assert!(spec.to_native_params().is_err());
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_invalid_codec() {
        let spec = SourceSpec {
            stream_id: "bad".into(),
            kind: SourceKind::V4l2,
            capture: CaptureSpec {
                backend: "v4l2".into(),
                device: "/dev/video0".into(),
                width: 640,
                height: 480,
                fps: 30,
                pixel_format: "yuyv".into(),
                prefer_dmabuf: false,
            },
            encoder: EncoderSpec {
                backend: "v4l2-m2m".into(),
                codec: "h266".into(),
                bitrate: 1_000_000,
                profile: "42001f".into(),
                gop: 60,
                prefer_dmabuf: false,
            },
            output: OutputSpec::default(),
        };

        assert!(spec.to_native_params().is_err());
    }
}
