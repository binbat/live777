use serde::{Deserialize, Serialize};
use std::{env, net::SocketAddr, str::FromStr};

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub http: Http,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub log: Log,
    #[serde(default)]
    pub liveion: Vec<Node>,
    #[serde(default)]
    pub cascade: Cascade,

    #[cfg(feature = "net4mqtt")]
    #[serde(default)]
    pub net4mqtt: Option<Net4mqtt>,

    #[serde(default)]
    pub nodes: Vec<Node>,

    #[serde(default)]
    pub database: Database,

    #[serde(default)]
    pub playback: Playback,

    /// Auto recording configuration (Liveman-driven)
    #[serde(default)]
    pub auto_record: AutoRecord,

    #[cfg(feature = "recorder")]
    #[serde(default)]
    pub recorder: Recorder,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    #[serde(default)]
    pub alias: String,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub url: String,
}

#[cfg(feature = "net4mqtt")]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Net4mqtt {
    #[serde(default)]
    pub mqtt_url: String,
    #[serde(default)]
    pub alias: String,
    #[serde(default = "default_net4mqtt_listen")]
    pub listen: SocketAddr,
    #[serde(default = "default_net4mqtt_domain")]
    pub domain: String,
}

#[cfg(feature = "net4mqtt")]
impl Net4mqtt {
    pub fn validate(&mut self) {
        self.mqtt_url = self.mqtt_url.replace("{alias}", &self.alias)
    }
}

#[cfg(feature = "net4mqtt")]
impl Default for Net4mqtt {
    fn default() -> Self {
        Self {
            mqtt_url: String::new(),
            alias: String::new(),
            listen: default_net4mqtt_listen(),
            domain: default_net4mqtt_domain(),
        }
    }
}

#[cfg(feature = "net4mqtt")]
fn default_net4mqtt_listen() -> SocketAddr {
    SocketAddr::from_str("0.0.0.0:1077").expect("invalid listen socks address")
}

#[cfg(feature = "net4mqtt")]
fn default_net4mqtt_domain() -> String {
    String::from("net4mqtt.local")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Http {
    #[serde(default = "default_http_listen")]
    pub listen: SocketAddr,
    #[serde(default)]
    pub cors: bool,
    #[serde(default)]
    pub public: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Auth {
    #[serde(default)]
    pub secret: String,
    #[serde(default)]
    pub tokens: Vec<String>,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Log {
    #[serde(default = "default_log_level")]
    pub level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishLeaveTimeout(pub u64);

impl Default for PublishLeaveTimeout {
    fn default() -> Self {
        PublishLeaveTimeout(15000)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSyncTickTime(pub u64);

impl Default for NodeSyncTickTime {
    fn default() -> Self {
        NodeSyncTickTime(5000)
    }
}

fn default_http_listen() -> SocketAddr {
    SocketAddr::from_str(&format!(
        "0.0.0.0:{}",
        env::var("PORT").unwrap_or(String::from("8888"))
    ))
    .expect("invalid listen address")
}

impl Default for Http {
    fn default() -> Self {
        Self {
            listen: default_http_listen(),
            public: Default::default(),
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

fn default_log_level() -> String {
    env::var("LOG_LEVEL").unwrap_or_else(|_| {
        if cfg!(debug_assertions) {
            "debug".to_string()
        } else {
            "info".to_string()
        }
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum CascadeMode {
    #[default]
    Push,
    Pull,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Cascade {
    #[serde(default)]
    pub check_attempts: CascadeCheckAttempts,
    #[serde(default)]
    pub check_tick_time: CheckCascadeTickTime,
    #[serde(default = "default_reforward_maximum_idle_time")]
    pub maximum_idle_time: u64,
    #[serde(default)]
    pub close_other_sub: bool,

    #[serde(default)]
    pub mode: CascadeMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CascadeCheckAttempts(pub u8);

impl Default for CascadeCheckAttempts {
    fn default() -> Self {
        CascadeCheckAttempts(5)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckCascadeTickTime(pub u64);

impl Default for CheckCascadeTickTime {
    fn default() -> Self {
        CheckCascadeTickTime(60 * 1000)
    }
}

impl Config {
    pub fn validate(&mut self) -> anyhow::Result<()> {
        if self.http.public.is_empty() {
            self.http.public = format!("http://{}", self.http.listen);
        }
        Ok(())
    }
}

fn default_reforward_maximum_idle_time() -> u64 {
    60 * 1000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Database {
    #[serde(default = "default_database_url")]
    pub url: String,
    #[serde(default = "default_database_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_database_connect_timeout")]
    pub connect_timeout: u64,
}

impl Default for Database {
    fn default() -> Self {
        Self {
            url: default_database_url(),
            max_connections: default_database_max_connections(),
            connect_timeout: default_database_connect_timeout(),
        }
    }
}

fn default_database_url() -> String {
    env::var("DATABASE_URL").unwrap_or_else(|_| "postgresql://localhost/live777".to_string())
}

fn default_database_max_connections() -> u32 {
    10
}

fn default_database_connect_timeout() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Playback {
    /// Whether to use signed redirects or direct proxy for segment access
    #[serde(default = "default_signed_redirect")]
    pub signed_redirect: bool,

    /// TTL in seconds for signed URLs (only used if signed_redirect is true)
    #[serde(default = "default_signed_ttl_seconds")]
    pub signed_ttl_seconds: u64,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            signed_redirect: default_signed_redirect(),
            signed_ttl_seconds: default_signed_ttl_seconds(),
        }
    }
}

fn default_signed_redirect() -> bool {
    false
}

fn default_signed_ttl_seconds() -> u64 {
    60
}

#[cfg(feature = "recorder")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Recorder {
    #[serde(default)]
    pub storage: storage::StorageConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoRecord {
    #[serde(default)]
    pub auto_streams: Vec<String>,
    #[serde(default)]
    pub base_prefix: String,
    #[serde(default = "default_auto_record_tick")]
    pub tick_ms: u64,
    #[serde(default)]
    pub enabled: bool,
}

impl Default for AutoRecord {
    fn default() -> Self {
        Self {
            auto_streams: vec![],
            base_prefix: String::new(),
            tick_ms: 5_000,
            enabled: false,
        }
    }
}

fn default_auto_record_tick() -> u64 {
    5_000
}
