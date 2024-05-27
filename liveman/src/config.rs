use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{env, fs, net::SocketAddr, str::FromStr};

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub http: Http,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub manager_auth: Auth,
    #[serde(default)]
    pub log: Log,
    #[serde(default)]
    pub reforward: Reforward,
    #[serde(default)]
    pub servers: Vec<crate::route::embed::Server>,
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
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub tokens: Vec<String>,
}

impl Auth {
    pub fn to_authorizations(&self) -> Vec<String> {
        let mut authorizations = vec![];
        for account in self.accounts.iter() {
            authorizations.push(account.to_authorization());
        }
        for token in self.tokens.iter() {
            authorizations.push(format!("Bearer {}", token));
        }
        authorizations
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
}

impl Account {
    pub fn to_authorization(&self) -> String {
        let encoded = STANDARD.encode(format!("{}:{}", self.username, self.password));
        format!("Basic {}", encoded)
    }
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
        env::var("PORT").unwrap_or(String::from("8080"))
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
    //#[serde(default)]
    //pub cascade: bool,
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
        let result = fs::read_to_string(path.unwrap_or(String::from("gateway.toml")))
            .or(fs::read_to_string("/etc/live777/gateway.toml"))
            .unwrap_or("".to_string());
        let cfg: Self = toml::from_str(result.as_str()).expect("config parse error");
        cfg
    }
}

fn default_reforward_maximum_idle_time() -> u64 {
    60 * 1000
}
