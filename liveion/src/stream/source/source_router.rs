//! Extended Source Router
//!
//! Creates `StreamSource` instances from configs and structured
//! `SourceSpec` entries.

use super::StreamSource;
use anyhow::Result;

#[cfg(feature = "source")]
use crate::config::SourceConfig;

#[cfg(feature = "native-source")]
use super::native_source::NativeSource;

#[cfg(feature = "source")]
use super::source_config::SourceSpec;

/// Creates a `StreamSource` from a connection URL.
///
/// Delegates rtsp://, file://, .sdp to the URL-based source factory.
#[cfg(feature = "source")]
pub async fn create_source_extended(
    url: &str,
    config: &SourceConfig,
) -> Result<Box<dyn StreamSource>> {
    super::create_url_source(url, config).await
}

#[cfg(not(feature = "source"))]
pub async fn create_source_extended(
    _url: &str,
    _config: &crate::config::SourceConfig,
) -> Result<Box<dyn StreamSource>> {
    anyhow::bail!("Source feature not enabled")
}

/// Create a `StreamSource` from a structured [`SourceSpec`].
#[cfg(feature = "source")]
pub async fn create_source_from_spec(spec: &SourceSpec) -> Result<Box<dyn StreamSource>> {
    match spec.kind {
        #[cfg(feature = "native-source")]
        super::source_config::SourceKind::V4l2 | super::source_config::SourceKind::Libcamera => {
            Ok(Box::new(NativeSource::from_spec(spec)?))
        }
        #[cfg(not(feature = "native-source"))]
        _ => anyhow::bail!("Native source feature not enabled. Use a native-* preset."),
    }
}

#[cfg(not(feature = "source"))]
pub async fn create_source_from_spec(
    _spec: &super::source_config::SourceSpec,
) -> Result<Box<dyn StreamSource>> {
    anyhow::bail!("Source feature not enabled")
}
