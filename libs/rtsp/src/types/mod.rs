use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMode {
    Push,
    Pull,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_codec: Option<VideoCodecParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codec: Option<AudioCodecParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_transport: Option<TransportInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_transport: Option<TransportInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VideoCodecParams {
    H264 {
        payload_type: u8,
        clock_rate: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        profile_level_id: Option<String>,
        sps: Vec<u8>,
        pps: Vec<u8>,
    },
    H265 {
        payload_type: u8,
        clock_rate: u32,
        vps: Vec<u8>,
        sps: Vec<u8>,
        pps: Vec<u8>,
    },
    VP8 {
        payload_type: u8,
        clock_rate: u32,
    },
    VP9 {
        payload_type: u8,
        clock_rate: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioCodecParams {
    pub codec: String,
    pub payload_type: u8,
    pub clock_rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransportInfo {
    Udp {
        rtp_send_port: Option<u16>,
        rtp_recv_port: Option<u16>,
        rtcp_send_port: Option<u16>,
        rtcp_recv_port: Option<u16>,
        server_addr: Option<SocketAddr>,
    },
    Tcp {
        rtp_channel: u8,
        rtcp_channel: u8,
    },
}

#[derive(Debug, Default, Clone)]
pub struct CodecInfo {
    pub video_codec: Option<webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters>,
    pub audio_codec: Option<webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters>,
}

impl CodecInfo {
    pub fn new() -> Self {
        Self::default()
    }
}

impl From<cli::Codec> for VideoCodecParams {
    fn from(codec: cli::Codec) -> Self {
        match codec {
            cli::Codec::H264 => VideoCodecParams::H264 {
                payload_type: 96,
                clock_rate: 90000,
                profile_level_id: Some("42001f".to_string()),
                sps: Vec::new(),
                pps: Vec::new(),
            },
            cli::Codec::H265 => VideoCodecParams::H265 {
                payload_type: 96,
                clock_rate: 90000,
                vps: Vec::new(),
                sps: Vec::new(),
                pps: Vec::new(),
            },
            cli::Codec::Vp8 => VideoCodecParams::VP8 {
                payload_type: 96,
                clock_rate: 90000,
            },
            cli::Codec::Vp9 => VideoCodecParams::VP9 {
                payload_type: 96,
                clock_rate: 90000,
            },
            _ => VideoCodecParams::H264 {
                payload_type: 96,
                clock_rate: 90000,
                profile_level_id: None,
                sps: Vec::new(),
                pps: Vec::new(),
            },
        }
    }
}

impl From<cli::Codec> for AudioCodecParams {
    fn from(codec: cli::Codec) -> Self {
        match codec {
            cli::Codec::Opus => AudioCodecParams {
                codec: "opus".to_string(),
                payload_type: 111,
                clock_rate: 48000,
                channels: 2,
            },
            cli::Codec::PCMA => AudioCodecParams {
                codec: "PCMA".to_string(),
                payload_type: 8,
                clock_rate: 8000,
                channels: 1,
            },
            cli::Codec::PCMU => AudioCodecParams {
                codec: "PCMU".to_string(),
                payload_type: 0,
                clock_rate: 8000,
                channels: 1,
            },
            cli::Codec::G722 => AudioCodecParams {
                codec: "G722".to_string(),
                payload_type: 9,
                clock_rate: 8000,
                channels: 1,
            },
            _ => AudioCodecParams {
                codec: "opus".to_string(),
                payload_type: 111,
                clock_rate: 48000,
                channels: 2,
            },
        }
    }
}

impl From<VideoCodecParams> for webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
    fn from(params: VideoCodecParams) -> Self {
        match params {
            VideoCodecParams::H264 { clock_rate, .. } => {
                webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                    mime_type: "video/H264".to_string(),
                    clock_rate,
                    channels: 0,
                    sdp_fmtp_line:
                        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                            .to_string(),
                    rtcp_feedback: vec![],
                }
            }
            VideoCodecParams::H265 { clock_rate, .. } => {
                webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                    mime_type: "video/H265".to_string(),
                    clock_rate,
                    channels: 0,
                    sdp_fmtp_line: "".to_string(),
                    rtcp_feedback: vec![],
                }
            }
            VideoCodecParams::VP8 { clock_rate, .. } => {
                webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                    mime_type: "video/VP8".to_string(),
                    clock_rate,
                    channels: 0,
                    sdp_fmtp_line: "".to_string(),
                    rtcp_feedback: vec![],
                }
            }
            VideoCodecParams::VP9 { clock_rate, .. } => {
                webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
                    mime_type: "video/VP9".to_string(),
                    clock_rate,
                    channels: 0,
                    sdp_fmtp_line: "profile-id=0".to_string(),
                    rtcp_feedback: vec![],
                }
            }
        }
    }
}

impl From<AudioCodecParams> for webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
    fn from(params: AudioCodecParams) -> Self {
        webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability {
            mime_type: format!("audio/{}", params.codec),
            clock_rate: params.clock_rate,
            channels: params.channels,
            sdp_fmtp_line: if params.codec == "opus" {
                "minptime=10;useinbandfec=1".to_string()
            } else {
                "".to_string()
            },
            rtcp_feedback: vec![],
        }
    }
}
