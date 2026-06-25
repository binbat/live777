use anyhow::Result;
use async_trait::async_trait;
#[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
use bytes::Bytes;
#[cfg(feature = "native-source")]
use rtc::rtp::packet::Packet;
#[cfg(feature = "native-source")]
use std::sync::Arc;
use tokio::sync::broadcast;

#[cfg(feature = "source-rtsp")]
mod rtsp_source;
#[cfg(feature = "source-sdp")]
mod sdp_source;

#[cfg(feature = "native-source")]
pub mod native_encoded_source;
#[cfg(feature = "native-source")]
pub mod source_config;
pub mod source_router;

pub mod manager;
#[cfg(feature = "native-source")]
pub mod native_source;

#[cfg(feature = "source-rtsp")]
pub use rtsp_source::RtspSource;
#[cfg(feature = "source-sdp")]
pub use sdp_source::SdpSource;

pub use manager::SourceManager;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StreamSourceState {
    Initializing,
    Connected,
    Disconnected,
    Reconnecting,
    Error,
}

#[derive(Debug, Clone)]
pub struct StateChangeEvent {
    pub old_state: StreamSourceState,
    pub new_state: StreamSourceState,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum MediaPacket {
    #[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
    Rtp { channel: u8, data: Bytes },
    #[cfg(feature = "native-source")]
    RtpPacket(Arc<Packet>),
    // Placeholder when no concrete source implementation is enabled.
    // The `source` feature alone has no active source types, so this
    // variant keeps the enum non-empty without exposing real data.
    #[cfg(not(any(
        feature = "source-rtsp",
        feature = "source-sdp",
        feature = "native-source"
    )))]
    _Unused,
}

#[derive(Debug, Clone)]
#[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
pub struct InternalSourceConfig {
    pub stream_id: String,
    pub url: String,
}

#[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
impl InternalSourceConfig {
    pub fn from_config(stream_id: &str, config: &crate::config::SourceConfig) -> Self {
        Self {
            stream_id: stream_id.to_string(),
            url: config.url.clone().unwrap_or_default(),
        }
    }

    pub fn reconnect_enabled(&self) -> bool {
        self.url.starts_with("rtsp://") || self.url.starts_with("rtsps://")
    }

    pub fn reconnect_interval_ms(&self) -> u64 {
        5000
    }

    pub fn max_reconnect_attempts(&self) -> u32 {
        0
    }
}

#[async_trait]
pub trait StreamSource: Send + Sync {
    fn stream_id(&self) -> &str;

    fn state(&self) -> StreamSourceState;

    async fn start(&mut self) -> Result<()>;

    async fn stop(&mut self) -> Result<()>;

    fn subscribe_rtp(&self) -> broadcast::Receiver<MediaPacket>;

    fn subscribe_state(&self) -> broadcast::Receiver<StateChangeEvent>;

    #[cfg(feature = "source")]
    async fn get_video_codec(
        &self,
    ) -> Option<rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters> {
        None
    }

    #[cfg(feature = "source")]
    async fn get_audio_codec(
        &self,
    ) -> Option<rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters> {
        None
    }

    #[cfg(feature = "source")]
    async fn get_rtcp_sender(&self) -> Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>> {
        None
    }
}

pub async fn create_source_from_url(
    stream_id: &str,
    url: &str,
    config: &crate::config::SourceConfig,
) -> Result<Box<dyn StreamSource>> {
    source_router::create_source_extended(stream_id, url, config).await
}

#[cfg(feature = "native-source")]
pub async fn create_source_from_spec(
    spec: &source_config::SourceSpec,
) -> Result<Box<dyn StreamSource>> {
    source_router::create_source_from_spec(spec).await
}

#[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
pub(crate) async fn create_url_source(
    stream_id: &str,
    url: &str,
    config: &crate::config::SourceConfig,
) -> Result<Box<dyn StreamSource>> {
    let internal_config = InternalSourceConfig::from_config(stream_id, config);

    if url.starts_with("rtsp://") || url.starts_with("rtsps://") {
        #[cfg(feature = "source-rtsp")]
        {
            let source = RtspSource::new(internal_config, url.to_string())?;
            Ok(Box::new(source))
        }

        #[cfg(not(feature = "source-rtsp"))]
        {
            anyhow::bail!("RTSP source support not enabled. Enable 'source-rtsp' feature.");
        }
    } else if url.starts_with("file://") || url.ends_with(".sdp") {
        #[cfg(feature = "source-sdp")]
        {
            let file_path = if url.starts_with("file://") {
                url.strip_prefix("file://").unwrap()
            } else {
                url
            };

            let sdp_content = tokio::fs::read_to_string(file_path).await?;
            let source = SdpSource::new(internal_config, sdp_content)?;
            Ok(Box::new(source))
        }

        #[cfg(not(feature = "source-sdp"))]
        {
            anyhow::bail!("SDP source support not enabled. Enable 'source-sdp' feature.");
        }
    } else {
        anyhow::bail!(
            "Unsupported URL format: {}. Use rtsp:// or file:// or .sdp file path",
            url
        );
    }
}
