//! Stream configuration v2
//!
//! Supports two coexisting configuration formats:
//!
//! 1. **Structured TOML** (recommended): `[[stream.sources]]` with explicit
//!    `capture`, `encoder`, and `output` sections.
//!
//! 2. **Legacy URL** (deprecated): URL-based config with query parameters.
//!    Parsers live in [`legacy_url`](super::legacy_url).
//!
//! # Structured TOML example
//!
//! ```toml
//! [stream]
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
//! backend = "rdk_x5"
//! codec = "h264"
//! bitrate = 1_500_000
//! profile = "42001f"
//! gop = 60
//!
//! [stream.sources.output]
//! payload_type = 96
//! clock_rate = 90000
//! ```
//!
//! # Legacy URL example (deprecated)
//!
//! ```toml
//! [[stream.sources]]
//! stream_id = "usbcam"
//! url = "v4l2:///dev/video0?width=640&height=480&fps=30&bitrate=1500000"
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::lifecycle::DaemonPolicy;

// ---------------------------------------------------------------------------
// Re-export legacy URL parsers for backward compatibility
// ---------------------------------------------------------------------------
pub use super::legacy_url::{
    LibcameraUrlParams, RtpUrlParams, V4L2UrlParams, parse_libcamera_url, parse_rtp_url,
    parse_v4l2_url,
};

// ---------------------------------------------------------------------------
// Structured source configuration types (v2 — recommended)
// ---------------------------------------------------------------------------

/// Identifies the type of media source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    V4l2,
    Libcamera,
    Rtp,
    Rtsp,
    Sdp,
}

/// Capture (input device) specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureSpec {
    /// Capture backend: `"v4l2"` or `"libcamera"`.
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
    /// Encoder backend: `"v4l2_m2m"` or `"rdk_x5"`.
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
/// This is the recommended structured config format replacing the legacy
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
        if self.encoder.bitrate == 0 {
            anyhow::bail!("encoder.bitrate must be non-zero");
        }
        Ok(())
    }

    /// Convert to the legacy `SourceConfig` for backward compatibility.
    pub fn to_legacy_source_config(&self) -> crate::config::SourceConfig {
        // Build a legacy URL from structured fields so existing code paths
        // work without changes.
        let url = match self.kind {
            SourceKind::V4l2 => format!(
                "v4l2://{}?width={}&height={}&fps={}&bitrate={}&profile={}&pt={}",
                self.capture.device,
                self.capture.width,
                self.capture.height,
                self.capture.fps,
                self.encoder.bitrate,
                self.encoder.profile,
                self.output.payload_type,
            ),
            SourceKind::Libcamera => format!(
                "libcamera://{}?width={}&height={}&fps={}&bitrate={}&profile={}&pt={}",
                self.capture.device,
                self.capture.width,
                self.capture.height,
                self.capture.fps,
                self.encoder.bitrate,
                self.encoder.profile,
                self.output.payload_type,
            ),
            SourceKind::Rtp => format!(
                "rtp://{}?codec={}&pt={}&clock_rate={}",
                self.capture.device,
                self.encoder.codec,
                self.output.payload_type,
                self.output.clock_rate,
            ),
            _ => format!(
                "{}://{}?width={}&height={}&fps={}&bitrate={}",
                serde_json::to_string(&self.kind)
                    .unwrap_or_default()
                    .trim_matches('"'),
                self.capture.device,
                self.capture.width,
                self.capture.height,
                self.capture.fps,
                self.encoder.bitrate,
            ),
        };

        crate::config::SourceConfig {
            stream_id: self.stream_id.clone(),
            url,
        }
    }
}

// ---------------------------------------------------------------------------
// Legacy StreamEntryConfig (deprecated URL-based format)
// ---------------------------------------------------------------------------

/// Configuration for a single stream (legacy URL-based format).
///
/// Kept for backward compatibility.  Prefer [`SourceSpec`] for new configs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEntryConfig {
    /// Source URL. Supported schemes:
    /// - `rtp://host:port` — Listen for incoming RTP on a UDP port
    /// - `rtsp://user:pass@host:port/path` — RTSP client pull
    /// - `file:///path/to/file.sdp` or `/path/to.sdp` — SDP file source
    /// - `libcamera://` — libcamera-bridge capture
    /// - `v4l2:///dev/videoN` — V4L2 direct capture
    pub source: String,

    /// Daemon policy.
    #[serde(default = "default_daemon")]
    pub daemon: String,

    /// Recording policy (optional).
    #[serde(default)]
    pub record: Option<String>,
}

fn default_daemon() -> String {
    "always".to_string()
}

impl StreamEntryConfig {
    /// Parse the daemon policy from the string configuration.
    pub fn daemon_policy(&self) -> DaemonPolicy {
        match self.daemon.to_lowercase().as_str() {
            "auto" => DaemonPolicy::Auto,
            _ => DaemonPolicy::Always,
        }
    }

    /// Validate the stream configuration.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.source.trim().is_empty() {
            anyhow::bail!("source URL cannot be empty");
        }

        let src = self.source.to_lowercase();
        let valid = src.starts_with("rtp://")
            || src.starts_with("libcamera://")
            || src.starts_with("v4l2://")
            || src.starts_with("rtsp://")
            || src.starts_with("rtsps://")
            || src.starts_with("file://")
            || src.ends_with(".sdp");

        if !valid {
            anyhow::bail!(
                "Unsupported source URL: {}. Valid schemes: rtp://, libcamera://, v4l2://, rtsp://, file://, .sdp",
                self.source
            );
        }

        match self.daemon.to_lowercase().as_str() {
            "auto" | "always" => {}
            other => anyhow::bail!(
                "Invalid daemon value: '{}'. Must be 'auto' or 'always'",
                other
            ),
        }

        if let Some(ref record) = self.record {
            match record.to_lowercase().as_str() {
                "auto" | "always" => {}
                other => anyhow::bail!(
                    "Invalid record value: '{}'. Must be 'auto' or 'always'",
                    other
                ),
            }
        }

        Ok(())
    }

    /// Convert the legacy config to the existing SourceConfig format.
    pub fn to_legacy_source_config(&self, stream_id: &str) -> crate::config::SourceConfig {
        crate::config::SourceConfig {
            stream_id: stream_id.to_string(),
            url: self.source.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level streams config (v2)
// ---------------------------------------------------------------------------

/// Top-level streams configuration.
///
/// Supports both the legacy `[streams.<id>]` format and the new
/// structured `[[stream.sources]]` format in the same TOML file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamsConfigV2 {
    /// Map of stream_id -> stream configuration (legacy format).
    #[serde(default)]
    pub streams: HashMap<String, StreamEntryConfig>,

    /// Structured source specifications (new format, recommended).
    #[serde(default)]
    pub sources: Vec<SourceSpec>,
}

impl StreamsConfigV2 {
    /// Validate all stream configurations.
    pub fn validate(&self) -> anyhow::Result<()> {
        for (stream_id, config) in &self.streams {
            config
                .validate()
                .map_err(|e| anyhow::anyhow!("Stream '{}': {}", stream_id, e))?;
        }
        for source in &self.sources {
            source
                .validate()
                .map_err(|e| anyhow::anyhow!("Source '{}': {}", source.stream_id, e))?;
        }
        Ok(())
    }

    /// Convert both legacy and structured configs to legacy SourceConfig format.
    pub fn to_legacy_configs(&self) -> Vec<crate::config::SourceConfig> {
        let mut configs: Vec<_> = self
            .streams
            .iter()
            .map(|(id, cfg)| cfg.to_legacy_source_config(id))
            .collect();

        configs.extend(self.sources.iter().map(|s| s.to_legacy_source_config()));

        configs
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Legacy StreamEntryConfig tests ---

    #[test]
    fn test_stream_entry_config_validate() {
        let cfg = StreamEntryConfig {
            source: "rtp://0.0.0.0:5004".to_string(),
            daemon: "always".to_string(),
            record: None,
        };
        assert!(cfg.validate().is_ok());

        let cfg = StreamEntryConfig {
            source: "".to_string(),
            daemon: "always".to_string(),
            record: None,
        };
        assert!(cfg.validate().is_err());

        let cfg = StreamEntryConfig {
            source: "rtp://0.0.0.0:5004".to_string(),
            daemon: "invalid".to_string(),
            record: None,
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_daemon_policy_parsing() {
        let cfg = StreamEntryConfig {
            source: "rtp://0.0.0.0:5004".to_string(),
            daemon: "auto".to_string(),
            record: None,
        };
        assert_eq!(cfg.daemon_policy(), DaemonPolicy::Auto);

        let cfg = StreamEntryConfig {
            source: "rtp://0.0.0.0:5004".to_string(),
            daemon: "always".to_string(),
            record: None,
        };
        assert_eq!(cfg.daemon_policy(), DaemonPolicy::Always);
    }

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
                backend: "v4l2_m2m".into(),
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
                backend: "v4l2_m2m".into(),
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
                backend: "v4l2_m2m".into(),
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
    fn test_source_spec_to_legacy_conversion() {
        let spec = SourceSpec {
            stream_id: "cam1".into(),
            kind: SourceKind::V4l2,
            capture: CaptureSpec {
                backend: "v4l2".into(),
                device: "/dev/video3".into(),
                width: 1920,
                height: 1080,
                fps: 60,
                pixel_format: "yuyv".into(),
                prefer_dmabuf: false,
            },
            encoder: EncoderSpec {
                backend: "rdk_x5".into(),
                codec: "h264".into(),
                bitrate: 4_000_000,
                profile: "42001f".into(),
                gop: 60,
                prefer_dmabuf: false,
            },
            output: OutputSpec::default(),
        };
        let legacy = spec.to_legacy_source_config();
        assert_eq!(legacy.stream_id, "cam1");
        assert!(legacy.url.starts_with("v4l2:///dev/video3?"));
        assert!(legacy.url.contains("width=1920"));
        assert!(legacy.url.contains("bitrate=4000000"));
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

    #[test]
    fn test_streams_config_v2_with_structured_sources() {
        let config = StreamsConfigV2 {
            streams: HashMap::new(),
            sources: vec![SourceSpec {
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
                    backend: "v4l2_m2m".into(),
                    codec: "h264".into(),
                    bitrate: 1_000_000,
                    profile: "42001f".into(),
                    gop: 60,
                    prefer_dmabuf: false,
                },
                output: OutputSpec::default(),
            }],
        };
        assert!(config.validate().is_ok());
        let legacy = config.to_legacy_configs();
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0].stream_id, "cam1");
    }
}
