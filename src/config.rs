use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use local_ip_address::local_ip;
use serde::{Deserialize, Serialize};
use std::{env, fs};
use webrtc::{
    ice,
    ice_transport::{ice_credential_type::RTCIceCredentialType, ice_server::RTCIceServer},
    Error,
};

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub http: Http,
    #[serde(default = "default_ice_servers")]
    pub ice_servers: Vec<IceServer>,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub admin_auth: Auth,
    #[serde(default)]
    pub log: Log,
    #[serde(default)]
    pub publish_leave_timeout: PublishLeaveTimeout,
    #[serde(default)]
    pub node_info: NodeInfo,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Http {
    #[serde(default = "default_http_listen")]
    pub listen: String,
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

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "model")]
pub enum StorageModel {
    RedisStandalone { addr: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeInfo {
    pub storage: Option<StorageModel>,
    #[serde(default = "default_registry_ip_port")]
    pub ip_port: String,
    #[serde(default)]
    pub meta_data: MetaData,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetaData {
    #[serde(default)]
    pub pub_max: MetaDataPubMax,
    #[serde(default)]
    pub sub_max: MetaDataSubMax,
    #[serde(default)]
    pub reforward_maximum_idle_time: ReforwardMaximumIdleTime,
    #[serde(default)]
    pub reforward_cascade: bool,
    #[serde(default)]
    pub reforward_close_sub: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaDataPubMax(pub u64);

impl Default for MetaDataPubMax {
    fn default() -> Self {
        MetaDataPubMax(u64::MAX)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaDataSubMax(pub u64);

impl Default for MetaDataSubMax {
    fn default() -> Self {
        MetaDataSubMax(u64::MAX)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReforwardMaximumIdleTime(pub u64);

impl Default for ReforwardMaximumIdleTime {
    fn default() -> Self {
        ReforwardMaximumIdleTime(1800000)
    }
}

fn default_http_listen() -> String {
    format!(
        "0.0.0.0:{}",
        env::var("PORT").unwrap_or(String::from("7777"))
    )
}

fn default_registry_ip_port() -> String {
    format!(
        "{}:{}",
        local_ip().unwrap(),
        env::var("PORT").unwrap_or(String::from("7777"))
    )
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
                url.username = self.username.clone();

                match self.credential_type.as_str().into() {
                    RTCIceCredentialType::Password => {
                        // https://www.w3.org/TR/webrtc/#set-the-configuration (step #11.3.3)
                        url.password = self.credential.clone();
                    }
                    RTCIceCredentialType::Oauth => {
                        // https://www.w3.org/TR/webrtc/#set-the-configuration (step #11.3.4)
                        /*if _, ok: = s.Credential.(OAuthCredential); !ok {
                                return nil,
                                &rtcerr.InvalidAccessError{Err: ErrTurnCredentials
                            }
                        }*/
                    }
                    _ => return Err(Error::ErrTurnCredentials),
                };
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
            credential_type: val.credential_type.as_str().into(),
        }
    }
}

impl Config {
    pub(crate) fn parse(path: Option<String>) -> Self {
        let result = fs::read_to_string(path.unwrap_or_else(|| String::from("config.toml")))
            .or_else(|_| fs::read_to_string("/etc/live777/config.toml"))
            .unwrap_or("".to_string());
        let cfg: Self = toml::from_str(result.as_str()).expect("config parse error");
        match cfg.validate() {
            Ok(_) => cfg,
            Err(err) => panic!("config validate [{}]", err),
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        if (!self.auth.accounts.is_empty() || !self.auth.tokens.is_empty())
            && (self.admin_auth.accounts.is_empty() && self.admin_auth.tokens.is_empty())
        {
            return Err(anyhow::anyhow!("auth not empty,but admin auth empty"));
        }
        if self.node_info.meta_data.pub_max.0 == 0 {
            return Err(anyhow::anyhow!(
                "node_info.meta_data.pub_max cannot be equal to 0"
            ));
        }
        if self.node_info.meta_data.sub_max.0 == 0 {
            return Err(anyhow::anyhow!(
                "node_info.meta_data.sub_max cannot be equal to 0"
            ));
        }
        if self.node_info.meta_data.pub_max.0 > self.node_info.meta_data.sub_max.0 {
            return Err(anyhow::anyhow!(
                "node_info.meta_data.pub_max cannot be greater than node_info.meta_data.sub_max"
            ));
        }
        for ice_server in self.ice_servers.iter() {
            ice_server
                .validate()
                .map_err(|e| anyhow::anyhow!(format!("ice_server error : {}", e)))?;
        }
        Ok(())
    }
}
