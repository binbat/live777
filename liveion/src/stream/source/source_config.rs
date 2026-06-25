//! Native source configuration types.
//!
//! Structured TOML config under a per-stream `[[stream.<name>.sources]]` block.
//! The source type is determined by `capture.backend`:
//!
//! ```toml
//! [stream.usb-cam]
//! [[stream.usb-cam.sources]]
//!
//! [stream.usb-cam.sources.capture]
//! backend = "v4l2"
//! device = "/dev/video0"
//! width = 640
//! height = 480
//! fps = 30
//! pixel_format = "yuyv"
//!
//! [stream.usb-cam.sources.encoder]
//! backend = "rdk"
//! codec = "h264"
//! bitrate = 1_500_000
//! profile = "42001f"
//! gop = 60
//!
//! [stream.usb-cam.sources.output]
//! payload_type = 96
//! clock_rate = 90000
//! ```

use serde::{Deserialize, Serialize};

#[cfg(feature = "native-source")]
use livehal::NativeSourceParams;

// ---------------------------------------------------------------------------
// Structured source configuration types (v2 — recommended)
// ---------------------------------------------------------------------------

/// Capture (input device) specification.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaptureSpec {
    /// Capture backend: `"libcamera"` or `"v4l2"`.
    pub backend: String,
    /// Capture device identifier.
    /// - For `libcamera`: camera ID, e.g. `"0"`.
    /// - For `v4l2`: device path, e.g. `"/dev/video0"`.
    #[serde(default)]
    pub device: Option<String>,
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EncoderSpec {
    /// Encoder backend: `"v4l2-m2m"` or `"rdk"`.
    pub backend: String,
    /// Video codec: `"h264"` or `"h265"`.
    pub codec: String,
    /// Target bitrate in bits per second.
    pub bitrate: u32,
    /// Codec profile identifier.
    /// For H.264 this is the profile/level-id hex string, e.g. `"42001f"`.
    pub profile: String,
    /// Optional explicit level (H.264/H.265 level-id component).
    /// When omitted, `profile` is treated as the complete profile-level-id.
    #[serde(default)]
    pub level: Option<String>,
    /// Optional encoder tier (H.265 only: `"main"` or `"high"`).
    #[serde(default)]
    pub tier: Option<String>,
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
        if self
            .capture
            .device
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            anyhow::bail!("capture.device cannot be empty");
        }
        let backend = self.capture.backend.to_lowercase();
        if backend != "libcamera" && backend != "v4l2" {
            anyhow::bail!(
                "capture.backend must be 'v4l2' or 'libcamera', got '{}'",
                self.capture.backend
            );
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

        let encoder_backend = self.encoder.backend.to_lowercase();
        if encoder_backend != "v4l2-m2m" && encoder_backend != "rdk" {
            anyhow::bail!(
                "encoder.backend must be 'v4l2-m2m' or 'rdk', got '{}'",
                self.encoder.backend
            );
        }

        // Validate profile/level/tier resolve to a usable profile-level-id.
        if let Err(e) = self.encoder.profile_level_id() {
            anyhow::bail!("encoder.profile/level/tier: {}", e);
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

impl EncoderSpec {
    /// Resolve the effective H.264 profile-level-id string.
    ///
    /// - If `profile` is already a 6-digit hex string, it is returned unchanged.
    /// - Otherwise `profile` is treated as a profile name and `level` must be provided.
    pub fn profile_level_id(&self) -> anyhow::Result<String> {
        if self.profile.len() == 6 && self.profile.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(self.profile.clone());
        }
        let level_idc = match self.level.as_deref() {
            Some(l) => h264_level_to_idc(l)?,
            None => anyhow::bail!(
                "encoder.level is required when profile is a profile name ('{}')",
                self.profile
            ),
        };
        let (profile_idc, constraint) = h264_profile_to_idc(&self.profile)?;
        Ok(format!("{}{}{:02x}", profile_idc, constraint, level_idc))
    }
}

/// Map common H.264 profile names to (profile_idc, constraint_set0_flag byte).
fn h264_profile_to_idc(name: &str) -> anyhow::Result<(&'static str, &'static str)> {
    match name.to_ascii_lowercase().as_str() {
        "baseline" => Ok(("42", "00")),
        "constrained-baseline" => Ok(("42", "C0")),
        "main" => Ok(("4D", "00")),
        "constrained-main" => Ok(("4D", "C0")),
        "extended" => Ok(("58", "00")),
        "high" => Ok(("64", "00")),
        "constrained-high" => Ok(("64", "C0")),
        "high-10" => Ok(("6E", "00")),
        "high-422" => Ok(("7A", "00")),
        "high-444" => Ok(("F4", "00")),
        other => anyhow::bail!(
            "unsupported H.264 profile name '{}'. Supported: baseline, constrained-baseline, main, constrained-main, extended, high, constrained-high, high-10, high-422, high-444",
            other
        ),
    }
}

/// Map H.264 level strings (e.g. "3.1", "5") to level_idc.
fn h264_level_to_idc(level: &str) -> anyhow::Result<u8> {
    let normalized = level.trim();
    let idc = if let Some(dot) = normalized.find('.') {
        let major: u8 = normalized[..dot].parse()?;
        let minor: u8 = normalized[dot + 1..].parse()?;
        major * 10 + minor
    } else {
        normalized.parse::<u8>()? * 10
    };
    if idc == 0 || idc > 186 {
        anyhow::bail!("invalid H.264 level '{}'", level);
    }
    Ok(idc)
}

impl SourceSpec {
    /// Build `NativeSourceParams` directly from a structured `SourceSpec`.
    ///
    /// This is the direct path — no URL-based roundtrip.
    /// Returns an error if `pixel_format` or `codec` strings are unrecognised.
    #[cfg(feature = "native-source")]
    pub fn to_native_params(&self) -> anyhow::Result<NativeSourceParams> {
        let backend = self.capture.backend.to_lowercase();
        if backend != "libcamera" && backend != "v4l2" {
            anyhow::bail!("unsupported capture backend: {}", self.capture.backend);
        }
        let capture_device = self.capture.device.clone().unwrap_or_default();
        Ok(NativeSourceParams {
            capture_backend: self.capture.backend.clone(),
            capture_device,
            width: self.capture.width,
            height: self.capture.height,
            fps: self.capture.fps,
            capture_pixel_format: pixel_format_to_u32(&self.capture.pixel_format)?,
            encoder_backend: self.encoder.backend.clone(),
            codec: codec_to_u32(&self.encoder.codec)?,
            bitrate: self.encoder.bitrate,
            profile: self.encoder.profile_level_id()?,
            gop: self.encoder.gop,
            payload_type: self.output.payload_type as u32,
            clock_rate: self.output.clock_rate,
            capture_prefer_dmabuf: self.capture.prefer_dmabuf as u8,
            encoder_prefer_dmabuf: self.encoder.prefer_dmabuf as u8,
            codec_name: self.encoder.codec.to_uppercase(),
            default_profile: self.encoder.profile_level_id()?,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn v4l2_spec() -> SourceSpec {
        SourceSpec {
            stream_id: "cam1".into(),
            capture: CaptureSpec {
                backend: "v4l2".into(),
                device: Some("/dev/video0".into()),
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
                level: None,
                tier: None,
                gop: 60,
                prefer_dmabuf: false,
            },
            output: OutputSpec::default(),
        }
    }

    fn libcamera_spec() -> SourceSpec {
        SourceSpec {
            stream_id: "pi-cam".into(),
            capture: CaptureSpec {
                backend: "libcamera".into(),
                device: Some("0".into()),
                width: 1920,
                height: 1080,
                fps: 30,
                pixel_format: "nv12".into(),
                prefer_dmabuf: true,
            },
            encoder: EncoderSpec {
                backend: "rdk".into(),
                codec: "h264".into(),
                bitrate: 2_000_000,
                profile: "high".into(),
                level: Some("4.2".into()),
                tier: None,
                gop: 60,
                prefer_dmabuf: false,
            },
            output: OutputSpec::default(),
        }
    }

    // --- SourceSpec validation tests ---

    #[test]
    fn test_source_spec_validate_v4l2_ok() {
        assert!(v4l2_spec().validate().is_ok());
    }

    #[test]
    fn test_source_spec_validate_libcamera_ok() {
        assert!(libcamera_spec().validate().is_ok());
    }

    #[test]
    fn test_source_spec_validate_empty_id() {
        let mut spec = v4l2_spec();
        spec.stream_id = "  ".into();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_spec_validate_missing_v4l2_device() {
        let mut spec = v4l2_spec();
        spec.capture.device = None;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_spec_validate_missing_libcamera_device() {
        let mut spec = libcamera_spec();
        spec.capture.device = None;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_source_spec_validate_zero_size() {
        let mut spec = v4l2_spec();
        spec.capture.width = 0;
        spec.capture.height = 0;
        assert!(spec.validate().is_err());
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

    // --- profile/level/tier tests ---

    #[test]
    fn test_profile_level_id_hex_passthrough() {
        let enc = EncoderSpec {
            profile: "64001f".into(),
            level: None,
            ..Default::default()
        };
        assert_eq!(enc.profile_level_id().unwrap(), "64001f");
    }

    #[test]
    fn test_profile_level_id_from_name() {
        let enc = EncoderSpec {
            profile: "high".into(),
            level: Some("4.2".into()),
            ..Default::default()
        };
        assert_eq!(enc.profile_level_id().unwrap(), "64002a");
    }

    #[test]
    fn test_profile_level_id_name_without_level() {
        let enc = EncoderSpec {
            profile: "main".into(),
            level: None,
            ..Default::default()
        };
        assert!(enc.profile_level_id().is_err());
    }

    // --- NativeSourceParams conversion tests ---

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_v4l2() {
        let params = v4l2_spec().to_native_params().unwrap();
        assert_eq!(params.capture_backend, "v4l2");
        assert_eq!(params.capture_device, "/dev/video0");
        assert_eq!(params.width, 640);
        assert_eq!(params.height, 480);
        assert_eq!(params.capture_pixel_format, 0); // yuyv
        assert_eq!(params.encoder_backend, "v4l2-m2m");
        assert_eq!(params.codec, 100); // h264
        assert_eq!(params.profile, "42001f");
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_libcamera() {
        let params = libcamera_spec().to_native_params().unwrap();
        assert_eq!(params.capture_backend, "libcamera");
        assert_eq!(params.capture_device, "0");
        assert_eq!(params.encoder_backend, "rdk");
        assert_eq!(params.profile, "64002a");
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_invalid_pixel_format() {
        let mut spec = v4l2_spec();
        spec.capture.pixel_format = "bad_format".into();
        assert!(spec.to_native_params().is_err());
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_invalid_codec() {
        let mut spec = v4l2_spec();
        spec.encoder.codec = "h266".into();
        assert!(spec.to_native_params().is_err());
    }
}
