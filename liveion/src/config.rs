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
    pub hooks: HooksConfig,

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

#[cfg(test)]
mod hook_tests {
    use super::*;

    #[test]
    fn deserializes_global_and_per_stream_hooks() {
        let cfg: Config = toml::from_str(
            r#"
            [hooks]
            timeout_ms = 3000
            on_error = "continue"
            on_stream_created = ["/global/up.sh"]
            on_stream_deleted = ["/global/down.sh", "/global/down2.sh"]

            [stream.cam1.hooks]
            on_stream_created = ["/per-stream/up.sh"]
            "#,
        )
        .unwrap();

        assert_eq!(cfg.hooks.timeout_ms, 3000);
        assert_eq!(cfg.hooks.on_error, OnError::Continue);
        assert_eq!(cfg.hooks.hooks.on_stream_created, ["/global/up.sh"]);
        assert_eq!(
            cfg.hooks.hooks.on_stream_deleted,
            ["/global/down.sh", "/global/down2.sh"]
        );
        let entry = cfg.stream.streams.get("cam1").unwrap();
        assert_eq!(entry.hooks.on_stream_created, ["/per-stream/up.sh"]);
        assert!(entry.hooks.on_stream_deleted.is_empty());
    }

    #[test]
    fn hooks_default_to_disabled_with_sane_policy() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.hooks.timeout_ms, 5000);
        assert_eq!(cfg.hooks.on_error, OnError::Stop);
        assert!(cfg.hooks.hooks.on_stream_created.is_empty());
        assert!(cfg.hooks.hooks.on_stream_deleted.is_empty());
    }

    #[test]
    fn zero_timeout_disables_the_timeout() {
        let cfg: Config = toml::from_str("[hooks]\ntimeout_ms = 0\n").unwrap();
        assert_eq!(cfg.hooks.timeout_ms, 0);
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

        #[cfg(feature = "target-whip")]
        for (stream_id, entry) in &self.stream.streams {
            let mut seen_urls = std::collections::HashSet::new();
            for target in &entry.targets {
                target.validate().map_err(|e| {
                    anyhow::anyhow!("stream[{}] target config error: {}", stream_id, e)
                })?;
                // Duplicate targets would race on the downstream ("A
                // connection has already been established") and retry
                // forever; reject them at startup instead.
                if !seen_urls.insert(target.url.trim().to_string()) {
                    anyhow::bail!("stream[{}] duplicate target url", stream_id);
                }
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
/// What to do when a hook script fails (non-zero exit, spawn error, or
/// timeout kill).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnError {
    /// Skip the remaining hooks of the same event.
    #[default]
    Stop,
    /// Run every hook of the event even if an earlier one failed.
    Continue,
}

/// Hook scripts for stream-lifecycle events. Used both globally (`[hooks]`)
/// and per stream (`[stream.<name>.hooks]`); per-stream scripts run after
/// the global ones.
///
/// Scripts are executed directly (no shell). Each receives the event
/// metadata as argv (`<event> <stream> [reason]`) and as the environment
/// variables `LIVE777_EVENT` / `LIVE777_STREAM` / `LIVE777_REASON`;
/// publish events additionally export `LIVE777_SESSION`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookConfig {
    /// Scripts run, in order, when a stream is created.
    #[serde(default)]
    pub on_stream_created: Vec<String>,
    /// Scripts run, in order, when a stream is deleted.
    #[serde(default)]
    pub on_stream_deleted: Vec<String>,
    /// Scripts run, in order, when a publisher attaches to a stream — a
    /// WHIP/cascade publisher, or a configured source starting (session id
    /// `virtual-source`). For on-demand streams this is the "someone is
    /// watching" signal that `on_stream_created` (fired at startup) cannot
    /// provide.
    #[serde(default)]
    pub on_publish_started: Vec<String>,
    /// Scripts run, in order, when a publisher detaches or a configured
    /// source stops. The stop reason (`peer-closed` / `api-deleted` /
    /// `idle-timeout`) is passed as argv[3] / `LIVE777_REASON`.
    #[serde(default)]
    pub on_publish_stopped: Vec<String>,
}

/// Global `[hooks]` section: hook scripts plus execution policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(flatten)]
    pub hooks: HookConfig,
    /// Per-script timeout in milliseconds; 0 disables the timeout.
    #[serde(default = "default_hook_timeout_ms")]
    pub timeout_ms: u64,
    /// Whether a failed script skips the remaining hooks of the same event.
    #[serde(default)]
    pub on_error: OnError,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            hooks: HookConfig::default(),
            timeout_ms: default_hook_timeout_ms(),
            on_error: OnError::default(),
        }
    }
}

fn default_hook_timeout_ms() -> u64 {
    5_000
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Optional per-stream hooks, run after the global `[hooks]`.
    #[serde(default)]
    pub hooks: HookConfig,
    /// Start this stream's sources only while it has subscribers instead of at
    /// server startup. The last subscriber leaving stops the sources again
    /// after `on_demand_close_after_ms`.
    #[serde(default)]
    pub on_demand: bool,
    /// Grace period in milliseconds after the last subscriber leaves before
    /// on-demand sources are stopped.
    #[serde(default = "default_on_demand_close_after_ms")]
    pub on_demand_close_after_ms: u64,
    /// How long a subscriber waits for an on-demand source to become ready
    /// (codec known) before the subscribe fails.
    #[serde(default = "default_on_demand_start_timeout_ms")]
    pub on_demand_start_timeout_ms: u64,
    /// Static output targets: push this stream to downstream WHIP endpoints
    /// (declarative cascade-push).
    #[cfg(feature = "target-whip")]
    #[serde(default)]
    pub targets: Vec<TargetConfig>,
}

impl Default for StreamEntry {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            #[cfg(feature = "source")]
            channel: None,
            strategy: None,
            hooks: HookConfig::default(),
            on_demand: false,
            on_demand_close_after_ms: default_on_demand_close_after_ms(),
            on_demand_start_timeout_ms: default_on_demand_start_timeout_ms(),
            #[cfg(feature = "target-whip")]
            targets: Vec::new(),
        }
    }
}

fn default_on_demand_close_after_ms() -> u64 {
    10_000
}

fn default_on_demand_start_timeout_ms() -> u64 {
    10_000
}

/// A static output target of a stream: media is pushed to a downstream WHIP
/// endpoint (declarative cascade-push), on par with how a WHEP source pulls
/// media in. The push is media-driven: it is established when the stream
/// gains a publisher and torn down when the publisher goes away; failures
/// are retried with backoff. A target on an `on_demand` stream acts as
/// standing demand and starts its sources once at startup.
#[cfg(feature = "target-whip")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetConfig {
    /// Downstream WHIP endpoint: `whip://[token@]host:port/whip/<stream>`
    /// (or `whips://`). A Bearer token can be carried as userinfo.
    pub url: String,
}

#[cfg(feature = "target-whip")]
impl TargetConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        crate::target::validate_target_url(self.url.trim())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    /// URL source for RTSP / WHEP / SDP inputs. Mutually exclusive with structured native fields.
    /// Supported: rtsp://, rtsps://, whep://, wheps://, file://, .sdp
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
            && !url_lower.starts_with("whep://")
            && !url_lower.starts_with("wheps://")
            && !url_lower.starts_with("file://")
            && !url_lower.ends_with(".sdp")
        {
            // Scheme-only message: echoing the full URL could leak embedded
            // credentials (e.g. whep://token@…) into startup error logs.
            let scheme = url.split_once("://").map(|(s, _)| s).unwrap_or("<none>");
            anyhow::bail!(
                "Unsupported source URL scheme '{scheme}'. Valid: rtsp://, rtsps://, whep://, wheps://, file://, .sdp"
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
    /// RTSP server listen URL.  Accepts two forms:
    ///
    /// - `rtsp://[user:pass@]host:port` — full URL; when credentials are
    ///   present Digest authentication is enabled automatically.
    /// - `host:port` — bare socket address (no auth).
    ///
    /// Examples: `rtsp://admin:secret@0.0.0.0:8554`, `0.0.0.0:8554`
    #[serde(default = "default_rtsp_listen")]
    pub listen: String,
    /// Maximum number of concurrent RTSP sessions.  New connections are
    /// refused when this limit is reached.
    #[serde(default = "default_rtsp_max_connections")]
    pub max_connections: usize,
    /// RTSP session timeout in seconds.  Sessions without activity are
    /// cleaned up after this duration.
    #[serde(default = "default_rtsp_session_timeout")]
    pub session_timeout: u64,
    /// Realm advertised in the `WWW-Authenticate` challenge.  Only used
    /// when credentials are present in the listen URL.
    #[serde(default = "default_rtsp_realm")]
    pub realm: String,
}

/// Parsed form of [`RtspConfig::listen`].
#[derive(Debug, Clone)]
pub struct RtspListen {
    pub addr: std::net::SocketAddr,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl RtspListen {
    pub fn parse(listen: &str) -> Result<Self, String> {
        if listen.starts_with("rtsp://") {
            #[cfg(feature = "rtsp")]
            {
                Self::parse_url(listen)
            }
            #[cfg(not(feature = "rtsp"))]
            {
                Err("RTSP URL syntax is not supported without the 'rtsp' feature".into())
            }
        } else {
            let addr: std::net::SocketAddr = listen
                .parse()
                .map_err(|e| format!("invalid RTSP listen address '{listen}': {e}"))?;
            Ok(Self {
                addr,
                username: None,
                password: None,
            })
        }
    }

    #[cfg(feature = "rtsp")]
    fn parse_url(listen: &str) -> Result<Self, String> {
        let url = url::Url::parse(listen)
            .map_err(|e| format!("invalid RTSP listen URL '{listen}': {e}"))?;
        if url.scheme() != "rtsp" {
            return Err(format!("RTSP listen URL must use rtsp scheme: '{listen}'"));
        }

        let port = url
            .port()
            .ok_or_else(|| format!("RTSP listen URL must include a port: '{listen}'"))?;
        let host = url
            .host()
            .ok_or_else(|| format!("RTSP listen URL must include a host: '{listen}'"))?;
        let addr = match host {
            url::Host::Ipv4(ip) => std::net::SocketAddr::new(std::net::IpAddr::V4(ip), port),
            url::Host::Ipv6(ip) => std::net::SocketAddr::new(std::net::IpAddr::V6(ip), port),
            url::Host::Domain(domain) => {
                use std::net::ToSocketAddrs;

                (domain, port)
                    .to_socket_addrs()
                    .map_err(|e| format!("failed to resolve RTSP listen host '{domain}': {e}"))?
                    .next()
                    .ok_or_else(|| format!("RTSP listen host '{domain}' resolved no addresses"))?
            }
        };

        let raw_username = url.username();
        let raw_password = url.password();

        if raw_username.is_empty() && raw_password.is_some() {
            return Err(format!(
                "RTSP listen URL password requires a username: '{listen}'"
            ));
        }
        if !raw_username.is_empty() && raw_password.is_none() {
            return Err(format!(
                "RTSP listen URL username requires a password: '{listen}'"
            ));
        }

        let username = (!raw_username.is_empty())
            .then(|| percent_decode_url_component(raw_username))
            .transpose()?;
        let password = raw_password.map(percent_decode_url_component).transpose()?;

        Ok(Self {
            addr,
            username,
            password,
        })
    }

    pub fn enable_auth(&self) -> bool {
        self.username.is_some() && self.password.is_some()
    }
}

#[cfg(feature = "rtsp")]
fn percent_decode_url_component(input: &str) -> Result<String, String> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(format!(
                    "invalid percent escape in RTSP listen URL: '{input}'"
                ));
            }
            let high = hex_value(bytes[i + 1])
                .ok_or_else(|| format!("invalid percent escape in RTSP listen URL: '{input}'"))?;
            let low = hex_value(bytes[i + 2])
                .ok_or_else(|| format!("invalid percent escape in RTSP listen URL: '{input}'"))?;
            decoded.push((high << 4) | low);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8(decoded)
        .map_err(|e| format!("RTSP listen URL credentials are not valid UTF-8: {e}"))
}

#[cfg(feature = "rtsp")]
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(all(test, feature = "rtsp"))]
mod rtsp_listen_tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn parses_bare_socket_address_without_auth() {
        let listen = RtspListen::parse("0.0.0.0:8554").unwrap();

        assert_eq!(
            listen.addr,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8554)
        );
        assert_eq!(listen.username, None);
        assert_eq!(listen.password, None);
        assert!(!listen.enable_auth());
    }

    #[test]
    fn parses_rtsp_url_with_ipv4_and_credentials() {
        let listen = RtspListen::parse("rtsp://admin:secret@0.0.0.0:8554").unwrap();

        assert_eq!(
            listen.addr,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8554)
        );
        assert_eq!(listen.username.as_deref(), Some("admin"));
        assert_eq!(listen.password.as_deref(), Some("secret"));
        assert!(listen.enable_auth());
    }

    #[test]
    fn parses_rtsp_url_with_ipv6_and_path() {
        let listen = RtspListen::parse("rtsp://user:pass@[::]:8554/live?ignored=1").unwrap();

        assert_eq!(
            listen.addr,
            SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 8554)
        );
        assert_eq!(listen.username.as_deref(), Some("user"));
        assert_eq!(listen.password.as_deref(), Some("pass"));
    }

    #[test]
    fn decodes_percent_encoded_credentials() {
        let listen = RtspListen::parse("rtsp://user%40mail:p%3A%2Fss@127.0.0.1:8554").unwrap();

        assert_eq!(listen.username.as_deref(), Some("user@mail"));
        assert_eq!(listen.password.as_deref(), Some("p:/ss"));
    }

    #[test]
    fn rejects_rtsp_url_without_port() {
        let err = RtspListen::parse("rtsp://127.0.0.1").unwrap_err();

        assert!(err.contains("must include a port"));
    }

    #[test]
    fn rejects_rtsp_url_with_password_but_no_username() {
        let err = RtspListen::parse("rtsp://:secret@127.0.0.1:8554").unwrap_err();

        assert!(err.contains("password requires a username"));
    }

    #[test]
    fn rejects_rtsp_url_with_username_but_no_password() {
        let err = RtspListen::parse("rtsp://admin@127.0.0.1:8554").unwrap_err();

        assert!(err.contains("username requires a password"));
    }
}

#[cfg(feature = "rtsp")]
impl Default for RtspConfig {
    fn default() -> Self {
        Self {
            listen: default_rtsp_listen(),
            max_connections: default_rtsp_max_connections(),
            session_timeout: default_rtsp_session_timeout(),
            realm: default_rtsp_realm(),
        }
    }
}

#[cfg(feature = "rtsp")]
fn default_rtsp_listen() -> String {
    "0.0.0.0:8554".to_string()
}

#[cfg(feature = "rtsp")]
fn default_rtsp_max_connections() -> usize {
    rtsp::server_constants::DEFAULT_MAX_CONNECTIONS
}

#[cfg(feature = "rtsp")]
fn default_rtsp_session_timeout() -> u64 {
    rtsp::server_constants::DEFAULT_SESSION_TIMEOUT
}

#[cfg(feature = "rtsp")]
fn default_rtsp_realm() -> String {
    "live777".to_string()
}

#[cfg(feature = "target-whip")]
#[cfg(test)]
mod target_tests {
    use super::*;

    #[test]
    fn target_config_validate_accepts_whip_schemes() {
        for url in [
            "whip://edge-1:7777/whip/cam1",
            "whips://edge-1/whip/cam1",
            "whip://token@edge-1:7777/whip/cam1",
            "WHIP://edge-1/whip/cam1",
        ] {
            let target = TargetConfig { url: url.into() };
            target.validate().unwrap_or_else(|e| panic!("{url}: {e}"));
        }
    }

    #[test]
    fn target_config_validate_rejects_bad_input() {
        for url in [
            "",
            "whep://edge-1/whep/cam1",
            "rtsp://edge-1/cam1",
            "whip://user:pass@edge-1/whip/cam1",
        ] {
            let target = TargetConfig { url: url.into() };
            assert!(target.validate().is_err(), "{url} must be rejected");
        }
    }

    #[test]
    fn stream_entry_targets_roundtrip_toml() {
        let entry: StreamEntry = toml::from_str(
            r#"
            [[targets]]
            url = "whip://token@edge-1:7777/whip/cam1"
            "#,
        )
        .unwrap();
        assert_eq!(entry.targets.len(), 1);
        assert_eq!(entry.targets[0].url, "whip://token@edge-1:7777/whip/cam1");
    }

    #[test]
    fn config_validate_reports_stream_target_error() {
        let mut cfg = Config::default();
        cfg.stream.streams.insert(
            "cam1".to_string(),
            StreamEntry {
                targets: vec![TargetConfig {
                    url: "whep://edge-1/whep/cam1".into(),
                }],
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("cam1"), "error must name the stream: {err}");
    }

    #[test]
    fn config_validate_rejects_duplicate_target_urls() {
        let mut cfg = Config::default();
        cfg.stream.streams.insert(
            "cam1".to_string(),
            StreamEntry {
                targets: vec![
                    TargetConfig {
                        url: "whip://edge-1:7777/whip/cam1".into(),
                    },
                    TargetConfig {
                        url: "whip://edge-1:7777/whip/cam1".into(),
                    },
                ],
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("duplicate"), "error must say duplicate: {err}");
    }
}
