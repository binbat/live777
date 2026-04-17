use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================
// 新的 Paths 配置结构
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub http: HttpConfig,
    pub log: LogConfig,
    #[serde(default)]
    pub webrtc: WebRtcConfig,
    
    // ============ 新增：Paths 配置 ============
    #[serde(default)]
    pub path_defaults: PathConfig,
    
    #[serde(default)]
    pub paths: HashMap<String, PathConfig>,
    
    // ============ 向后兼容：旧配置 ============
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<StreamConfig>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraConfig>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whip: Option<WhipConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathConfig {
    /// 数据源类型
    #[serde(default)]
    pub source: SourceType,
    
    /// 按需启动
    #[serde(default = "default_true")]
    pub source_on_demand: bool,
    
    /// 最大订阅者数 (0 = 无限)
    #[serde(default)]
    pub max_readers: usize,
    
    /// RTP 端口（某些源需要）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtp_port: Option<u16>,
    
    /// RTP 目标地址（默认为 127.0.0.1）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtp_dest: Option<String>,
    
    /// Codec 配置
    #[serde(default)]
    pub codec: CodecConfig,
    
    /// Libcamera 配置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libcamera: Option<LibcameraConfig>,
    
    /// V4L2 配置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub v4l2: Option<V4l2Config>,
    
    /// WHIP 配置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whip: Option<WhipConfig>,
    
    /// RTSP 配置
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtsp: Option<RtspConfig>,
}

fn default_true() -> bool {
    true
}

impl Default for PathConfig {
    fn default() -> Self {
        Self {
            source: SourceType::default(),
            source_on_demand: true,
            max_readers: 0,
            rtp_port: None,
            rtp_dest: None,
            codec: CodecConfig::default(),
            libcamera: None,
            v4l2: None,
            whip: None,
            rtsp: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    /// 等待 WHEP 推流
    Publisher,
    
    /// libcamera（树莓派摄像头）via custom libcamera-bridge
    Libcamera,
    
    /// rpicam-vid（树莓派官方工具，自动硬件编码）
    Rpicam,
    
    /// V4L2 直接捕获
    V4l2,
    
    /// WHIP 推流到其他服务器
    Whip,
    
    /// RTSP URL 拉流
    Rtsp(String),
    
    /// 本地文件
    File(String),
}

impl Default for SourceType {
    fn default() -> Self {
        SourceType::Publisher
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibcameraConfig {
    #[serde(default = "default_width")]
    pub width: u32,
    
    #[serde(default = "default_height")]
    pub height: u32,
    
    #[serde(default = "default_fps")]
    pub fps: u32,
    
    #[serde(default = "default_bitrate")]
    pub bitrate: u32,
    
    #[serde(default = "default_codec_str")]
    pub codec: String,
    
    #[serde(default)]
    pub camera_id: u32,
    
    #[serde(default)]
    pub rotation: u32,
    
    #[serde(default)]
    pub hflip: bool,
    
    #[serde(default)]
    pub vflip: bool,
}

fn default_width() -> u32 { 1920 }
fn default_height() -> u32 { 1080 }
fn default_fps() -> u32 { 30 }
fn default_bitrate() -> u32 { 2_000_000 }
fn default_codec_str() -> String { "h264".to_string() }

impl Default for LibcameraConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
            bitrate: 2_000_000,
            codec: "h264".to_string(),
            camera_id: 0,
            rotation: 0,
            hflip: false,
            vflip: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RtspConfig {
    #[serde(default = "default_rtsp_transport")]
    pub transport: String,
    
    #[serde(default = "default_timeout")]
    pub timeout: u32,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer_size: Option<usize>,
}

fn default_rtsp_transport() -> String { "tcp".to_string() }
fn default_timeout() -> u32 { 10 }

impl Default for RtspConfig {
    fn default() -> Self {
        Self {
            transport: "tcp".to_string(),
            timeout: 10,
            buffer_size: None,
        }
    }
}

// ============================================================
// 旧配置结构（向后兼容）
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpConfig {
    pub listen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogConfig {
    pub level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    pub id: String,
    pub rtp_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureSource {
    Ffmpeg,
    V4l2,
}

impl Default for CaptureSource {
    fn default() -> Self {
        CaptureSource::Ffmpeg
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V4l2Config {
    pub device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: String,
    #[serde(default)]
    pub bitrate: Option<u32>,
}

impl Default for V4l2Config {
    fn default() -> Self {
        Self {
            device: "/dev/video0".to_string(),
            width: 640,
            height: 480,
            fps: 30,
            format: "H264".to_string(),
            bitrate: Some(1_000_000),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraConfig {
    pub device: String,
    #[serde(default)]
    pub source: CaptureSource,
    #[serde(default)]
    pub command: Option<String>,
    pub codec: CodecConfig,
    #[serde(default)]
    pub v4l2: Option<V4l2Config>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodecConfig {
    pub mime_type: String,
    pub clock_rate: u32,
    #[serde(default)]
    pub channels: u16,
    #[serde(default)]
    pub sdp_fmtp_line: Option<String>,
}

impl Default for CodecConfig {
    fn default() -> Self {
        Self {
            mime_type: "video/H264".to_string(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: Some(
                "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                    .to_string(),
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebRtcConfig {
    #[serde(default)]
    pub ice_servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Whep,
    Whip,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhipConfig {
    pub url: String,
    pub token: Option<String>,
}

// ============================================================
// 配置验证和辅助方法
// ============================================================

impl Config {
    pub fn validate(&self) -> anyhow::Result<()> {
        // 新配置验证
        if !self.paths.is_empty() {
            for (name, path_config) in &self.paths {
                self.validate_path(name, path_config)?;
            }
            return Ok(());
        }
        
        // 旧配置验证（向后兼容）
        if let Some(stream) = &self.stream {
            if stream.id.is_empty() {
                anyhow::bail!("stream.id cannot be empty");
            }
            if stream.rtp_port == 0 {
                anyhow::bail!("stream.rtp_port must be greater than 0");
            }
        }
        
        if let Some(mode) = &self.mode {
            if *mode == Mode::Whip && self.whip.is_none() {
                anyhow::bail!("WHIP mode requires [whip] configuration");
            }
        }
        
        if let Some(camera) = &self.camera {
            match camera.source {
                CaptureSource::Ffmpeg => {
                    if camera.command.is_none() || camera.command.as_ref().unwrap().is_empty() {
                        anyhow::bail!("camera.command cannot be empty when source is ffmpeg");
                    }
                }
                CaptureSource::V4l2 => {
                    if camera.v4l2.is_none() {
                        anyhow::bail!("camera.v4l2 config is required when source is v4l2");
                    }
                }
            }
        }
        
        Ok(())
    }
    
    fn validate_path(&self, name: &str, config: &PathConfig) -> anyhow::Result<()> {
        if name.is_empty() {
            anyhow::bail!("Path name cannot be empty");
        }
        
        match &config.source {
            SourceType::Libcamera | SourceType::Rpicam => {
                if config.libcamera.is_none() {
                    anyhow::bail!(
                        "Path '{}': libcamera/rpicam source requires [paths.{}.libcamera] config",
                        name, name
                    );
                }
                if config.rtp_port.is_none() {
                    anyhow::bail!("Path '{}': libcamera/rpicam source requires rtp_port", name);
                }
            }
            SourceType::V4l2 => {
                if config.v4l2.is_none() {
                    anyhow::bail!(
                        "Path '{}': v4l2 source requires [paths.{}.v4l2] config",
                        name, name
                    );
                }
                if config.rtp_port.is_none() {
                    anyhow::bail!("Path '{}': v4l2 source requires rtp_port", name);
                }
            }
            SourceType::Whip => {
                if config.whip.is_none() {
                    anyhow::bail!(
                        "Path '{}': whip source requires [paths.{}.whip] config",
                        name, name
                    );
                }
            }
            SourceType::Rtsp(url) => {
                if url.is_empty() {
                    anyhow::bail!("Path '{}': rtsp URL cannot be empty", name);
                }
                if !url.starts_with("rtsp://") && !url.starts_with("rtsps://") {
                    anyhow::bail!("Path '{}': invalid RTSP URL: {}", name, url);
                }
            }
            SourceType::File(path) => {
                if path.is_empty() {
                    anyhow::bail!("Path '{}': file path cannot be empty", name);
                }
            }
            SourceType::Publisher => {
                // Publisher 不需要额外配置
            }
        }
        
        Ok(())
    }
    
    /// 检查是否为新配置格式
    pub fn is_paths_mode(&self) -> bool {
        !self.paths.is_empty()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            http: HttpConfig {
                listen: "0.0.0.0:7778".to_string(),
            },
            log: LogConfig {
                level: "info".to_string(),
            },
            webrtc: WebRtcConfig::default(),
            path_defaults: PathConfig::default(),
            paths: HashMap::new(),
            // 旧配置默认值
            stream: Some(StreamConfig {
                id: "camera".to_string(),
                rtp_port: 5004,
            }),
            camera: Some(CameraConfig {
                device: "/dev/video0".to_string(),
                source: CaptureSource::Ffmpeg,
                command: Some(
                    "ffmpeg -f v4l2 -video_size 640x480 -framerate 30 -i /dev/video0 -pix_fmt yuv420p -c:v libx264 -preset ultrafast -tune zerolatency -profile:v baseline -g 30 -b:v 1M -f rtp rtp://127.0.0.1:5004"
                        .to_string(),
                ),
                codec: CodecConfig::default(),
                v4l2: None,
            }),
            mode: Some(Mode::default()),
            whip: None,
        }
    }
}

impl From<CodecConfig> for webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
    fn from(val: CodecConfig) -> Self {
        webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
            mime_type: val.mime_type,
            clock_rate: val.clock_rate,
            channels: val.channels,
            sdp_fmtp_line: val.sdp_fmtp_line.unwrap_or_default(),
            rtcp_feedback: vec![
                webrtc::rtp_transceiver::RTCPFeedback {
                    typ: "goog-remb".to_owned(),
                    parameter: "".to_owned(),
                },
                webrtc::rtp_transceiver::RTCPFeedback {
                    typ: "ccm".to_owned(),
                    parameter: "fir".to_owned(),
                },
                webrtc::rtp_transceiver::RTCPFeedback {
                    typ: "nack".to_owned(),
                    parameter: "".to_owned(),
                },
                webrtc::rtp_transceiver::RTCPFeedback {
                    typ: "nack".to_owned(),
                    parameter: "pli".to_owned(),
                },
            ],
        }
    }
}
