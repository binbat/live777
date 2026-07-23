use anyhow::Result;
use async_trait::async_trait;
#[cfg(any(
    feature = "source-rtsp",
    feature = "source-sdp",
    feature = "source-whep"
))]
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
#[cfg(feature = "source-whep")]
mod whep_source;

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
#[cfg(feature = "source-whep")]
pub use whep_source::WhepSource;

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
    #[cfg(any(
        feature = "source-rtsp",
        feature = "source-sdp",
        feature = "source-whep"
    ))]
    Rtp { channel: u8, data: Bytes },
    #[cfg(feature = "native-source")]
    RtpPacket(Arc<Packet>),
    // Placeholder when no concrete source implementation is enabled.
    // The `source` feature alone has no active source types, so this
    // variant keeps the enum non-empty without exposing real data.
    #[cfg(not(any(
        feature = "source-rtsp",
        feature = "source-sdp",
        feature = "source-whep",
        feature = "native-source"
    )))]
    _Unused,
}

/// RTP/RTCP channel assignment for `MediaPacket::Rtp` producers and the
/// source bridge consuming them. URL-based sources (RTSP interleaved
/// channels, WHEP synthesized channels) and the bridge must agree on one
/// mapping, so it lives here next to `MediaPacket` instead of being
/// mirrored on both sides.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(crate) struct ChannelMapping {
    pub(crate) video_rtp: Option<u8>,
    pub(crate) video_rtcp: Option<u8>,
    pub(crate) audio_rtp: Option<u8>,
    pub(crate) audio_rtcp: Option<u8>,
}

#[allow(dead_code)]
impl ChannelMapping {
    pub(crate) fn new(has_video: bool, has_audio: bool) -> Self {
        match (has_video, has_audio) {
            (true, false) => Self {
                video_rtp: Some(0),
                video_rtcp: Some(1),
                audio_rtp: None,
                audio_rtcp: None,
            },
            (false, true) => Self {
                video_rtp: None,
                video_rtcp: None,
                audio_rtp: Some(0),
                audio_rtcp: Some(1),
            },
            (true, true) => Self {
                video_rtp: Some(0),
                video_rtcp: Some(1),
                audio_rtp: Some(2),
                audio_rtcp: Some(3),
            },
            (false, false) => Self {
                video_rtp: None,
                video_rtcp: None,
                audio_rtp: None,
                audio_rtcp: None,
            },
        }
    }

    pub(crate) fn is_video_rtp(&self, channel: u8) -> bool {
        self.video_rtp == Some(channel)
    }

    pub(crate) fn is_video_rtcp(&self, channel: u8) -> bool {
        self.video_rtcp == Some(channel)
    }

    pub(crate) fn is_audio_rtp(&self, channel: u8) -> bool {
        self.audio_rtp == Some(channel)
    }

    pub(crate) fn is_audio_rtcp(&self, channel: u8) -> bool {
        self.audio_rtcp == Some(channel)
    }
}

/// `url` with any userinfo credentials stripped, safe for log lines.
/// Falls back to a scheme-only placeholder when the URL cannot be parsed
/// (an unparseable URL may still embed credentials).
#[cfg(feature = "source")]
pub(crate) fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        }
        Err(_) => match raw.split_once("://") {
            Some((scheme, _)) => format!("{scheme}://<redacted>"),
            None => "<redacted>".to_string(),
        },
    }
}

/// Source-kind label for a URL, mirroring `create_url_source`'s dispatch.
#[cfg(feature = "source")]
pub(crate) fn url_source_kind(url: &str) -> &'static str {
    if url.starts_with("rtsp://") || url.starts_with("rtsps://") {
        "rtsp"
    } else if url.starts_with("whep://") || url.starts_with("wheps://") {
        "whep"
    } else {
        "sdp"
    }
}

#[derive(Debug, Clone)]
#[cfg(any(
    feature = "source-rtsp",
    feature = "source-sdp",
    feature = "source-whep"
))]
pub struct InternalSourceConfig {
    pub stream_id: String,
    /// Only reconnect-capable sources (RTSP/WHEP) consult the URL; the SDP
    /// file source never reconnects.
    #[cfg(any(feature = "source-rtsp", feature = "source-whep"))]
    pub url: String,
}

#[cfg(any(
    feature = "source-rtsp",
    feature = "source-sdp",
    feature = "source-whep"
))]
impl InternalSourceConfig {
    pub fn from_config(stream_id: &str, config: &crate::config::SourceConfig) -> Self {
        #[cfg(not(any(feature = "source-rtsp", feature = "source-whep")))]
        let _ = config;

        Self {
            stream_id: stream_id.to_string(),
            #[cfg(any(feature = "source-rtsp", feature = "source-whep"))]
            url: config.url.clone().unwrap_or_default(),
        }
    }
}

/// Reconnect policy shared by the reconnect-capable (RTSP/WHEP) sources.
#[cfg(any(feature = "source-rtsp", feature = "source-whep"))]
impl InternalSourceConfig {
    pub fn reconnect_enabled(&self) -> bool {
        self.url.starts_with("rtsp://")
            || self.url.starts_with("rtsps://")
            || self.url.starts_with("whep://")
            || self.url.starts_with("wheps://")
    }

    /// Delay before reconnect `attempt` (1-based): exponential backoff from a
    /// 5 s base, capped at 60 s (5 s, 10 s, 20 s, 40 s, 60 s, …).
    pub fn reconnect_delay_ms(&self, attempt: u32) -> u64 {
        const RECONNECT_BASE_MS: u64 = 5_000;
        const RECONNECT_MAX_MS: u64 = 60_000;
        let shift = attempt.saturating_sub(1).min(4);
        RECONNECT_BASE_MS
            .saturating_mul(1u64 << shift)
            .min(RECONNECT_MAX_MS)
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

#[cfg(any(
    feature = "source-rtsp",
    feature = "source-sdp",
    feature = "source-whep"
))]
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
    } else if url.starts_with("whep://") || url.starts_with("wheps://") {
        #[cfg(feature = "source-whep")]
        {
            let source = WhepSource::new(internal_config, url)?;
            Ok(Box::new(source))
        }

        #[cfg(not(feature = "source-whep"))]
        {
            anyhow::bail!("WHEP source support not enabled. Enable 'source-whep' feature.");
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
            "Unsupported URL format: {}. Use rtsp://, whep://, file:// or .sdp file path",
            redact_url(url)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_url_strips_userinfo() {
        assert_eq!(
            redact_url("rtsp://user:pass@example.com:8554/cam?x=1"),
            "rtsp://example.com:8554/cam?x=1"
        );
        assert_eq!(
            redact_url("whep://token@edge-0:7777/whep/cam1"),
            "whep://edge-0:7777/whep/cam1"
        );
        // Nothing to strip: returned unchanged.
        assert_eq!(
            redact_url("rtsp://example.com:8554/cam"),
            "rtsp://example.com:8554/cam"
        );
    }

    #[test]
    fn redact_url_falls_back_without_leaking_credentials() {
        // An unparseable URL may still embed credentials: only the scheme
        // (or nothing) survives.
        assert_eq!(redact_url("whep://tok en@not a host"), "whep://<redacted>");
        assert_eq!(redact_url("not-a-url"), "<redacted>");
    }

    #[test]
    fn url_source_kind_matches_factory_dispatch() {
        assert_eq!(url_source_kind("rtsp://h/s"), "rtsp");
        assert_eq!(url_source_kind("rtsps://h/s"), "rtsp");
        assert_eq!(url_source_kind("whep://h/whep/s"), "whep");
        assert_eq!(url_source_kind("wheps://h/whep/s"), "whep");
        assert_eq!(url_source_kind("file://cam.sdp"), "sdp");
        assert_eq!(url_source_kind("cam.sdp"), "sdp");
    }

    #[cfg(any(feature = "source-rtsp", feature = "source-whep"))]
    mod reconnect {
        use super::*;

        fn config(url: &str) -> InternalSourceConfig {
            InternalSourceConfig {
                stream_id: "test".to_string(),
                url: url.to_string(),
            }
        }

        #[test]
        fn reconnect_enabled_for_rtsp_and_whep_schemes() {
            for url in [
                "rtsp://h/s",
                "rtsps://h/s",
                "whep://h/whep/s",
                "wheps://h/whep/s",
            ] {
                assert!(config(url).reconnect_enabled(), "{url}");
            }
            assert!(!config("file://cam.sdp").reconnect_enabled());
        }

        #[test]
        fn reconnect_delay_doubles_with_cap() {
            let config = config("whep://h/whep/s");
            assert_eq!(config.reconnect_delay_ms(1), 5_000);
            assert_eq!(config.reconnect_delay_ms(2), 10_000);
            assert_eq!(config.reconnect_delay_ms(3), 20_000);
            assert_eq!(config.reconnect_delay_ms(4), 40_000);
            assert_eq!(config.reconnect_delay_ms(5), 60_000);
            // Capped afterwards, and saturating on huge attempt counts.
            assert_eq!(config.reconnect_delay_ms(6), 60_000);
            assert_eq!(config.reconnect_delay_ms(u32::MAX), 60_000);
        }
    }
}
