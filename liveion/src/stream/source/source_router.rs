//! Extended Source Router
//!
//! Wraps the existing `create_source_from_url` and adds support for
//! the new v2 URL schemes (`rtp://` and `exec://`).
//!
//! This acts as an adapter layer to maintain backward compatibility
//! while adding new source types incrementally.

use super::StreamSource;
use anyhow::Result;

#[cfg(feature = "source")]
use crate::config::SourceConfig;

#[cfg(feature = "source-rtp")]
use super::rtp_listener::RtpListenerSource;

#[cfg(feature = "source-libcamera")]
use super::libcamera_source::LibcameraSource;

/// Creates a `StreamSource` from a connection URL.
/// 
/// Intercepts new URL schemes (rtp://, exec://) and delegates
/// everything else to the existing `create_source_from_url` function.
#[cfg(feature = "source")]
pub async fn create_source_extended(url: &str, config: &SourceConfig) -> Result<Box<dyn StreamSource>> {
    let url_lower = url.to_lowercase();

    // Check for RTP Listener scheme
    if url_lower.starts_with("rtp://") {
        #[cfg(feature = "source-rtp")]
        {
            let source = RtpListenerSource::from_url(url, config)?;
            return Ok(Box::new(source));
        }
        #[cfg(not(feature = "source-rtp"))]
        {
            anyhow::bail!("RTP source feature not enabled. Recompile with feature 'source-rtp'");
        }
    }

    // Check for Libcamera-Bridge scheme
    if url_lower.starts_with("libcamera://") {
        #[cfg(feature = "source-libcamera")]
        {
            let source = LibcameraSource::from_url(url, config)?;
            return Ok(Box::new(source));
        }
        #[cfg(not(feature = "source-libcamera"))]
        {
            anyhow::bail!("Libcamera source feature not enabled. Recompile with feature 'source-libcamera'");
        }
    }

    // Check for V4L2 Direct Capture scheme
    if url_lower.starts_with("v4l2://") {
        #[cfg(feature = "source-libcamera")]
        {
            use super::v4l2_source::V4L2Source;
            let source = V4L2Source::from_url(url, config)?;
            return Ok(Box::new(source));
        }
        #[cfg(not(feature = "source-libcamera"))]
        {
            anyhow::bail!("V4L2 source requires feature 'source-libcamera' (shared hardware encoder)");
        }
    }

    // Delegate to existing, unmodified factory for legacy schemes (rtsp://, file://, .sdp)
    super::create_legacy_source_from_url(url, config).await
}

#[cfg(not(feature = "source"))]
pub async fn create_source_extended(_url: &str, _config: &crate::config::SourceConfig) -> Result<Box<dyn StreamSource>> {
    anyhow::bail!("Source feature not enabled")
}
