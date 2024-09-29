use serde::{Deserialize, Serialize};
use std::{env, fs, net::SocketAddr, str::FromStr};

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub http: Http,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub log: Log,
    #[serde(default)]
    pub liveion: Vec<crate::mem::Server>,
    #[serde(default)]
    pub reforward: Reforward,

    #[cfg(feature = "net4mqtt")]
    #[serde(default)]
    pub net4mqtt: Option<Net4mqtt>,

    #[serde(default)]
    pub nodes: Vec<crate::mem::Server>,
}

#[cfg(feature = "net4mqtt")]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Net4mqtt {
    #[serde(default)]
    pub mqtt_url: String,
    #[serde(default)]
    pub alias: String,
    #[serde(default = "default_socks_listen")]
    pub listen: SocketAddr,
}

#[cfg(feature = "net4mqtt")]
impl Default for Net4mqtt {
    fn default() -> Self {
        Self {
            mqtt_url: String::new(),
            alias: String::new(),
            listen: default_socks_listen(),
        }
    }
}

#[cfg(feature = "net4mqtt")]
fn default_socks_listen() -> SocketAddr {
    SocketAddr::from_str("0.0.0.0:1077").expect("invalid listen socks address")
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Reforward {
    #[serde(default)]
    pub check_attempts: ReforwardCheckAttempts,
    #[serde(default)]
    pub check_tick_time: CheckReforwardTickTime,
    #[serde(default = "default_reforward_maximum_idle_time")]
    pub maximum_idle_time: u64,
    #[serde(default)]
    pub close_other_sub: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReforwardCheckAttempts(pub u8);

impl Default for ReforwardCheckAttempts {
    fn default() -> Self {
        ReforwardCheckAttempts(5)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReforwardTickTime(pub u64);

impl Default for CheckReforwardTickTime {
    fn default() -> Self {
        CheckReforwardTickTime(60 * 1000)
    }
}

impl Config {
    pub(crate) fn parse(path: Option<String>) -> Self {
        let result = fs::read_to_string(path.unwrap_or(String::from("liveman.toml")))
            .or(fs::read_to_string("/etc/live777/liveman.toml"))
            .unwrap_or("".to_string());
        let cfg: Self = toml::from_str(result.as_str()).expect("config parse error");
        match cfg.validate() {
            Ok(_) => cfg,
            Err(err) => panic!("config validate [{}]", err),
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

fn default_reforward_maximum_idle_time() -> u64 {
    60 * 1000
}
