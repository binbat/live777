use std::{collections::HashMap, env, net::SocketAddr, str::FromStr};

use iceserver::{IceServer, default_ice_servers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub http: Http,
    #[serde(default = "default_ice_servers")]
    pub ice_servers: Vec<IceServer>,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub log: Log,
    #[serde(default)]
    pub strategy: api::strategy::Strategy,

    #[serde(default)]
    pub sdp: Sdp,

    #[serde(default)]
    pub webrtc: WebRtc,

    #[cfg(feature = "net4mqtt")]
    #[serde(default)]
    pub net4mqtt: Option<Net4mqtt>,

    #[cfg(feature = "recorder")]
    #[serde(default)]
    pub recorder: RecorderConfig,

    #[cfg(feature = "rtsp")]
    #[serde(default)]
    pub rtsp: RtspConfig,

    #[serde(default)]
    pub stream: StreamConfig,
}

#[cfg(feature = "net4mqtt")]
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Net4mqtt {
    #[serde(default)]
    pub mqtt_url: String,
    #[serde(default)]
    pub alias: String,
}

#[cfg(feature = "net4mqtt")]
impl Net4mqtt {
    pub fn validate(&mut self) {
        self.mqtt_url = self.mqtt_url.replace("{alias}", &self.alias)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Http {
    #[serde(default = "default_http_listen")]
    pub listen: SocketAddr,
    #[serde(default)]
    pub cors: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Auth {
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub tokens: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Log {
    #[serde(default = "default_log_level")]
    pub level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Sdp {
    /// Disable specific codecs in SDP negotiation, e.g. ["VP8", "H264"]
    #[serde(default)]
    pub disable_codecs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRtc {
    /// UDP bind addresses used by WebRTC ICE host candidates.
    ///
    /// Environment variables are still supported and take priority:
    /// LIVE777_WEBRTC_ICE_UDP_ADDRS, LIVE777_WEBRTC_ICE_UDP_ADDR,
    /// LIVETWO_WEBRTC_ICE_UDP_ADDR.
    #[serde(default = "default_webrtc_ice_udp_addrs")]
    pub ice_udp_addrs: Vec<String>,
}

fn default_webrtc_ice_udp_addrs() -> Vec<String> {
    vec![api::webrtc::DEFAULT_WEBRTC_ICE_UDP_ADDR.to_string()]
}

impl Default for WebRtc {
    fn default() -> Self {
        Self {
            ice_udp_addrs: default_webrtc_ice_udp_addrs(),
        }
    }
}

fn default_http_listen() -> SocketAddr {
    SocketAddr::from_str(&format!(
        "0.0.0.0:{}",
        env::var("PORT").unwrap_or(String::from("7777"))
    ))
    .expect("invalid listen address")
}

impl Default for Http {
    fn default() -> Self {
        Self {
            listen: default_http_listen(),
            cors: Default::default(),
        }
    }
}

impl Default for Log {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
}

#[cfg(feature = "source")]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChannelConfig {
    /// Local UDP socket address to bind for the DataChannel bridge.
    /// Example: `"0.0.0.0:7774"` or `"[::]:7774"`.
    pub listen: std::net::SocketAddr,
    /// Target UDP address where DataChannel messages are forwarded.
    /// Example: `"127.0.0.1:8890"`.
    pub target: std::net::SocketAddr,
}

#[cfg(feature = "source")]
impl ChannelConfig {
    /// Return the listen and target socket addresses.
    pub fn endpoints(&self) -> (std::net::SocketAddr, std::net::SocketAddr) {
        (self.listen, self.target)
    }
}

#[cfg(feature = "source")]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_config_ipv4() {
        let s = ChannelConfig {
            listen: "0.0.0.0:7774".parse().unwrap(),
            target: "127.0.0.1:1234".parse().unwrap(),
        };
        let (listen, target) = s.endpoints();
        assert_eq!(listen.to_string(), "0.0.0.0:7774");
        assert_eq!(target.to_string(), "127.0.0.1:1234");
    }

    #[test]
    fn test_channel_config_ipv6() {
        let s = ChannelConfig {
            listen: "[::]:7774".parse().unwrap(),
            target: "[::1]:1234".parse().unwrap(),
        };
        let (listen, target) = s.endpoints();
        assert_eq!(listen.to_string(), "[::]:7774");
        assert_eq!(target.to_string(), "[::1]:1234");
    }

    #[test]
    #[cfg(feature = "native-source")]
    fn test_stream_entry_roundtrip() {
        let entry: StreamEntry = toml::from_str(
            r#"
            [[sources]]
            [sources.capture]
            backend = "libcamera"
            device = "0"
            width = 640
            height = 480
            fps = 30
            pixel_format = "yuv420"
            [sources.encoder]
            backend = "v4l2-m2m"
            codec = "h264"
            bitrate = 1000000
            profile = "baseline"
            level = "3.1"
            gop = 60

            [channel]
            listen = "0.0.0.0:8891"
            target = "127.0.0.1:8890"

            [strategy]
            auto_create_whip = false
            "#,
        )
        .unwrap();

        assert_eq!(entry.sources.len(), 1);
        let source = entry.sources.first().unwrap();
        assert!(source.capture.is_some());
        let capture = source.capture.as_ref().unwrap();
        assert_eq!(capture.backend, "libcamera");
        assert_eq!(capture.device.as_deref(), Some("0"));
        assert_eq!(source.encoder.as_ref().unwrap().profile, "baseline");
        assert_eq!(
            source.encoder.as_ref().unwrap().level.as_deref(),
            Some("3.1")
        );

        let channel = entry.channel.as_ref().unwrap();
        assert_eq!(channel.listen.to_string(), "0.0.0.0:8891");
        assert_eq!(channel.target.to_string(), "127.0.0.1:8890");

        let strategy = entry.strategy.as_ref().unwrap();
        assert!(!strategy.auto_create_whip);
    }
}

#[cfg(test)]
mod webrtc_tests {
    use super::*;

    #[test]
    fn deserializes_webrtc_ice_udp_addrs_config() {
        let cfg: Config = toml::from_str(
            r#"
            [webrtc]
            ice_udp_addrs = ["127.0.0.1:0"]
            "#,
        )
        .unwrap();

        assert_eq!(cfg.webrtc.ice_udp_addrs, vec!["127.0.0.1:0"]);
    }
}

fn default_log_level() -> String {
    env::var("LOG_LEVEL").unwrap_or_else(|_| {
        if cfg!(debug_assertions) {
            "debug".to_string()
        } else {
            "info".to_string()
        }
    })
}

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        for ice_server in self.ice_servers.iter() {
            ice_server
                .validate()
                .map_err(|e| anyhow::anyhow!(format!("ice_server error : {}", e)))?;
        }

        #[cfg(feature = "source")]
        for (stream_id, entry) in &self.stream.streams {
            for source in &entry.sources {
                source.validate().map_err(|e| {
                    anyhow::anyhow!("stream[{}] source config error: {}", stream_id, e)
                })?;
            }
            if let Some(channel) = &entry.channel
                && (channel.listen.port() == 0 || channel.target.port() == 0)
            {
                anyhow::bail!(
                    "stream[{}] channel listen/target ports must be non-zero",
                    stream_id
                );
            }
        }
        Ok(())
    }
}

#[cfg(feature = "recorder")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecorderConfig {
    /// List of stream names to automatically record
    #[serde(default)]
    pub auto_streams: Vec<String>,

    /// Storage backend configuration
    #[serde(default)]
    pub storage: storage::StorageConfig,

    /// Node alias for identification (optional)
    #[serde(default)]
    pub node_alias: Option<String>,

    /// Optional path for recorder index file (index.json)
    #[serde(default)]
    pub index_path: Option<String>,

    /// Maximum duration in seconds for a single recording before rotation (0 disables auto-rotation)
    #[serde(default = "default_max_recording_seconds")]
    pub max_recording_seconds: u64,

    /// Async upload configuration
    #[serde(default)]
    pub upload: UploadConfig,
}

#[cfg(feature = "recorder")]
fn default_max_recording_seconds() -> u64 {
    86_400
}

#[cfg(feature = "recorder")]
impl Default for RecorderConfig {
    fn default() -> Self {
        Self {
            auto_streams: vec![],
            storage: Default::default(),
            node_alias: None,
            index_path: None,
            max_recording_seconds: default_max_recording_seconds(),
            upload: Default::default(),
        }
    }
}

#[cfg(feature = "recorder")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadConfig {
    /// Enable async uploads via Liveman presigned URLs
    #[serde(default)]
    pub enabled: bool,
    /// Liveman base URL, e.g. http://127.0.0.1:8888
    #[serde(default)]
    pub liveman_url: String,
    /// Liveman bearer token for presign API
    #[serde(default)]
    pub liveman_token: String,
    /// Queue file path for pending uploads
    #[serde(default = "default_upload_queue_path")]
    pub queue_path: String,
    /// Local spool directory for recordings before upload
    #[serde(default = "default_upload_local_dir")]
    pub local_dir: String,
    /// Presigned URL TTL seconds
    #[serde(default = "default_presign_ttl_seconds")]
    pub presign_ttl_seconds: u64,
    /// Upload loop interval in milliseconds
    #[serde(default = "default_upload_interval_ms")]
    pub interval_ms: u64,
    /// Maximum concurrent uploads
    #[serde(default = "default_upload_concurrency")]
    pub concurrency: usize,
}

#[cfg(feature = "recorder")]
impl Default for UploadConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            liveman_url: String::new(),
            liveman_token: String::new(),
            queue_path: default_upload_queue_path(),
            local_dir: default_upload_local_dir(),
            presign_ttl_seconds: default_presign_ttl_seconds(),
            interval_ms: default_upload_interval_ms(),
            concurrency: default_upload_concurrency(),
        }
    }
}

#[cfg(feature = "recorder")]
fn default_upload_queue_path() -> String {
    "./recordings/upload_queue.jsonl".to_string()
}

#[cfg(feature = "recorder")]
fn default_upload_local_dir() -> String {
    "./recordings".to_string()
}

#[cfg(feature = "recorder")]
fn default_presign_ttl_seconds() -> u64 {
    300
}

#[cfg(feature = "recorder")]
fn default_upload_interval_ms() -> u64 {
    2_000
}

#[cfg(feature = "recorder")]
fn default_upload_concurrency() -> usize {
    2
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamConfig {
    /// Per-stream configuration, keyed by stream name.
    ///
    /// Example:
    ///   [stream.dc-udp]
    ///   [stream.dc-udp.channel]
    ///   listen = "0.0.0.0:8891"
    ///   target = "127.0.0.1:8890"
    ///
    ///   [stream.rtsp-cam]
    ///   [[stream.rtsp-cam.sources]]
    ///   url = "rtsp://..."
    #[serde(flatten)]
    pub streams: HashMap<String, StreamEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamEntry {
    /// Media input sources for this stream.
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
    /// Optional DataChannel <-> UDP bridge for this stream.
    #[cfg(feature = "source")]
    #[serde(default)]
    pub channel: Option<ChannelConfig>,
    /// Optional per-stream strategy override.
    #[serde(default)]
    pub strategy: Option<api::strategy::Strategy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    /// URL source for RTSP / SDP inputs. Mutually exclusive with structured native fields.
    /// Supported: rtsp://, rtsps://, file://, .sdp
    #[serde(default)]
    pub url: Option<String>,

    /// Capture config (required for structured native sources).
    #[cfg(feature = "native-source")]
    #[serde(default)]
    pub capture: Option<crate::stream::source::source_config::CaptureSpec>,

    /// Encoder config (required for structured native sources).
    #[cfg(feature = "native-source")]
    #[serde(default)]
    pub encoder: Option<crate::stream::source::source_config::EncoderSpec>,

    /// RTP output params (optional, defaults apply).
    #[cfg(feature = "native-source")]
    #[serde(default)]
    pub output: crate::stream::source::source_config::OutputSpec,
}

impl SourceConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        #[cfg(feature = "native-source")]
        if self.capture.is_some() {
            let capture = self
                .capture
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("capture is required for native sources"))?;
            if capture.device.as_deref().unwrap_or("").trim().is_empty() {
                anyhow::bail!("capture.device cannot be empty");
            }
            let backend = capture.backend.to_lowercase();
            if backend != "libcamera" && backend != "v4l2" {
                anyhow::bail!(
                    "capture.backend must be 'libcamera' or 'v4l2', got '{}'",
                    backend
                );
            }
            if capture.width == 0 || capture.height == 0 {
                anyhow::bail!("capture width/height must be non-zero");
            }
            let encoder = self
                .encoder
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("encoder is required for native sources"))?;
            if encoder.bitrate == 0 {
                anyhow::bail!("encoder.bitrate must be non-zero");
            }
            return Ok(());
        }

        let url = self.url.as_deref().unwrap_or("");
        if url.is_empty() {
            anyhow::bail!("either url or capture must be set");
        }

        let url_lower = url.to_lowercase();
        if !url_lower.starts_with("rtsp://")
            && !url_lower.starts_with("rtsps://")
            && !url_lower.starts_with("file://")
            && !url_lower.ends_with(".sdp")
        {
            anyhow::bail!(
                "Unsupported URL: {}. Valid: rtsp://, rtsps://, file://, .sdp",
                url
            );
        }
        Ok(())
    }

    /// Build a `SourceSpec` from structured fields (for native sources).
    #[cfg(feature = "native-source")]
    pub fn to_spec(
        &self,
        stream_id: &str,
    ) -> Option<crate::stream::source::source_config::SourceSpec> {
        let capture = self.capture.clone()?;
        let encoder = self.encoder.clone()?;
        Some(crate::stream::source::source_config::SourceSpec {
            stream_id: stream_id.to_string(),
            capture,
            encoder,
            output: self.output.clone(),
        })
    }
}

#[cfg(feature = "rtsp")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RtspConfig {
    /// Listen address for RTSP server input (PUSH / ANNOUNCE-RECORD).
    #[serde(default = "default_rtsp_push_listen")]
    pub push_listen: std::net::SocketAddr,
    /// Listen address for RTSP server output (PULL / DESCRIBE-PLAY).
    #[serde(default = "default_rtsp_pull_listen")]
    pub pull_listen: std::net::SocketAddr,
}

#[cfg(feature = "rtsp")]
impl Default for RtspConfig {
    fn default() -> Self {
        Self {
            push_listen: default_rtsp_push_listen(),
            pull_listen: default_rtsp_pull_listen(),
        }
    }
}

#[cfg(feature = "rtsp")]
fn default_rtsp_push_listen() -> std::net::SocketAddr {
    std::net::SocketAddr::from_str("0.0.0.0:8554").unwrap()
}

#[cfg(feature = "rtsp")]
fn default_rtsp_pull_listen() -> std::net::SocketAddr {
    std::net::SocketAddr::from_str("0.0.0.0:8555").unwrap()
}
