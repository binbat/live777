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
    /// Resolved profile parameters for the configured codec.
    ///
    /// `profile_idc`/`level_idc`/`tier_flag` are the numeric values passed
    /// to the native encoder and the H.265 SDP fmtp line; `profile_string`
    /// is the H.264-style profile-level-id (empty for H.265).
    fn resolve_profile(&self) -> anyhow::Result<(u32, u32, u32, String)> {
        let codec_str = self.encoder.codec.to_ascii_lowercase();
        let is_rkmpp = self.encoder.backend.eq_ignore_ascii_case("rkmpp");

        if is_rkmpp && !matches!(codec_str.as_str(), "h264" | "h265" | "hevc") {
            anyhow::bail!(
                "encoder backend 'rkmpp' supports only h264 and h265, got '{}'",
                self.encoder.codec
            );
        }

        // Resolve profile_idc / level_idc / tier_flag per codec.
        // Rust is the sole parsing layer; C++ receives numeric values directly.
        // rkmpp passes them to the encoder; every backend needs the H.265
        // numerics for the SDP fmtp line.
        let (profile_idc, level_idc, tier_flag) = match codec_str.as_str() {
            "h264" if is_rkmpp => {
                let (pidc, lidc) = self.encoder.h264_profile_level_id()?;
                (u32::from(pidc), u32::from(lidc), 0u32)
            }
            "h265" | "hevc" => {
                // h265_profile_params returns (profile_idc, tier_flag, level_idc)
                let (pidc, tier, lidc) = self.encoder.h265_profile_params()?;
                (u32::from(pidc), u32::from(lidc), u32::from(tier))
            }
            // v4l2-m2m / rdk H.264 and the other codecs: profile/level are
            // resolved via string helpers; numeric fields are unused.
            _ => (0u32, 0u32, 0u32),
        };

        // H.265 SDP fmtp is built from the numeric fields above; the
        // H.264-style profile-level-id string does not apply.
        let is_h265 = matches!(codec_str.as_str(), "h265" | "hevc");
        let profile_string = if is_h265 {
            String::new()
        } else {
            self.encoder.profile_level_id()?
        };

        Ok((profile_idc, level_idc, tier_flag, profile_string))
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
        if encoder_backend != "v4l2-m2m" && encoder_backend != "rdk" && encoder_backend != "rkmpp" {
            anyhow::bail!(
                "encoder.backend must be 'v4l2-m2m', 'rdk', or 'rkmpp', got '{}'",
                self.encoder.backend
            );
        }

        // Profile/level/tier are resolved with the same rules
        // to_native_params() uses, so anything validate() accepts can be
        // converted and vice versa.
        if let Err(e) = self.resolve_profile() {
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
    /// For H.264 the level is encoded in the 6-char profile-level-id hex.
    /// When `profile` is a profile name, the separate `level` field is
    /// required to build the hex string.
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

    /// Parse a 6-char H.264 profile-level-id hex string into (profile_idc, level_idc).
    ///
    /// Only accepts Baseline (66), Main (77), and High (100) profile_idc.
    /// Rejects a separate `level` field — the level comes from the hex string only.
    pub fn h264_profile_level_id(&self) -> anyhow::Result<(u8, u8)> {
        if self.level.is_some() {
            anyhow::bail!(
                "H.264 does not use a separate 'level' field; \
                 the level is encoded in the profile-level-id hex (e.g. '420028' = Baseline Level 4.0). \
                 Remove the 'level' field from this encoder config."
            );
        }
        let hex = self.profile.trim();
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!(
                "H.264 requires a 6-character profile-level-id hex string (e.g. '420028'), got '{}'",
                self.profile
            );
        }
        let profile_idc = u8::from_str_radix(&hex[0..2], 16)?;
        let level_idc = u8::from_str_radix(&hex[4..6], 16)?;

        match profile_idc {
            66 | 77 | 100 => {}
            other => anyhow::bail!(
                "unsupported H.264 profile_idc {} (from '{}'); \
                 supported: 66=Baseline, 77=Main, 100=High",
                other,
                self.profile
            ),
        }
        if level_idc == 0 || level_idc > 62 {
            anyhow::bail!(
                "invalid H.264 level_idc {} in '{}' (max 62 = Level 6.2)",
                level_idc,
                self.profile
            );
        }
        Ok((profile_idc, level_idc))
    }

    /// Resolve H.265 profile parameters.
    ///
    /// Returns `(profile_idc, tier_flag, level_idc)`.
    /// MVP accepts only Main profile + Main tier.
    pub fn h265_profile_params(&self) -> anyhow::Result<(u8, u8, u8)> {
        let profile = self.profile.trim();

        // Hex check BEFORE name match — produces targeted error
        if profile.len() == 6 && profile.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!(
                "H.265 does not use H.264-style profile-level-id '{}'; use profile='main'",
                profile
            );
        }

        let profile_idc = match profile.to_ascii_lowercase().as_str() {
            "" | "main" => 1u8,
            other => anyhow::bail!(
                "unsupported H.265 profile '{}'; only 'main' is supported",
                other
            ),
        };

        let tier_flag = match self.tier.as_deref() {
            None | Some("main") => 0u8,
            Some("high") => anyhow::bail!("H.265 high tier is not supported in MVP"),
            Some(other) => anyhow::bail!("unsupported H.265 tier '{}'", other),
        };

        let level_idc = match self.level.as_deref() {
            Some(l) => h265_level_to_idc(l)?,
            None => 120u8, // default Level 4.0
        };

        Ok((profile_idc, tier_flag, level_idc))
    }
}

/// Map H.265 level strings (e.g. "4.0") to level_idc per HEVC spec.
/// level_idc = level × 30 (e.g. 4.0 → 120).
fn h265_level_to_idc(level: &str) -> anyhow::Result<u8> {
    match level.trim() {
        "3.0" => Ok(90),
        "3.1" => Ok(93),
        "4.0" => Ok(120),
        "4.1" => Ok(123),
        "5.0" => Ok(150),
        "5.1" => Ok(153),
        other => anyhow::bail!(
            "unsupported H.265 level '{}'. Supported: 3.0, 3.1, 4.0, 4.1, 5.0, 5.1",
            other
        ),
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
    if idc == 0 || idc > 62 {
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

        // Profile resolution is shared with validate() — see resolve_profile().
        let (profile_idc, level_idc, tier_flag, profile_string) = self.resolve_profile()?;

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
            profile: profile_string.clone(),
            gop: self.encoder.gop,
            payload_type: self.output.payload_type as u32,
            clock_rate: self.output.clock_rate,
            capture_prefer_dmabuf: self.capture.prefer_dmabuf as u8,
            encoder_prefer_dmabuf: self.encoder.prefer_dmabuf as u8,
            codec_name: self.encoder.codec.to_uppercase(),
            default_profile: profile_string,
            profile_idc,
            level_idc,
            tier_flag,
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

    fn rkmpp_spec() -> SourceSpec {
        SourceSpec {
            stream_id: "rk-cam".into(),
            capture: CaptureSpec {
                backend: "v4l2".into(),
                device: Some("/dev/video11".into()),
                width: 1920,
                height: 1080,
                fps: 30,
                pixel_format: "nv12".into(),
                prefer_dmabuf: false,
            },
            encoder: EncoderSpec {
                backend: "rkmpp".into(),
                codec: "h264".into(),
                bitrate: 4_000_000,
                profile: "640028".into(), // High Level 4.0
                level: None,
                tier: None,
                gop: 60,
                prefer_dmabuf: false,
            },
            output: OutputSpec::default(),
        }
    }

    fn rkmpp_h265_spec() -> SourceSpec {
        SourceSpec {
            stream_id: "rk-h265".into(),
            capture: CaptureSpec {
                backend: "v4l2".into(),
                device: Some("/dev/video11".into()),
                width: 1920,
                height: 1080,
                fps: 30,
                pixel_format: "nv12".into(),
                prefer_dmabuf: false,
            },
            encoder: EncoderSpec {
                backend: "rkmpp".into(),
                codec: "h265".into(),
                bitrate: 4_000_000,
                profile: "main".into(),
                level: Some("4.0".into()),
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
            profile: "420028".into(),
            level: None,
            ..Default::default()
        };
        assert_eq!(enc.profile_level_id().unwrap(), "420028");
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
    fn test_h264_level_6_2_maps_to_62() {
        assert_eq!(h264_level_to_idc("6.2").unwrap(), 62);
    }

    #[test]
    fn test_h264_level_above_6_2_rejected() {
        assert!(h264_level_to_idc("6.3").is_err());
        assert!(h264_level_to_idc("18.6").is_err());
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

    // --- H.264 profile-level-id numeric tests ---

    #[test]
    fn test_h264_profile_420028() {
        let enc = EncoderSpec {
            profile: "420028".into(),
            codec: "h264".into(),
            level: None,
            ..Default::default()
        };
        let (pidc, lidc) = enc.h264_profile_level_id().unwrap();
        assert_eq!(pidc, 66); // Baseline
        assert_eq!(lidc, 40); // Level 4.0
    }

    #[test]
    fn test_h264_profile_4d0028() {
        let enc = EncoderSpec {
            profile: "4d0028".into(),
            codec: "h264".into(),
            level: None,
            ..Default::default()
        };
        let (pidc, _) = enc.h264_profile_level_id().unwrap();
        assert_eq!(pidc, 77); // Main
    }

    #[test]
    fn test_h264_profile_640028() {
        let enc = EncoderSpec {
            profile: "640028".into(),
            codec: "h264".into(),
            level: None,
            ..Default::default()
        };
        let (pidc, _) = enc.h264_profile_level_id().unwrap();
        assert_eq!(pidc, 100); // High
    }

    #[test]
    fn test_h264_with_separate_level_rejected() {
        let enc = EncoderSpec {
            profile: "640028".into(),
            codec: "h264".into(),
            level: Some("4.2".into()),
            ..Default::default()
        };
        assert!(enc.h264_profile_level_id().is_err());
    }

    #[test]
    fn test_h264_unsupported_profile_rejected() {
        let enc = EncoderSpec {
            profile: "580028".into(), // Extended
            codec: "h264".into(),
            level: None,
            ..Default::default()
        };
        assert!(enc.h264_profile_level_id().is_err());
    }

    // --- H.265 profile params tests ---

    #[test]
    fn test_h265_profile_main_valid() {
        let enc = EncoderSpec {
            profile: "main".into(),
            codec: "h265".into(),
            level: Some("4.0".into()),
            tier: None,
            ..Default::default()
        };
        let (pidc, tier, lidc) = enc.h265_profile_params().unwrap();
        assert_eq!(pidc, 1);
        assert_eq!(tier, 0);
        assert_eq!(lidc, 120);
    }

    #[test]
    fn test_h265_profile_empty_defaults() {
        let enc = EncoderSpec {
            profile: "".into(),
            codec: "h265".into(),
            level: None,
            tier: None,
            ..Default::default()
        };
        let (pidc, tier, lidc) = enc.h265_profile_params().unwrap();
        assert_eq!(pidc, 1);
        assert_eq!(tier, 0);
        assert_eq!(lidc, 120);
    }

    #[test]
    fn test_h265_hex_profile_rejected() {
        let enc = EncoderSpec {
            profile: "42001f".into(),
            codec: "h265".into(),
            level: Some("4.0".into()),
            tier: None,
            ..Default::default()
        };
        assert!(enc.h265_profile_params().is_err());
    }

    #[test]
    fn test_h265_bad_profile_rejected() {
        let enc = EncoderSpec {
            profile: "foo".into(),
            codec: "h265".into(),
            level: Some("4.0".into()),
            tier: None,
            ..Default::default()
        };
        assert!(enc.h265_profile_params().is_err());
    }

    #[test]
    fn test_h265_high_tier_rejected() {
        let enc = EncoderSpec {
            profile: "main".into(),
            codec: "h265".into(),
            level: Some("4.0".into()),
            tier: Some("high".into()),
            ..Default::default()
        };
        assert!(enc.h265_profile_params().is_err());
    }

    #[test]
    fn test_h265_level_4_0_maps_to_120() {
        assert_eq!(h265_level_to_idc("4.0").unwrap(), 120);
    }

    #[test]
    fn test_h265_level_4_1_maps_to_123() {
        assert_eq!(h265_level_to_idc("4.1").unwrap(), 123);
    }

    #[test]
    fn test_h265_level_unsupported() {
        assert!(h265_level_to_idc("6.0").is_err());
    }

    // --- SourceSpec validation tests (rkmpp codec gate) ---

    #[test]
    fn test_rkmpp_h264_valid() {
        let spec = rkmpp_spec();
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_rkmpp_h265_valid() {
        let spec = rkmpp_h265_spec();
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_rkmpp_invalid_codec_av1() {
        let mut spec = rkmpp_h265_spec();
        spec.encoder.codec = "av1".into();
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_rkmpp_invalid_codec_vp8() {
        let mut spec = rkmpp_h265_spec();
        spec.encoder.codec = "vp8".into();
        assert!(spec.validate().is_err());
    }

    // --- validate() / to_native_params() consistency tests ---
    // Both use resolve_profile(), so they must accept and reject the same
    // profile/level combinations for every backend, not just rkmpp.

    #[test]
    fn test_validate_v4l2_h265_name_profile_without_level() {
        // H.265 profile names do not require a separate level; validate()
        // must accept what to_native_params() accepts.
        let mut spec = v4l2_spec();
        spec.encoder.codec = "h265".into();
        spec.encoder.profile = "main".into();
        spec.encoder.level = None;
        assert!(spec.validate().is_ok());
    }

    #[test]
    fn test_validate_v4l2_h265_hex_profile_rejected() {
        // H.264-style hex profile-level-id is invalid for H.265 on every
        // backend — to_native_params() rejects it, so validate() must too.
        let mut spec = v4l2_spec();
        spec.encoder.codec = "h265".into();
        spec.encoder.profile = "42001f".into();
        spec.encoder.level = None;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn test_validate_v4l2_h265_h264_profile_name_rejected() {
        // 'high' is an H.264 profile name; it must not pass H.265 validation.
        let mut spec = v4l2_spec();
        spec.encoder.codec = "h265".into();
        spec.encoder.profile = "high".into();
        spec.encoder.level = Some("4.0".into());
        assert!(spec.validate().is_err());
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
    fn test_to_native_params_rkmpp_h264() {
        let params = rkmpp_spec().to_native_params().unwrap();
        assert_eq!(params.capture_backend, "v4l2");
        assert_eq!(params.encoder_backend, "rkmpp");
        assert_eq!(params.capture_pixel_format, 1); // nv12
        assert_eq!(params.codec, 100); // h264
        assert_eq!(params.profile, "640028");
        assert_eq!(params.profile_idc, 100);
        assert_eq!(params.level_idc, 40);
        assert_eq!(params.tier_flag, 0);
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_rkmpp_h265() {
        let params = rkmpp_h265_spec().to_native_params().unwrap();
        assert_eq!(params.capture_backend, "v4l2");
        assert_eq!(params.encoder_backend, "rkmpp");
        assert_eq!(params.capture_pixel_format, 1); // nv12
        assert_eq!(params.codec, 101); // h265
        assert_eq!(params.profile_idc, 1);
        assert_eq!(params.level_idc, 120);
        assert_eq!(params.tier_flag, 0);
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_rkmpp_h265_default_level() {
        // H.265 without an explicit level must pass validate() AND
        // to_native_params(), defaulting to Level 4.0 (120).
        let mut spec = rkmpp_h265_spec();
        spec.encoder.level = None;
        spec.validate().unwrap();
        let params = spec.to_native_params().unwrap();
        assert_eq!(params.profile_idc, 1);
        assert_eq!(params.level_idc, 120);
        assert_eq!(params.tier_flag, 0);
        // H.265 does not use the H.264-style profile-level-id string.
        assert_eq!(params.default_profile, "");
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_to_native_params_v4l2_h265_numerics() {
        // Non-rkmpp backends also need resolved H.265 numerics for the SDP
        // fmtp line — they must not fall back to 0/0/0.
        let mut spec = v4l2_spec();
        spec.encoder.codec = "h265".into();
        spec.encoder.profile = "main".into();
        spec.encoder.level = None;
        let params = spec.to_native_params().unwrap();
        assert_eq!(params.codec, 101); // h265
        assert_eq!(params.profile_idc, 1);
        assert_eq!(params.level_idc, 120);
        assert_eq!(params.tier_flag, 0);
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
