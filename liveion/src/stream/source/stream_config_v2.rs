//! Stream configuration v2
//!
//! New configuration format that coexists with the existing `SourceConfig`.
//! Supports URL-based source specification, daemon policies, and recording policies.
//!
//! Example TOML:
//! ```toml
//! [streams.my_camera]
//! source = "rtp://0.0.0.0:5004"
//! daemon = "always"
//!
//! [streams.my_camera]
//! source = "rtp://0.0.0.0:5004"
//! daemon = "always"
//!
//! [streams.remote_feed]
//! source = "rtsp://admin:pass@192.168.1.100:554/stream1"
//! daemon = "auto"
//! record = "always"
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::lifecycle::DaemonPolicy;

/// Configuration for a single stream (v2 format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEntryConfig {
    /// Source URL. Supported schemes:
    /// - `rtp://host:port` — Listen for incoming RTP on a UDP port
    /// - `rtp://host:port` — Listen for incoming RTP on a UDP port
    /// - `rtsp://user:pass@host:port/path` — RTSP client pull
    /// - `file:///path/to/file.sdp` or `/path/to.sdp` — SDP file source
    pub source: String,

    /// Daemon policy: when to keep the source running.
    /// - `"always"` (default): source runs regardless of subscribers
    /// - `"auto"`: source starts when subscribers arrive, stops when they leave
    #[serde(default = "default_daemon")]
    pub daemon: String,

    /// Recording policy (optional).
    /// - `"auto"`: record when the stream is active
    /// - `"always"`: always record
    /// - `null` / absent: no recording
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
            || src.starts_with("rtsp://")
            || src.starts_with("rtsps://")
            || src.starts_with("file://")
            || src.ends_with(".sdp");

        if !valid {
            anyhow::bail!(
                "Unsupported source URL: {}. Valid schemes: rtp://, libcamera://, rtsp://, file://, .sdp",
                self.source
            );
        }

        // Validate daemon value
        match self.daemon.to_lowercase().as_str() {
            "auto" | "always" => {}
            other => anyhow::bail!(
                "Invalid daemon value: '{}'. Must be 'auto' or 'always'",
                other
            ),
        }

        // Validate record value
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

    /// Convert the v2 config to the existing SourceConfig format for backward compatibility.
    pub fn to_legacy_source_config(&self, stream_id: &str) -> crate::config::SourceConfig {
        crate::config::SourceConfig {
            stream_id: stream_id.to_string(),
            url: self.source.clone(),
        }
    }
}

/// Top-level streams configuration (v2 format).
///
/// This sits alongside the existing `StreamConfig` in the config file.
/// Both formats can coexist in the same TOML file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamsConfigV2 {
    /// Map of stream_id -> stream configuration
    #[serde(default)]
    pub streams: HashMap<String, StreamEntryConfig>,
}

impl StreamsConfigV2 {
    /// Validate all stream configurations.
    pub fn validate(&self) -> anyhow::Result<()> {
        for (stream_id, config) in &self.streams {
            config
                .validate()
                .map_err(|e| anyhow::anyhow!("Stream '{}': {}", stream_id, e))?;
        }
        Ok(())
    }

    /// Convert all v2 configs to legacy SourceConfig format.
    pub fn to_legacy_configs(&self) -> Vec<crate::config::SourceConfig> {
        self.streams
            .iter()
            .map(|(id, cfg)| cfg.to_legacy_source_config(id))
            .collect()
    }
}


/// Parse the `libcamera://` URL.
///
/// Supports query parameters for width, height, fps, bitrate, etc.
/// Format: `libcamera:///path/to/bin?width=640&height=480&fps=30&bitrate=2000000`
pub fn parse_libcamera_url(url: &str) -> anyhow::Result<LibcameraUrlParams> {
    let stripped = url.strip_prefix("libcamera://").ok_or_else(|| anyhow::anyhow!("URL must start with libcamera://"))?;
    let (addr_part, query_part) = match stripped.find('?') {
        Some(idx) => (&stripped[..idx], Some(&stripped[idx + 1..])),
        None => (stripped, None),
    };

    let mut width = 1280;
    let mut height = 720;
    let mut fps = 30;
    let mut bitrate = 2_000_000;
    let mut camera_id = 0;
    let mut rotation = 0;
    let mut hflip = false;
    let mut vflip = false;
    let mut codec = "H264".to_string();
    let mut profile = "42001f".to_string();
    let clock_rate = 90000;
    let mut payload_type = 96;

    if let Some(query) = query_part {
        for param in query.split('&') {
            let (key, value) = match param.find('=') {
                Some(idx) => (&param[..idx], &param[idx + 1..]),
                None => continue,
            };

            match key {
                "width" | "w" => width = value.parse().unwrap_or(1280),
                "height" | "h" => height = value.parse().unwrap_or(720),
                "fps" | "f" => fps = value.parse().unwrap_or(30),
                "bitrate" | "b" => bitrate = value.parse().unwrap_or(2_000_000),
                "camera" | "c" => camera_id = value.parse().unwrap_or(0),
                "rotation" | "r" => rotation = value.parse().unwrap_or(0),
                "hflip" => hflip = value == "true" || value == "1",
                "vflip" => vflip = value == "true" || value == "1",
                "codec" => codec = value.to_uppercase(),
                "profile" => profile = value.into(),
                "pt" => payload_type = value.parse().unwrap_or(96),
                _ => {}
            }
        }
    }

    Ok(LibcameraUrlParams {
        placeholder_path: addr_part.to_string(),
        width,
        height,
        fps,
        bitrate,
        camera_id,
        rotation,
        hflip,
        vflip,
        codec,
        profile,
        clock_rate,
        payload_type,
    })
}

/// Parsed parameters from a `libcamera://` URL.
#[derive(Debug, Clone)]
pub struct LibcameraUrlParams {
    pub placeholder_path: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub camera_id: u32,
    pub rotation: u32,
    pub hflip: bool,
    pub vflip: bool,
    pub codec: String,
    pub profile: String,
    pub clock_rate: u32,
    pub payload_type: u8,
}

/// Parsed parameters from an `exec://` URL.
#[derive(Debug, Clone)]
pub struct ExecUrlParams {
    pub executable: String,
    pub args: Vec<String>,
    pub codec: String,
    pub profile: String,
    pub clock_rate: u32,
    pub payload_type: u8,
}

/// Parse a `rtp://` URL to extract bind address.
///
/// Format: `rtp://host:port`
///
/// Query parameters:
/// - `codec`: video codec name (default: "H264")
/// - `profile`: H.264 profile-level-id (default: "42001f")
/// - `clock_rate`: RTP clock rate (default: 90000)
/// - `payload_type`: RTP payload type (default: 96)
pub fn parse_rtp_url(url: &str) -> anyhow::Result<RtpUrlParams> {
    let stripped = url
        .strip_prefix("rtp://")
        .ok_or_else(|| anyhow::anyhow!("URL must start with rtp://"))?;

    let (addr_part, query_part) = match stripped.find('?') {
        Some(idx) => (&stripped[..idx], Some(&stripped[idx + 1..])),
        None => (stripped, None),
    };

    let bind_addr: std::net::SocketAddr = addr_part
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid bind address '{}': {}", addr_part, e))?;

    let mut codec = "H264".to_string();
    let mut profile = "42001f".to_string();
    let mut clock_rate: u32 = 90000;
    let mut payload_type: u8 = 96;

    if let Some(query) = query_part {
        for param in query.split('&') {
            let (key, value) = match param.find('=') {
                Some(idx) => (&param[..idx], &param[idx + 1..]),
                None => continue,
            };

            match key {
                "codec" => codec = value.to_uppercase(),
                "profile" => profile = value.to_string(),
                "clock_rate" => clock_rate = value.parse().unwrap_or(90000),
                "payload_type" | "pt" => payload_type = value.parse().unwrap_or(96),
                _ => {
                    tracing::warn!("Unknown rtp:// query parameter: {}={}", key, value);
                }
            }
        }
    }

    Ok(RtpUrlParams {
        bind_addr,
        codec,
        profile,
        clock_rate,
        payload_type,
    })
}

/// Parsed parameters from an `rtp://` URL.
#[derive(Debug, Clone)]
pub struct RtpUrlParams {
    pub bind_addr: std::net::SocketAddr,
    pub codec: String,
    pub profile: String,
    pub clock_rate: u32,
    pub payload_type: u8,
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn test_parse_rtp_url() {
        let params = parse_rtp_url("rtp://0.0.0.0:5004").unwrap();
        assert_eq!(params.bind_addr.port(), 5004);
        assert_eq!(params.codec, "H264");
        assert_eq!(params.payload_type, 96);
    }

    #[test]
    fn test_parse_rtp_url_with_query() {
        let params =
            parse_rtp_url("rtp://0.0.0.0:5004?codec=H265&pt=97&clock_rate=90000").unwrap();
        assert_eq!(params.codec, "H265");
        assert_eq!(params.payload_type, 97);
    }

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
}
