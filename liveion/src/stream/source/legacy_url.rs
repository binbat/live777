//! Legacy URL-based source configuration parsers.
//!
//! These functions parse the old URL query-string format (e.g.
//! `v4l2:///dev/video0?width=640&height=480&fps=30`).
//!
//! They are **deprecated** in favour of the structured `[[stream.sources]]`
//! TOML configuration.  Existing call sites continue to work but emit
//! a compile-time deprecation warning and a run-time `tracing::warn!`.

use tracing::warn;

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

/// Parsed parameters from a `v4l2://` URL.
#[derive(Debug, Clone)]
pub struct V4L2UrlParams {
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub profile: String,
    pub clock_rate: u32,
    pub payload_type: u8,
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

// ---------------------------------------------------------------------------
// Legacy URL parsers (deprecated — use structured TOML config)
// ---------------------------------------------------------------------------

/// Parse a `libcamera://` URL.
///
/// Format: `libcamera:///path/to/bin?width=640&height=480&fps=30&bitrate=2000000`
///
/// Note: this function is the internal compatibility path for legacy URL-style
/// configs.  New code should use the structured `[[stream.sources]]` TOML format.
pub fn parse_libcamera_url(url: &str) -> anyhow::Result<LibcameraUrlParams> {
    warn!("legacy URL format is deprecated: use structured [[stream.sources]] config");

    let stripped = url
        .strip_prefix("libcamera://")
        .ok_or_else(|| anyhow::anyhow!("URL must start with libcamera://"))?;
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

/// Parse a `v4l2://` URL for direct V4L2 capture.
///
/// Format: `v4l2:///dev/video2?width=640&height=480&fps=30&bitrate=2000000`
///
/// Note: this function is the internal compatibility path for legacy URL-style
/// configs.  New code should use the structured `[[stream.sources]]` TOML format.
pub fn parse_v4l2_url(url: &str) -> anyhow::Result<V4L2UrlParams> {
    warn!("legacy URL format is deprecated: use structured [[stream.sources]] config");

    let stripped = url
        .strip_prefix("v4l2://")
        .ok_or_else(|| anyhow::anyhow!("URL must start with v4l2://"))?;
    let (device_part, query_part) = match stripped.find('?') {
        Some(idx) => (&stripped[..idx], Some(&stripped[idx + 1..])),
        None => (stripped, None),
    };

    let device = if device_part.is_empty() {
        "/dev/video2".to_string()
    } else {
        device_part.to_string()
    };

    let mut width: u32 = 640;
    let mut height: u32 = 480;
    let mut fps: u32 = 30;
    let mut bitrate: u32 = 2_000_000;
    let mut profile = "42001f".to_string();
    let clock_rate: u32 = 90000;
    let mut payload_type: u8 = 96;

    if let Some(query) = query_part {
        for param in query.split('&') {
            let (key, value) = match param.find('=') {
                Some(idx) => (&param[..idx], &param[idx + 1..]),
                None => continue,
            };
            match key {
                "width" | "w" => width = value.parse().unwrap_or(640),
                "height" | "h" => height = value.parse().unwrap_or(480),
                "fps" | "f" => fps = value.parse().unwrap_or(30),
                "bitrate" | "b" => bitrate = value.parse().unwrap_or(2_000_000),
                "profile" => profile = value.into(),
                "pt" => payload_type = value.parse().unwrap_or(96),
                _ => {}
            }
        }
    }

    Ok(V4L2UrlParams {
        device,
        width,
        height,
        fps,
        bitrate,
        profile,
        clock_rate,
        payload_type,
    })
}

/// Parse a `rtp://` URL to extract bind address.
///
/// Format: `rtp://host:port`
///
/// Note: this function is the internal compatibility path for legacy URL-style
/// configs.  New code should use the structured `[[stream.sources]]` TOML format.
pub fn parse_rtp_url(url: &str) -> anyhow::Result<RtpUrlParams> {
    warn!("legacy URL format is deprecated: use structured [[stream.sources]] config");

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
        let params = parse_rtp_url("rtp://0.0.0.0:5004?codec=H265&pt=97&clock_rate=90000").unwrap();
        assert_eq!(params.codec, "H265");
        assert_eq!(params.payload_type, 97);
    }

    #[test]
    fn test_parse_v4l2_url_defaults() {
        let params = parse_v4l2_url("v4l2:///dev/video0").unwrap();
        assert_eq!(params.device, "/dev/video0");
        assert_eq!(params.width, 640);
        assert_eq!(params.height, 480);
        assert_eq!(params.fps, 30);
    }

    #[test]
    fn test_parse_v4l2_url_with_query() {
        let params =
            parse_v4l2_url("v4l2:///dev/video3?width=1920&height=1080&fps=60&bitrate=4000000")
                .unwrap();
        assert_eq!(params.device, "/dev/video3");
        assert_eq!(params.width, 1920);
        assert_eq!(params.height, 1080);
        assert_eq!(params.fps, 60);
        assert_eq!(params.bitrate, 4_000_000);
    }

    #[test]
    fn test_parse_libcamera_url_defaults() {
        let params = parse_libcamera_url("libcamera://").unwrap();
        assert_eq!(params.width, 1280);
        assert_eq!(params.height, 720);
        assert_eq!(params.fps, 30);
        assert_eq!(params.bitrate, 2_000_000);
    }
}
