use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// UDP server configuration
    pub udp: UdpConfig,
    
    /// Liveion server configuration
    pub liveion: LiveionConfig,
    
    /// Bridge configuration
    pub bridge: BridgeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdpConfig {
    /// UDP listen address
    #[serde(default = "default_udp_listen")]
    pub listen: String,
    
    /// UDP listen port
    #[serde(default = "default_udp_port")]
    pub port: u16,
    
    /// Default target addresses for broadcasting messages
    #[serde(default = "default_target_addresses")]
    pub target_addresses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveionConfig {
    /// Liveion server URL
    #[serde(default = "default_liveion_url")]
    pub url: String,
    
    /// Stream name to connect to
    #[serde(default = "default_stream_name")]
    pub stream: String,
    
    /// Authentication credentials
    pub auth: Option<AuthConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Username for authentication
    pub username: String,
    
    /// Password for authentication
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    /// Reconnection interval in seconds
    #[serde(default = "default_reconnect_interval")]
    pub reconnect_interval: u64,
    
    /// Maximum message size in bytes
    #[serde(default = "default_max_message_size")]
    pub max_message_size: usize,
    
    /// Enable message logging
    #[serde(default = "default_enable_logging")]
    pub enable_logging: bool,
}

fn default_udp_listen() -> String {
    "0.0.0.0".to_string()
}

fn default_udp_port() -> u16 {
    8888
}

fn default_liveion_url() -> String {
    "http://localhost:7777".to_string()
}

fn default_stream_name() -> String {
    "camera".to_string()
}

fn default_reconnect_interval() -> u64 {
    5
}

fn default_max_message_size() -> usize {
    1024 * 16
}

fn default_enable_logging() -> bool {
    true
}

fn default_target_addresses() -> Vec<String> {
    vec!["localhost:8889".to_string()]
}

impl Default for Config {
    fn default() -> Self {
        Self {
            udp: UdpConfig {
                listen: default_udp_listen(),
                port: default_udp_port(),
                target_addresses: default_target_addresses(),
            },
            liveion: LiveionConfig {
                url: default_liveion_url(),
                stream: default_stream_name(),
                auth: None,
            },
            bridge: BridgeConfig {
                reconnect_interval: default_reconnect_interval(),
                max_message_size: default_max_message_size(),
                enable_logging: default_enable_logging(),
            },
        }
    }
}

impl Config {
    pub async fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        
        if !path.exists() {
            // Create default config file
            let default_config = Self::default();
            let toml_content = toml::to_string_pretty(&default_config)?;
            fs::write(path, toml_content).await?;
            tracing::info!("Created default configuration file at {:?}", path);
            return Ok(default_config);
        }
        
        let content = fs::read_to_string(path).await?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}