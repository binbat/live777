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

#[cfg(feature = "source")]
use super::stream_config_v2::SourceSpec;

#[cfg(feature = "source-rtp")]
use super::rtp_listener::RtpListenerSource;

#[cfg(any(
    feature = "livesrc-libcamera",
    feature = "source-libcamera",
    feature = "livesrc-v4l2",
    feature = "source-v4l2"
))]
use super::native_source::NativeSource;

/// Creates a `StreamSource` from a connection URL.
///
/// Intercepts new URL schemes (rtp://, exec://) and delegates
/// everything else to the existing `create_source_from_url` function.
#[cfg(feature = "source")]
pub async fn create_source_extended(
    url: &str,
    config: &SourceConfig,
) -> Result<Box<dyn StreamSource>> {
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
        #[cfg(any(feature = "livesrc-libcamera", feature = "source-libcamera"))]
        {
            let source = NativeSource::from_url(url, config)?;
            return Ok(Box::new(source));
        }
        #[cfg(not(any(feature = "livesrc-libcamera", feature = "source-libcamera")))]
        {
            anyhow::bail!(
                "Libcamera source feature not enabled. Recompile with feature 'livesrc-libcamera'"
            );
        }
    }

    // Check for V4L2 Direct Capture scheme
    if url_lower.starts_with("v4l2://") {
        #[cfg(any(feature = "livesrc-v4l2", feature = "source-v4l2"))]
        {
            let source = NativeSource::from_url(url, config)?;
            return Ok(Box::new(source));
        }
        #[cfg(not(any(feature = "livesrc-v4l2", feature = "source-v4l2")))]
        {
            anyhow::bail!("V4L2 source requires feature 'livesrc-v4l2'");
        }
    }

    // Delegate to existing, unmodified factory for legacy schemes (rtsp://, file://, .sdp)
    super::create_legacy_source_from_url(url, config).await
}

#[cfg(not(feature = "source"))]
pub async fn create_source_extended(
    _url: &str,
    _config: &crate::config::SourceConfig,
) -> Result<Box<dyn StreamSource>> {
    anyhow::bail!("Source feature not enabled")
}

/// Create a `StreamSource` directly from a structured [`SourceSpec`],
/// bypassing the legacy URL roundtrip.
///
/// Routes based on `spec.kind`:
/// - `V4l2` → `V4L2Source::from_spec()`
/// - `Libcamera` → `LibcameraSource::from_spec()`
/// - `Rtp`, `Rtsp`, `Sdp` → fall back to legacy URL conversion.
#[cfg(feature = "source")]
pub async fn create_source_from_spec(spec: &SourceSpec) -> Result<Box<dyn StreamSource>> {
    match spec.kind {
        super::stream_config_v2::SourceKind::V4l2 => {
            #[cfg(any(feature = "livesrc-v4l2", feature = "source-v4l2"))]
            {
                return Ok(Box::new(NativeSource::from_spec(spec)?));
            }
            #[cfg(not(any(feature = "livesrc-v4l2", feature = "source-v4l2")))]
            anyhow::bail!("V4L2 source requires feature 'livesrc-v4l2'");
        }
        super::stream_config_v2::SourceKind::Libcamera => {
            #[cfg(any(feature = "livesrc-libcamera", feature = "source-libcamera"))]
            {
                return Ok(Box::new(NativeSource::from_spec(spec)?));
            }
            #[cfg(not(any(feature = "livesrc-libcamera", feature = "source-libcamera")))]
            anyhow::bail!("Libcamera source requires feature 'livesrc-libcamera'");
        }
        // For Rtp, Rtsp, Sdp: fall back through the extended router so
        // that rtp:// is handled before delegating to the legacy path.
        _ => {
            let config = spec.to_legacy_source_config();
            create_source_extended(&config.url, &config).await
        }
    }
}

#[cfg(not(feature = "source"))]
pub async fn create_source_from_spec(
    _spec: &super::stream_config_v2::SourceSpec,
) -> Result<Box<dyn StreamSource>> {
    anyhow::bail!("Source feature not enabled")
}
