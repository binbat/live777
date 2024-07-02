use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::{env, fs, net::SocketAddr, str::FromStr};
use webrtc::{ice, ice_transport::ice_server::RTCIceServer, Error};

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
    pub strategy: Strategy,
    #[serde(default)]
    pub webhook: Webhook,
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Webhook {
    #[serde(default)]
    pub webhooks: Vec<String>,
    #[serde(default)]
    pub node_addr: Option<SocketAddr>,
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
pub struct Strategy {
    #[serde(default)]
    pub reforward_close_sub: bool,
    #[serde(default = "default_true")]
    pub auto_create_whip: bool,
    #[serde(default = "default_true")]
    pub auto_create_whep: bool,
    #[serde(default)]
    pub auto_delete_whip: AutoDestrayTime,
    #[serde(default)]
    pub auto_delete_whep: AutoDestrayTime,
}

fn default_true() -> bool {
    true
}

impl Default for Strategy {
    fn default() -> Self {
        Self {
            reforward_close_sub: false,
            auto_create_whip: true,
            auto_create_whep: true,
            auto_delete_whip: AutoDestrayTime::default(),
            auto_delete_whep: AutoDestrayTime::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoDestrayTime(pub i64);

impl Default for AutoDestrayTime {
    fn default() -> Self {
        AutoDestrayTime(60)
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

fn default_ice_servers() -> Vec<IceServer> {
    vec![IceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_string()],
        username: "".to_string(),
        credential: "".to_string(),
        credential_type: "".to_string(),
    }]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IceServer {
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub credential: String,
    #[serde(default)]
    pub credential_type: String,
}

// from https://github.com/webrtc-rs/webrtc/blob/71157ba2153a891a8cfd819f3cf1441a7a0808d8/webrtc/src/ice_transport/ice_server.rs
impl IceServer {
    pub(crate) fn parse_url(&self, url_str: &str) -> webrtc::error::Result<ice::url::Url> {
        Ok(ice::url::Url::parse_url(url_str)?)
    }

    pub(crate) fn validate(&self) -> webrtc::error::Result<()> {
        self.urls()?;
        Ok(())
    }

    pub(crate) fn urls(&self) -> webrtc::error::Result<Vec<ice::url::Url>> {
        let mut urls = vec![];

        for url_str in &self.urls {
            let mut url = self.parse_url(url_str)?;
            if url.scheme == ice::url::SchemeType::Turn || url.scheme == ice::url::SchemeType::Turns
            {
                // https://www.w3.org/TR/webrtc/#set-the-configuration (step #11.3.2)
                if self.username.is_empty() || self.credential.is_empty() {
                    return Err(Error::ErrNoTurnCredentials);
                }
                url.username.clone_from(&self.username);
            }
            urls.push(url);
        }

        Ok(urls)
    }
}

impl From<IceServer> for RTCIceServer {
    fn from(val: IceServer) -> Self {
        RTCIceServer {
            urls: val.urls,
            username: val.username,
            credential: val.credential,
        }
    }
}

impl Config {
    pub fn parse(path: Option<String>) -> Self {
        let result = fs::read_to_string(path.unwrap_or(String::from("live777.toml")))
            .or(fs::read_to_string("/etc/live777/live777.toml"))
            .unwrap_or("".to_string());
        let cfg: Self = toml::from_str(result.as_str()).expect("config parse error");
        match cfg.validate() {
            Ok(_) => cfg,
            Err(err) => panic!("config validate [{}]", err),
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        for ice_server in self.ice_servers.iter() {
            ice_server
                .validate()
                .map_err(|e| anyhow::anyhow!(format!("ice_server error : {}", e)))?;
        }
        Ok(())
    }
}
