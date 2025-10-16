use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Serialize};
use std::{env, net::SocketAddr, str::FromStr};
use tracing::{info, warn};
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct Config {
    #[serde(default)]
    pub http: Http,
    #[serde(default)]
    pub log: Log,
    pub cameras: Vec<CameraConfig>,
    #[serde(default)]
    pub ice_servers: Vec<iceserver::IceServer>,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub stream: StreamConfig,
    #[serde(default)]
    pub network: NetworkConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default = "default_protocol")]
    pub protocol: String,
    #[serde(default)]
    pub static_ip: StaticIpConfig,
    #[serde(default)]
    pub ntp: NtpConfig,
    #[serde(default)]
    pub camera: CameraSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    #[serde(default = "default_command")]
    pub command: String,
    #[serde(default = "default_rtp_port")]
    pub rtp_port: u16,
}

fn default_command() -> String {
    "ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libx264 -profile:v baseline -level 3.1 -pix_fmt yuv420p -g 15 -keyint_min 15 -b:v 1000k -minrate 1000k -maxrate 1000k -bufsize 1000k -preset ultrafast -tune zerolatency -x264-params repeat_headers=1 -f rtp rtp://127.0.0.1:5004?pkt_size=1200".to_string()
}

fn default_rtp_port() -> u16 {
    5004
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub username: String,
    pub password_hash: String,
    #[serde(default = "default_jwt_secret")]
    pub jwt_secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Log {
    #[serde(default = "default_log_level")]
    pub level: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticIpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub ip: String,
    #[serde(default = "default_netmask")]
    pub netmask: String,
    #[serde(default)]
    pub gateway: String,
    #[serde(default = "default_dns")]
    pub dns: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NtpConfig {
    #[serde(default = "default_ntp_enabled")]
    pub enabled: bool,
    #[serde(default = "default_ntp_server")]
    pub server: String,
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraSettings {
    #[serde(default = "default_resolution")]
    pub resolution: String,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_bitrate")]
    pub bitrate: u32,
}

fn default_protocol() -> String {
    "rtp".to_string()
}

fn default_netmask() -> String {
    "255.255.255.0".to_string()
}

fn default_dns() -> String {
    "8.8.8.8".to_string()
}

fn default_ntp_enabled() -> bool {
    true
}

fn default_ntp_server() -> String {
    "pool.ntp.org".to_string()
}

fn default_timezone() -> String {
    "UTC".to_string()
}

fn default_resolution() -> String {
    "1280x720".to_string()
}

fn default_fps() -> u32 {
    30
}

fn default_bitrate() -> u32 {
    2000
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            protocol: default_protocol(),
            static_ip: StaticIpConfig::default(),
            ntp: NtpConfig::default(),
            camera: CameraSettings::default(),
        }
    }
}

impl Default for StaticIpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ip: "192.168.42.1".to_string(),
            netmask: "255.255.255.0".to_string(),
            gateway: "192.168.42.1".to_string(),
            dns: default_dns(),
        }
    }
}

impl Default for NtpConfig {
    fn default() -> Self {
        Self {
            enabled: default_ntp_enabled(),
            server: default_ntp_server(),
            timezone: default_timezone(),
        }
    }
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            resolution: default_resolution(),
            fps: default_fps(),
            bitrate: default_bitrate(),
        }
    }
}

fn default_http_listen() -> SocketAddr {
    SocketAddr::from_str(&format!(
        "0.0.0.0:{}",
        env::var("PORT").unwrap_or(String::from("9999"))
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

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            username: "admin".to_string(),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$bmljZXRyeQ$PqTT/n9ToBNVsdsoquTz1A/P5s9O4yvA9fym5Vd5s9s".to_string(),
            jwt_secret: default_jwt_secret(),
        }
    }
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            command: default_command(),
            rtp_port: default_rtp_port(),
        }
    }
}

fn default_jwt_secret() -> String {
    let random_bytes: [u8; 32] = rand::random();
    general_purpose::URL_SAFE_NO_PAD.encode(random_bytes)
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
pub struct CameraConfig {
    pub id: String,
    pub rtp_port: u16,
    pub codec: CodecConfig,
    #[serde(default)]
    pub command: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CodecConfig {
    pub mime_type: String,
    pub clock_rate: u32,
    pub channels: u16,
    pub sdp_fmtp_line: Option<String>,
}

impl From<CodecConfig> for RTCRtpCodecCapability {
    fn from(val: CodecConfig) -> Self {
        RTCRtpCodecCapability {
            mime_type: val.mime_type,
            clock_rate: val.clock_rate,
            channels: val.channels,
            sdp_fmtp_line: val.sdp_fmtp_line.unwrap_or_default(),
            rtcp_feedback: vec![], // TODO
        }
    }
}

impl Config {
    pub fn validate(&mut self) -> anyhow::Result<()> {
        if self.http.public.is_empty() {
            self.http.public = format!("http://{}", self.http.listen);
        }

        if self.auth.jwt_secret.is_empty() {
            warn!(
                "auth.jwt_secret is empty or not set. A random secret will be used for this session."
            );
            self.auth.jwt_secret = default_jwt_secret();
        }

        let global_port_str = self.stream.rtp_port.to_string();
        for cam in &mut self.cameras {
            if cam.command.is_empty() {
                cam.command = self
                    .stream
                    .command
                    .replace(&global_port_str, &cam.rtp_port.to_string());
                info!("Filled command for camera {}: {}", cam.id, cam.command);
            }
            if !cam.command.contains(&format!("127.0.0.1:{}", cam.rtp_port)) {
                warn!(
                    "Camera {} command does not include rtp_port {}, may need update.",
                    cam.id, cam.rtp_port
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation() {
        let mut config = Config {
            http: Http {
                listen: "127.0.0.1:8080".parse().unwrap(),
                public: String::new(),
                cors: false,
            },
            cameras: vec![CameraConfig {
                id: "test_cam".to_string(),
                rtp_port: 5004,
                codec: CodecConfig {
                    mime_type: "video/H264".to_string(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: None,
                },
                command: String::new(),
            }],
            ..Default::default()
        };

        assert!(config.validate().is_ok());
        assert!(!config.http.public.is_empty());
        assert!(!config.cameras[0].command.is_empty());
    }

    #[test]
    fn test_network_config_defaults() {
        let network_config = NetworkConfig::default();
        assert_eq!(network_config.protocol, "rtp");
        assert!(!network_config.static_ip.enabled);
        assert!(network_config.ntp.enabled);
        assert_eq!(network_config.camera.resolution, "1280x720");
        assert_eq!(network_config.camera.fps, 30);
    }

    #[test]
    fn test_auth_config_jwt_secret() {
        let auth_config = AuthConfig::default();
        assert!(!auth_config.jwt_secret.is_empty());
        assert_eq!(auth_config.username, "admin");
    }
}
