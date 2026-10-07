use std::{collections::HashMap, env, net::SocketAddr, str::FromStr};

use iceserver::IceServer;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub http: Http,
    /// STUN/TURN servers advertised to WHIP/WHEP clients via
    /// `Link: ...; rel="ice-server"` response headers and used by outgoing
    /// peers (e.g. WHEP pull sources). Empty by default — nothing is
    /// advertised and only host candidates are gathered, so an unconfigured
    /// server stays fully on the LAN. Add entries only for internet-facing
    /// deployments. Server-side sessions ignore this list while
    /// `webrtc.ice_lite` is on (the default).
    #[serde(default)]
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
    /// Only allow these video codecs in SDP negotiation, e.g. ["H264"].
    /// Empty means no restriction.
    /// Available: VP8, VP9, H264, H265, AV1
    #[serde(default)]
    pub video_codecs: Vec<String>,
    /// Only allow these audio codecs in SDP negotiation, e.g. ["OPUS"].
    /// Empty means no restriction.
    /// Available: OPUS, G722, PCMU, PCMA
    #[serde(default)]
    pub audio_codecs: Vec<String>,
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
    /// Run server-side WebRTC sessions as an ICE Lite agent (RFC 8445
    /// section 2.7), as required of WHIP/WHEP endpoints by RFC 9725 and
    /// draft-ietf-wish-whep: answer with `a=ice-lite`, only respond to
    /// connectivity checks, never initiate them. This lets trickle-ICE
    /// clients connect even when their candidates are unusable by the
    /// server (e.g. browser mDNS `*.local` candidates, which liveion does
    /// not resolve). Lite agents gather host candidates only, so
    /// `[[ice_servers]]` are not used by server-side sessions in this mode;
    /// set this to false if the server itself is behind NAT and needs
    /// srflx/relay candidates, or if a peer requires a full-ICE server.
    #[serde(default = "default_webrtc_ice_lite")]
    pub ice_lite: bool,
}

fn default_webrtc_ice_udp_addrs() -> Vec<String> {
    vec![api::webrtc::DEFAULT_WEBRTC_ICE_UDP_ADDR.to_string()]
}

fn default_webrtc_ice_lite() -> bool {
    true
}

impl Default for WebRtc {
    fn default() -> Self {
        Self {
            ice_udp_addrs: default_webrtc_ice_udp_addrs(),
            ice_lite: default_webrtc_ice_lite(),
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
    fn ice_servers_default_to_empty() {
        // LAN-only out of the box: an omitted key must not pull in a public
        // STUN server, and an explicit empty list stays empty.
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.ice_servers.is_empty());
        let cfg: Config = toml::from_str("ice_servers = []").unwrap();
        assert!(cfg.ice_servers.is_empty());
    }

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

    #[test]
    #[cfg(feature = "native-source")]
    fn test_stream_entry_source_tiers() {
        let entry: StreamEntry = toml::from_str(
            r#"
            [[sources]]
            [sources.capture]
            backend = "v4l2"
            device = "/dev/video11"
            width = 1920
            height = 1080
            fps = 30
            pixel_format = "nv12"
            [sources.encoder]
            backend = "rkmpp"
            codec = "h264"
            bitrate = 4000000
            profile = "640028"
            gop = 60
            adaptive = { min_bitrate = 300000 }

            [[sources.tiers]]
            name = "low"
            encoder = { bitrate = 600000 }
            [[sources.tiers]]
            name = "mid"
            capture = { width = 1280, height = 720, fps = 15 }
            "#,
        )
        .unwrap();

        let source = entry.sources.first().unwrap();
        assert_eq!(source.tiers.len(), 2);
        assert_eq!(source.tiers[0].name, "low");
        assert_eq!(source.tiers[0].encoder.bitrate, Some(600_000));
        assert_eq!(source.tiers[1].name, "mid");
        assert_eq!(source.tiers[1].capture.width, Some(1280));

        // The spec built from this config must pass validation and carry
        // the tiers through.
        let spec = source.to_spec("cam").unwrap();
        assert!(spec.validate().is_ok());
        assert_eq!(spec.tiers.len(), 2);
        assert!(spec.encoder.adaptive.is_some());
    }

    fn url_source(url: &str, multicast_interface: Option<&str>) -> SourceConfig {
        SourceConfig {
            url: Some(url.to_string()),
            multicast_interface: multicast_interface.map(str::to_string),
            #[cfg(feature = "native-source")]
            capture: None,
            #[cfg(feature = "native-source")]
            encoder: None,
            #[cfg(feature = "native-source")]
            output: Default::default(),
            #[cfg(feature = "native-source")]
            tiers: Vec::new(),
        }
    }

    #[test]
    fn multicast_interface_accepts_address_index_and_name() {
        for iface in [
            "192.168.123.11",
            "0.0.0.0",
            "2",
            "eth0",
            "vlan.100",
            "br-lan",
        ] {
            let cfg = url_source("/etc/live777/cam.sdp", Some(iface));
            assert!(cfg.validate().is_ok(), "'{iface}' must validate");
        }
    }

    #[test]
    fn multicast_interface_rejects_garbage() {
        for iface in [
            "not an ip",
            "99999999999999999999999",
            "way-too-long-interface-name",
        ] {
            let cfg = url_source("/etc/live777/cam.sdp", Some(iface));
            assert!(cfg.validate().is_err(), "'{iface}' must not validate");
        }
    }

    #[test]
    fn multicast_interface_warns_but_validates_for_non_sdp_sources() {
        // Set on an RTSP source it is ignored (only SDP file sources join
        // multicast groups), but it is not a config error.
        let cfg = url_source("rtsp://192.168.1.100:554/stream", Some("192.168.123.11"));
        assert!(cfg.validate().is_ok());
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

    #[test]
    fn webrtc_ice_lite_defaults_to_true() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.webrtc.ice_lite);

        let cfg: Config = toml::from_str("[webrtc]\nice_lite = false\n").unwrap();
        assert!(!cfg.webrtc.ice_lite);
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

        #[cfg(feature = "target")]
        for (stream_id, entry) in &self.stream.streams {
            let mut seen_urls = std::collections::HashSet::new();
            for target in &entry.targets {
                target.validate().map_err(|e| {
                    anyhow::anyhow!("stream[{}] target config error: {}", stream_id, e)
                })?;
                // Duplicate targets would keep displacing each other on
                // the downstream and flap forever; reject them at startup
                // instead.
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
    /// Scripts run, in order, when a source's parameter set is changed
    /// through the admin tier API (`POST /api/sources/{stream}/tier`) — a
    /// tier is a named preset of source parameters, applying one
    /// re-provisions the source.  The scripts run **synchronously inside**
    /// the apply, before the tier's capture+encoder re-provisioning:
    /// hardware that must be reconfigured for the new parameter set
    /// (e.g. a camera sensor's mode gear on Rockchip-style V4L2 pipelines,
    /// where framerate is a sensor-mode property the capture node cannot
    /// change) is switched here so the subsequent pipeline rebuild starts
    /// on the right hardware state.  A failing script aborts the apply
    /// when `on_error = "stop"`, before the pipeline is touched.  When
    /// the apply does not reach the target state — aborted, or the
    /// pipeline rebuild failed and rolled back — the scripts run again
    /// with the source's *current* state, so hardware they switched is
    /// switched back (best effort; scripts must be idempotent).
    ///
    /// Unlike the lifecycle hooks these do not go through the queued hook
    /// executor.  argv is `<stream> <tier>`; the tier name is also
    /// exported as `LIVE777_SOURCE_TIER` (empty when the source runs its
    /// configured base profile), and the tier parameters as
    /// `LIVE777_SOURCE_WIDTH` / `LIVE777_SOURCE_HEIGHT` /
    /// `LIVE777_SOURCE_FPS` / `LIVE777_SOURCE_BITRATE` — the *declared*
    /// tier values on the pre-apply run (empty for unset fields), the
    /// pipeline's *actual* current values on the compensation run.
    #[serde(default)]
    pub on_source_changed: Vec<String>,
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
    /// (declarative cascade-push), send it out as plain RTP/UDP to a
    /// multicast group or unicast address, and/or push it to an RTSP server
    /// as a client (ANNOUNCE/RECORD).
    #[cfg(feature = "target")]
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
            #[cfg(feature = "target")]
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

/// A static output target of a stream. Three flavors:
///
/// - `whip://`/`whips://`: media is pushed to a downstream WHIP endpoint
///   (declarative cascade-push), on par with how a WHEP source pulls media
///   in.
/// - `rtp://`: media is sent out as plain RTP over UDP to a multicast group
///   (e.g. a Unitree video receiver) or a unicast address.
/// - `rtsp://`: media is pushed to an RTSP server as a client
///   (ANNOUNCE/SETUP/RECORD), mirroring how an RTSP source pulls media in.
///
/// All are media-driven: sending starts when the stream gains a publisher
/// and stops when the publisher goes away; failures are retried with
/// backoff. A target on an `on_demand` stream acts as standing demand: its
/// sources are (re)started whenever the stream has neither a publisher nor
/// an active target session.
///
/// Multicast senders keep the kernel's default `IP_MULTICAST_LOOP`, so a
/// same-host SDP file source joined to the same group:port ingests the
/// target's own output — a feedback loop. Sending and ingesting the same
/// group:port on one host is a misconfiguration (use distinct groups or
/// ports for same-host demos).
#[cfg(feature = "target")]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TargetConfig {
    /// Downstream WHIP endpoint: `whip://[token@]host:port/whip/<stream>`
    /// (or `whips://`), or an RTP destination: `rtp://host:port`. A Bearer
    /// token can be carried as userinfo (WHIP only). The RTP host must be
    /// an IP literal (v6 in brackets); a multicast address sends to the
    /// group, anything else is unicast.
    pub url: String,
    /// Outbound multicast interface for `rtp://` targets: an IPv4 address
    /// for IPv4 groups; an interface index or name for IPv6 groups. Unset
    /// lets the kernel choose. Only valid with a multicast `rtp://` url.
    #[serde(default)]
    pub multicast_interface: Option<String>,
    /// IPv4 multicast TTL / IPv6 multicast hops for `rtp://` targets
    /// (default 1). Only valid with a multicast `rtp://` url.
    #[serde(default)]
    pub ttl: Option<u32>,
    /// Payload type stamped on the video track's outgoing packets and
    /// advertised in the generated SDP (96-127, dynamic range). Unset keeps
    /// the automatic choice: the publisher's negotiated PT, or 96 for
    /// dynamic codecs. Only valid with an `rtp://` url.
    #[serde(default)]
    pub payload_type: Option<u32>,
    /// Write a receiver-side SDP file describing this target's output when
    /// sending starts (like ffmpeg's rtp output writing test.sdp). The
    /// file is directly consumable by live777's SDP file source
    /// (`source-sdp`), ffmpeg, and gstreamer. Unset disables. Only valid
    /// with an `rtp://` url.
    #[serde(default)]
    pub sdp_file: Option<String>,
}

#[cfg(feature = "target")]
impl TargetConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        let url = self.url.trim();
        let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
        match scheme.as_str() {
            #[cfg(feature = "target-whip")]
            "whip" | "whips" => {
                if self.multicast_interface.is_some()
                    || self.ttl.is_some()
                    || self.payload_type.is_some()
                    || self.sdp_file.is_some()
                {
                    anyhow::bail!(
                        "multicast_interface, ttl, payload_type and sdp_file \
                         are only valid with an rtp:// target"
                    );
                }
                crate::target::validate_target_url(url)
            }
            #[cfg(feature = "target-rtp")]
            "rtp" => crate::target_rtp::validate_rtp_target(self),
            #[cfg(feature = "target-rtsp")]
            "rtsp" => crate::target_rtsp::validate_rtsp_target(self),
            _ => anyhow::bail!("unsupported target url scheme: {url}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    /// URL source for RTSP / WHEP / SDP inputs. Mutually exclusive with structured native fields.
    /// Supported: rtsp://, rtsps://, whep://, wheps://, file://, .sdp
    #[serde(default)]
    pub url: Option<String>,

    /// Interface for joining multicast groups (SDP file sources whose SDP
    /// connection address is a multicast group, e.g. `c=IN IP4 230.1.1.1`).
    /// An IPv4 address selects the interface for IPv4 groups; IPv6 groups
    /// take an interface index or name (a `%zone` on the SDP address, e.g.
    /// `c=IN IP6 ff12::1%eth0`, is the fallback). Unset lets the kernel
    /// choose, which only receives traffic arriving on the default-route
    /// interface and cannot pick link-local IPv6 groups (ff02::/16,
    /// ff12::/16) — those always need an explicit interface.
    /// Ignored by non-SDP sources.
    #[serde(default)]
    pub multicast_interface: Option<String>,

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

    /// Named quality tiers (bitrate presets) for this source.
    #[cfg(feature = "native-source")]
    #[serde(default)]
    pub tiers: Vec<crate::stream::source::source_config::TierSpec>,
}

/// Whether `s` can be a network interface name: kernel names are at most
/// 15 bytes (IFNAMSIZ-1) and conventionally use this charset (`eth0`,
/// `vlan.100`, `br-lan`).  Looser than the kernel on purpose — validation
/// only needs to catch mistyped IPs/indexes; a bogus name fails with a
/// clear error when the source resolves it at start.
fn is_plausible_interface_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 15
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
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
            if self.multicast_interface.is_some() {
                tracing::warn!(
                    "multicast_interface is only used by SDP file sources; it is ignored for this source"
                );
            }
            return Ok(());
        }

        let url = self.url.as_deref().unwrap_or("");
        if url.is_empty() {
            anyhow::bail!("either url or capture must be set");
        }

        let url_lower = url.to_lowercase();

        if let Some(iface) = self.multicast_interface.as_deref() {
            let iface = iface.trim();
            if iface.parse::<std::net::IpAddr>().is_err()
                && iface.parse::<u32>().is_err()
                && !is_plausible_interface_name(iface)
            {
                anyhow::bail!(
                    "multicast_interface must be an IP address, an interface index, or an interface name, got '{iface}'"
                );
            }
            let is_sdp_source = url_lower.starts_with("file://") || url_lower.ends_with(".sdp");
            if !is_sdp_source {
                tracing::warn!(
                    "multicast_interface is only used by SDP file sources; it is ignored for this source"
                );
            }
        }

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
            tiers: self.tiers.clone(),
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

#[cfg(feature = "target")]
#[cfg(test)]
mod target_tests {
    use super::*;

    #[cfg(feature = "target-rtp")]
    fn rtp_target(url: &str) -> TargetConfig {
        TargetConfig {
            url: url.to_string(),
            multicast_interface: None,
            ttl: None,
            payload_type: None,
            sdp_file: None,
        }
    }

    #[cfg(feature = "target-whip")]
    #[test]
    fn target_config_validate_accepts_whip_schemes() {
        for url in [
            "whip://edge-1:7777/whip/cam1",
            "whips://edge-1/whip/cam1",
            "whip://token@edge-1:7777/whip/cam1",
            "WHIP://edge-1/whip/cam1",
        ] {
            let target = TargetConfig {
                url: url.into(),
                multicast_interface: None,
                ttl: None,
                payload_type: None,
                sdp_file: None,
            };
            target.validate().unwrap_or_else(|e| panic!("{url}: {e}"));
        }
    }

    #[cfg(feature = "target-whip")]
    #[test]
    fn target_config_validate_rejects_bad_input() {
        for url in [
            "",
            "whep://edge-1/whep/cam1",
            "rtmp://edge-1/cam1",
            "whip://user:pass@edge-1/whip/cam1",
        ] {
            let target = TargetConfig {
                url: url.into(),
                multicast_interface: None,
                ttl: None,
                payload_type: None,
                sdp_file: None,
            };
            assert!(target.validate().is_err(), "{url} must be rejected");
        }
        // A scheme whose feature is disabled falls through to the same
        // unsupported-scheme rejection.
        #[cfg(not(feature = "target-rtsp"))]
        {
            let target = TargetConfig {
                url: "rtsp://edge-1/cam1".into(),
                multicast_interface: None,
                ttl: None,
                payload_type: None,
                sdp_file: None,
            };
            assert!(
                target.validate().is_err(),
                "rtsp:// must be rejected without the target-rtsp feature"
            );
        }
    }

    #[cfg(feature = "target-rtsp")]
    #[test]
    fn target_config_validate_accepts_rtsp_schemes() {
        for url in [
            "rtsp://mediamtx:8554/cam1",
            "rtsp://user:pass@edge-1/cam1",
            "rtsp://edge-1/cam1?transport=tcp",
            "RTSP://edge-1/cam1",
        ] {
            let target = TargetConfig {
                url: url.into(),
                multicast_interface: None,
                ttl: None,
                payload_type: None,
                sdp_file: None,
            };
            target.validate().unwrap_or_else(|e| panic!("{url}: {e}"));
        }
    }

    #[cfg(feature = "target-whip")]
    #[test]
    fn target_config_validate_rejects_whip_url_with_multicast_options() {
        let target = TargetConfig {
            url: "whip://edge-1:7777/whip/cam1".into(),
            multicast_interface: Some("192.168.1.10".into()),
            ttl: None,
            payload_type: None,
            sdp_file: None,
        };
        let err = target.validate().unwrap_err().to_string();
        assert!(
            err.contains("rtp://"),
            "error must point at rtp:// targets: {err}"
        );

        let target = TargetConfig {
            url: "whip://edge-1:7777/whip/cam1".into(),
            multicast_interface: None,
            ttl: Some(16),
            payload_type: None,
            sdp_file: None,
        };
        assert!(target.validate().is_err());
    }

    #[cfg(feature = "target-rtp")]
    #[test]
    fn target_config_validate_accepts_rtp_urls() {
        for url in [
            "rtp://230.1.1.1:1720",
            "rtp://192.168.1.10:5004",
            "rtp://[ff12::1]:1720",
            "RTP://230.1.1.1:1720",
        ] {
            let target = TargetConfig {
                url: url.into(),
                multicast_interface: None,
                ttl: None,
                payload_type: None,
                sdp_file: None,
            };
            target.validate().unwrap_or_else(|e| panic!("{url}: {e}"));
        }
    }

    #[cfg(feature = "target-rtp")]
    #[test]
    fn target_config_validate_accepts_multicast_rtp_options() {
        let target = TargetConfig {
            url: "rtp://230.1.1.1:1720".into(),
            multicast_interface: Some("192.168.1.10".into()),
            ttl: Some(16),
            payload_type: None,
            sdp_file: None,
        };
        target.validate().unwrap();

        let target = TargetConfig {
            url: "rtp://[ff12::1]:1720".into(),
            multicast_interface: Some("2".into()),
            ttl: Some(255),
            payload_type: None,
            sdp_file: None,
        };
        target.validate().unwrap();
    }

    #[cfg(feature = "target-rtp")]
    #[test]
    fn target_config_validate_rejects_bad_rtp_urls() {
        for url in [
            "rtp://",
            "rtp://230.1.1.1",
            "rtp://camera.local:1720",
            "rtp://user@230.1.1.1:1720",
            "rtp://230.1.1.1:1720/x",
            "rtp://230.1.1.1:0",
        ] {
            let target = TargetConfig {
                url: url.into(),
                multicast_interface: None,
                ttl: None,
                payload_type: None,
                sdp_file: None,
            };
            assert!(target.validate().is_err(), "{url} must be rejected");
        }
    }

    #[cfg(feature = "target-rtp")]
    #[test]
    fn target_config_validate_rejects_bad_rtp_options() {
        // TTL out of range.
        let target = TargetConfig {
            url: "rtp://230.1.1.1:1720".into(),
            multicast_interface: None,
            ttl: Some(256),
            payload_type: None,
            sdp_file: None,
        };
        assert!(target.validate().is_err());

        // IPv4 group: the interface must be an IPv4 address, not a name or
        // an interface index (those are the IPv6 form).
        for interface in ["eth0", "2"] {
            let target = TargetConfig {
                url: "rtp://230.1.1.1:1720".into(),
                multicast_interface: Some(interface.into()),
                ttl: None,
                payload_type: None,
                sdp_file: None,
            };
            let err = target.validate().unwrap_err().to_string();
            assert!(
                err.contains("multicast_interface"),
                "error must name multicast_interface: {err}"
            );
        }

        // IPv6 group: an IPv4 interface address does not fit.
        let target = TargetConfig {
            url: "rtp://[ff12::1]:1720".into(),
            multicast_interface: Some("192.168.1.10".into()),
            ttl: None,
            payload_type: None,
            sdp_file: None,
        };
        assert!(target.validate().is_err());

        // Unicast destination: multicast-only options are rejected.
        for (interface, ttl) in [(Some("192.168.1.10"), None), (None, Some(16))] {
            let target = TargetConfig {
                url: "rtp://192.168.1.10:5004".into(),
                multicast_interface: interface.map(str::to_string),
                ttl,
                payload_type: None,
                sdp_file: None,
            };
            let err = target.validate().unwrap_err().to_string();
            assert!(
                err.contains("multicast"),
                "error must say multicast-only: {err}"
            );
        }
    }

    #[cfg(feature = "target-rtp")]
    #[test]
    fn target_config_validate_accepts_payload_type_and_sdp_file() {
        let target = TargetConfig {
            url: "rtp://230.1.1.1:1720".into(),
            payload_type: Some(96),
            sdp_file: Some("/etc/live777/robot-cam.sdp".into()),
            multicast_interface: None,
            ttl: None,
        };
        target.validate().unwrap();

        // The whole dynamic range is usable.
        let target = TargetConfig {
            payload_type: Some(127),
            ..rtp_target("rtp://230.1.1.1:1720")
        };
        target.validate().unwrap();
    }

    #[cfg(feature = "target-rtp")]
    #[test]
    fn target_config_validate_rejects_payload_type_outside_dynamic_range() {
        for pt in [0, 95, 128, 200] {
            let target = TargetConfig {
                payload_type: Some(pt),
                ..rtp_target("rtp://230.1.1.1:1720")
            };
            let err = target.validate().unwrap_err().to_string();
            assert!(
                err.contains("payload_type") && err.contains("96"),
                "error must explain the dynamic range: {err}"
            );
        }
    }

    #[cfg(feature = "target-whip")]
    #[test]
    fn target_config_validate_rejects_whip_url_with_rtp_options() {
        // payload_type and sdp_file follow the same rule as the multicast
        // options: an rtp://-only knob on a WHIP target is rejected.
        let target = TargetConfig {
            url: "whip://edge-1:7777/whip/cam1".into(),
            multicast_interface: None,
            ttl: None,
            payload_type: Some(96),
            sdp_file: None,
        };
        let err = target.validate().unwrap_err().to_string();
        assert!(
            err.contains("rtp://"),
            "error must point at rtp:// targets: {err}"
        );

        let target = TargetConfig {
            url: "whip://edge-1:7777/whip/cam1".into(),
            multicast_interface: None,
            ttl: None,
            payload_type: None,
            sdp_file: Some("cam.sdp".into()),
        };
        assert!(target.validate().is_err());
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
        assert_eq!(entry.targets[0].multicast_interface, None);
        assert_eq!(entry.targets[0].ttl, None);
        assert_eq!(entry.targets[0].payload_type, None);
        assert_eq!(entry.targets[0].sdp_file, None);
    }

    #[cfg(feature = "target-rtp")]
    #[test]
    fn stream_entry_rtp_target_roundtrip_toml() {
        let entry: StreamEntry = toml::from_str(
            r#"
            [[targets]]
            url = "rtp://230.1.1.1:1720"
            multicast_interface = "192.168.123.10"
            ttl = 16
            payload_type = 96
            sdp_file = "/etc/live777/robot-cam.sdp"
            "#,
        )
        .unwrap();
        assert_eq!(entry.targets.len(), 1);
        assert_eq!(entry.targets[0].url, "rtp://230.1.1.1:1720");
        assert_eq!(
            entry.targets[0].multicast_interface.as_deref(),
            Some("192.168.123.10")
        );
        assert_eq!(entry.targets[0].ttl, Some(16));
        assert_eq!(entry.targets[0].payload_type, Some(96));
        assert_eq!(
            entry.targets[0].sdp_file.as_deref(),
            Some("/etc/live777/robot-cam.sdp")
        );
    }

    #[test]
    fn config_validate_reports_stream_target_error() {
        let mut cfg = Config::default();
        cfg.stream.streams.insert(
            "cam1".to_string(),
            StreamEntry {
                targets: vec![TargetConfig {
                    url: "whep://edge-1/whep/cam1".into(),
                    multicast_interface: None,
                    ttl: None,
                    payload_type: None,
                    sdp_file: None,
                }],
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("cam1"), "error must name the stream: {err}");
    }

    #[test]
    fn config_validate_rejects_duplicate_target_urls() {
        // Both schemes funnel through the same duplicate check; use whichever
        // this feature set validates.
        let url = if cfg!(feature = "target-whip") {
            "whip://edge-1:7777/whip/cam1"
        } else {
            "rtp://230.1.1.1:1720"
        };
        let mut cfg = Config::default();
        cfg.stream.streams.insert(
            "cam1".to_string(),
            StreamEntry {
                targets: vec![
                    TargetConfig {
                        url: url.into(),
                        multicast_interface: None,
                        ttl: None,
                        payload_type: None,
                        sdp_file: None,
                    },
                    TargetConfig {
                        url: url.into(),
                        multicast_interface: None,
                        ttl: None,
                        payload_type: None,
                        sdp_file: None,
                    },
                ],
                ..Default::default()
            },
        );
        let err = cfg.validate().unwrap_err().to_string();
        assert!(err.contains("duplicate"), "error must say duplicate: {err}");
    }
}
