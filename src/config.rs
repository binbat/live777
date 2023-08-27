use std::fs;

use serde::{Deserialize, Serialize};
use webrtc::ice_transport::ice_server::RTCIceServer;

#[derive(Clone, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub ice_servers: Vec<IceServer>,
}

#[derive(Clone, Deserialize, Serialize)]
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

impl Into<RTCIceServer> for IceServer {
    fn into(self) -> RTCIceServer {
        RTCIceServer {
            urls: self.urls,
            username: self.username,
            credential: self.credential,
            credential_type: self.credential_type.as_str().into(),
        }
    }
}


impl Config {
    pub(crate) fn parse() -> Self {
        let mut result = fs::read_to_string("config.json");
        if result.is_err() {
            result = fs::read_to_string("/etc/live777/config.json");
        }
        if let Ok(cfg) = result {
            serde_json::from_str(cfg.as_str()).expect("config parse error")
        } else {
            Config {
                ice_servers: vec![IceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_string()],
                    username: "".to_string(),
                    credential: "".to_string(),
                    credential_type: "".to_string(),
                }],
            }
        }
    }
}
