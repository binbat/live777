use anyhow::Result;
use async_trait::async_trait;
#[cfg(any(
    feature = "source-rtsp",
    feature = "source-sdp",
    feature = "source-whep"
))]
use bytes::Bytes;
#[cfg(any(feature = "native-source", feature = "source-wayland"))]
use rtc::rtp::packet::Packet;
use std::net::SocketAddr;
#[cfg(any(feature = "native-source", feature = "source-wayland"))]
use std::sync::Arc;
use tokio::sync::broadcast;
use webrtc::peer_connection::RTCIceServer;

#[cfg(feature = "source-rtsp")]
mod rtsp_source;
#[cfg(feature = "source-sdp")]
mod sdp_source;
#[cfg(all(feature = "source-wayland", target_os = "linux"))]
pub mod wayland;
#[cfg(feature = "source-whep")]
mod whep_source;

#[cfg(feature = "source")]
pub(crate) mod adaptive_bitrate;
#[cfg(feature = "native-source")]
pub mod native_encoded_source;
#[cfg(feature = "native-source")]
pub mod source_config;
pub mod source_router;

pub mod manager;
#[cfg(feature = "native-source")]
pub mod native_source;
#[cfg(feature = "source")]
pub mod tier;

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
    #[cfg(any(feature = "native-source", feature = "source-wayland"))]
    RtpPacket(Arc<Packet>),
    // Placeholder when no concrete source implementation is enabled.
    // The `source` feature alone has no active source types, so this
    // variant keeps the enum non-empty without exposing real data.
    #[cfg(not(any(
        feature = "source-rtsp",
        feature = "source-sdp",
        feature = "source-whep",
        feature = "native-source",
        feature = "source-wayland"
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
    // Schemes are case-insensitive (RFC 3986), same as config validation.
    let url = url.to_ascii_lowercase();
    if url.starts_with("rtsp://") || url.starts_with("rtsps://") {
        "rtsp"
    } else if url.starts_with("whep://") || url.starts_with("wheps://") {
        "whep"
    } else if url.starts_with("wayland://") {
        "wayland"
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
    /// Only the SDP file source joins multicast groups.
    #[cfg(feature = "source-sdp")]
    pub multicast_interface: Option<String>,
}

#[cfg(any(
    feature = "source-rtsp",
    feature = "source-sdp",
    feature = "source-whep"
))]
impl InternalSourceConfig {
    pub fn from_config(stream_id: &str, config: &crate::config::SourceConfig) -> Self {
        Self {
            stream_id: stream_id.to_string(),
            #[cfg(any(feature = "source-rtsp", feature = "source-whep"))]
            url: config.url.clone().unwrap_or_default(),
            #[cfg(feature = "source-sdp")]
            multicast_interface: config.multicast_interface.clone(),
        }
    }
}

/// Reconnect policy shared by the reconnect-capable (RTSP/WHEP) sources.
#[cfg(any(feature = "source-rtsp", feature = "source-whep"))]
impl InternalSourceConfig {
    pub fn reconnect_enabled(&self) -> bool {
        // Schemes are case-insensitive (RFC 3986), same as config validation.
        let url = self.url.to_ascii_lowercase();
        url.starts_with("rtsp://")
            || url.starts_with("rtsps://")
            || url.starts_with("whep://")
            || url.starts_with("wheps://")
    }

    /// Delay before reconnect `attempt` (1-based) in milliseconds; see
    /// [`crate::reconnect::reconnect_delay`] for the policy.
    pub fn reconnect_delay_ms(&self, attempt: u32) -> u64 {
        crate::reconnect::reconnect_delay(attempt).as_millis() as u64
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

    /// Adaptive-bitrate bounds when the source's encoder supports runtime
    /// retuning (native sources only; issue #409).  `None` disables the
    /// per-stream controller.
    #[cfg(feature = "source")]
    fn adaptive_bitrate_config(&self) -> Option<adaptive_bitrate::AdaptiveBitrateConfig> {
        None
    }

    /// Apply a new encoder bitrate at runtime (adaptive bitrate control).
    /// Returns false when the backend cannot retune a running encoder.
    #[cfg(feature = "source")]
    async fn set_bitrate(&self, _bps: u32) -> bool {
        false
    }

    /// The bitrate the encoder was configured with (native sources only).
    /// Used to seed the per-stream bitrate state for the admin API.
    #[cfg(feature = "source")]
    fn configured_bitrate(&self) -> Option<u32> {
        None
    }

    /// Named quality tiers configured on the source (native sources only).
    /// Empty when the source defines no tiers.
    #[cfg(feature = "source")]
    fn tiers(&self) -> Vec<tier::QualityTier> {
        Vec::new()
    }

    /// The last tier applied through the admin API, when the source tracks
    /// it (native sources only).  `None` means the configured base
    /// profile.
    #[cfg(feature = "source")]
    fn active_tier(&self) -> Option<String> {
        None
    }

    /// The state the source's pipeline is actually running with (native
    /// sources only): the last applied tier's name plus the current
    /// capture geometry and encoder bitrate.  Read after a failed tier
    /// apply rolled the pipeline back, to re-align hook-driven hardware
    /// with the restored state.
    #[cfg(feature = "source")]
    fn active_tier_state(&self) -> Option<tier::ActiveTierState> {
        None
    }

    /// Apply a quality tier (source-level re-provisioning).  Bitrate-only
    /// tiers retune the running encoder in place; a tier carrying
    /// geometry rebuilds the capture+encoder pipeline underneath the
    /// stream (the RTP broadcast channel and subscriber sessions survive;
    /// expect a sub-second frame gap).  Returns false when the source
    /// cannot apply the tier (non-native source, or a rebuild that failed
    /// and was rolled back).
    #[cfg(feature = "source")]
    async fn apply_tier(&mut self, _tier: &tier::QualityTier) -> bool {
        false
    }
}

/// Server-wide WebRTC network settings handed to sources that dial out to
/// an upstream (currently only the WHEP source; RTSP/SDP sources ignore
/// them).
#[derive(Debug, Clone, Default)]
#[allow(dead_code)] // fields are only read by the source-whep feature
pub struct SourceNetConfig {
    /// ICE servers for the outgoing peer's ICE gathering.
    pub ice_servers: Vec<RTCIceServer>,
    /// UDP sockets ICE binds to (already resolved from config/env).
    pub ice_udp_addrs: Vec<SocketAddr>,
}

pub async fn create_source_from_url(
    stream_id: &str,
    url: &str,
    config: &crate::config::SourceConfig,
    net: SourceNetConfig,
) -> Result<Box<dyn StreamSource>> {
    source_router::create_source_extended(stream_id, url, config, net).await
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
    feature = "source-whep",
    feature = "source-wayland"
))]
pub(crate) async fn create_url_source(
    stream_id: &str,
    url: &str,
    config: &crate::config::SourceConfig,
    net: SourceNetConfig,
) -> Result<Box<dyn StreamSource>> {
    #[cfg(any(
        feature = "source-rtsp",
        feature = "source-sdp",
        feature = "source-whep"
    ))]
    let internal_config = InternalSourceConfig::from_config(stream_id, config);
    #[cfg(not(any(
        feature = "source-rtsp",
        feature = "source-sdp",
        feature = "source-whep"
    )))]
    let _ = config;

    // Only the WHEP source consumes the server-wide network settings; keep
    // other feature combinations warning-free.
    #[cfg(not(feature = "source-whep"))]
    let _ = net;

    // Scheme/suffix checks are case-insensitive (RFC 3986), matching config
    // validation; the sources are still constructed from the original URL.
    let url_lower = url.to_ascii_lowercase();

    if url_lower.starts_with("rtsp://") || url_lower.starts_with("rtsps://") {
        #[cfg(feature = "source-rtsp")]
        {
            let source = RtspSource::new(internal_config, url.to_string())?;
            Ok(Box::new(source))
        }

        #[cfg(not(feature = "source-rtsp"))]
        {
            anyhow::bail!("RTSP source support not enabled. Enable 'source-rtsp' feature.");
        }
    } else if url_lower.starts_with("whep://") || url_lower.starts_with("wheps://") {
        #[cfg(feature = "source-whep")]
        {
            let source = WhepSource::new(internal_config, url, net)?;
            Ok(Box::new(source))
        }

        #[cfg(not(feature = "source-whep"))]
        {
            anyhow::bail!("WHEP source support not enabled. Enable 'source-whep' feature.");
        }
    } else if url_lower.starts_with("wayland://") {
        #[cfg(all(feature = "source-wayland", target_os = "linux"))]
        {
            let source = wayland::WaylandSource::new(stream_id, url)?;
            Ok(Box::new(source))
        }

        #[cfg(not(all(feature = "source-wayland", target_os = "linux")))]
        {
            anyhow::bail!(
                "Wayland source support not enabled. Enable the 'source-wayland' feature (Linux only)."
            );
        }
    } else if url_lower.starts_with("file://") || url_lower.ends_with(".sdp") {
        #[cfg(feature = "source-sdp")]
        {
            let file_path = if url_lower.starts_with("file://") {
                &url["file://".len()..]
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
            "Unsupported URL format: {}. Use rtsp://, whep://, wayland://, file:// or .sdp file path",
            redact_url(url)
        );
    }
}

/// Minimal H.264 Annex-B SPS scanner — only used for H.264 dynamic
/// profile-level-id detection in get_video_codec() (native and wayland
/// sources).
#[cfg(any(
    feature = "native-source",
    all(feature = "source-wayland", target_os = "linux")
))]
pub(crate) fn scan_sps_profile(data: &[u8]) -> Option<String> {
    let mut pos = 0;
    while pos + 3 < data.len() {
        let start_len = if data[pos] == 0 && data[pos + 1] == 0 && data[pos + 2] == 1 {
            3
        } else if pos + 4 <= data.len()
            && data[pos] == 0
            && data[pos + 1] == 0
            && data[pos + 2] == 0
            && data[pos + 3] == 1
        {
            4
        } else {
            pos += 1;
            continue;
        };
        let nal_start = pos + start_len;
        if nal_start < data.len() {
            let nal_type = data[nal_start] & 0x1F;
            if nal_type == 7 {
                // Copy the SPS NAL payload into a small buffer while removing
                // H.264 emulation prevention bytes (0x00 0x00 0x03). The
                // profile-level-id follows the NAL header byte, so we need the
                // next three de-escaped bytes.
                let mut buf = [0u8; 4];
                let mut src = nal_start;
                let mut dst = 0;
                while src < data.len() && dst < 4 {
                    if src + 2 < data.len()
                        && data[src] == 0
                        && data[src + 1] == 0
                        && data[src + 2] == 3
                    {
                        buf[dst] = 0;
                        dst += 1;
                        if dst < 4 {
                            buf[dst] = 0;
                            dst += 1;
                        }
                        src += 3;
                    } else {
                        buf[dst] = data[src];
                        dst += 1;
                        src += 1;
                    }
                }
                if dst == 4 {
                    return Some(format!("{:02x}{:02x}{:02x}", buf[1], buf[2], buf[3]));
                }
            }
        }
        pos += start_len;
    }
    None
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
        assert_eq!(url_source_kind("wayland://display-0"), "wayland");
        // Schemes are case-insensitive, same as the factory dispatch.
        assert_eq!(url_source_kind("WHEP://h/whep/s"), "whep");
        assert_eq!(url_source_kind("RTSP://h/s"), "rtsp");
        assert_eq!(url_source_kind("WAYLAND://display-0"), "wayland");
    }

    #[cfg(any(feature = "source-rtsp", feature = "source-whep"))]
    mod reconnect {
        use super::*;

        fn config(url: &str) -> InternalSourceConfig {
            InternalSourceConfig {
                stream_id: "test".to_string(),
                url: url.to_string(),
                #[cfg(feature = "source-sdp")]
                multicast_interface: None,
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
