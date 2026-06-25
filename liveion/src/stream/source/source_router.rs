//! Extended Source Router
//!
//! Creates `StreamSource` instances from configs and structured
//! `SourceSpec` entries.

use super::StreamSource;
use anyhow::Result;

#[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
use crate::config::SourceConfig;

#[cfg(feature = "native-source")]
use super::native_source::NativeSource;

#[cfg(feature = "native-source")]
use super::source_config::SourceSpec;

/// Creates a `StreamSource` from a connection URL.
///
/// Delegates rtsp://, file://, .sdp to the URL-based source factory.
#[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
pub async fn create_source_extended(
    stream_id: &str,
    url: &str,
    config: &SourceConfig,
) -> Result<Box<dyn StreamSource>> {
    super::create_url_source(stream_id, url, config).await
}

#[cfg(not(any(feature = "source-rtsp", feature = "source-sdp")))]
pub async fn create_source_extended(
    _stream_id: &str,
    _url: &str,
    _config: &crate::config::SourceConfig,
) -> Result<Box<dyn StreamSource>> {
    anyhow::bail!("URL-based sources require source-rtsp or source-sdp feature")
}

/// Create a `StreamSource` from a structured [`SourceSpec`].
#[cfg(feature = "native-source")]
pub async fn create_source_from_spec(spec: &SourceSpec) -> Result<Box<dyn StreamSource>> {
    Ok(Box::new(NativeSource::from_spec(spec)?))
}
