use std::fs;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    pub load_balancing: String,
    pub listen_addr: String,
    pub model: String,
    pub addr: String,
}

impl Config {
    pub fn parse() -> Self {
        let result = fs::read_to_string("config.toml")
            .or_else(|_| fs::read_to_string("/etc/live777/config.toml"));
        if let Ok(cfg_str) = result {
            if let Ok(cfg) = toml::from_str::<Self>(&cfg_str) {
                return cfg; 
            }
        }
        Config {
            load_balancing: "random".to_string(),
            listen_addr: "127.0.0.1:8080".to_string(),
            model: "RedisStandalone".to_string(),
            addr: "redis://127.0.0.1:6379".to_string(),
        }
    }
}

