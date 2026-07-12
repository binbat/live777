use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
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

impl MediaInfo {
    pub fn normalize_audio_only(&mut self) {
        if self.is_audio_only() && self.audio_transport.is_none() && self.video_transport.is_some()
        {
            self.audio_transport = self.video_transport.take();
        }
    }

    pub fn is_audio_only(&self) -> bool {
        self.video_codec.is_none() && self.audio_codec.is_some()
    }

    pub fn is_video_only(&self) -> bool {
        self.video_codec.is_some() && self.audio_codec.is_none()
    }

    pub fn has_both(&self) -> bool {
        self.video_codec.is_some() && self.audio_codec.is_some()
    }

    pub fn video_transport(&self) -> Option<&TransportInfo> {
        self.video_transport.as_ref()
    }

    pub fn audio_transport(&self) -> Option<&TransportInfo> {
        self.audio_transport.as_ref()
    }
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
    AV1 {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaProfile {
    pub video: Option<CodecFingerprint>,
    pub audio: Option<CodecFingerprint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecFingerprint {
    pub kind: MediaKind,
    pub mime_type: String,
    pub clock_rate: u32,
    pub channels: Option<u16>,
    pub fmtp: Option<String>,
    pub codec_private_hash: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Audio,
}

impl MediaProfile {
    pub fn from_media_info(media_info: &MediaInfo) -> Self {
        Self {
            video: media_info
                .video_codec
                .as_ref()
                .map(CodecFingerprint::from_video_codec_params),
            audio: media_info
                .audio_codec
                .as_ref()
                .map(CodecFingerprint::from_audio_codec_params),
        }
    }

    pub fn is_replace_compatible_with(&self, next: &MediaProfile) -> bool {
        self.video == next.video && self.audio == next.audio
    }
}

impl CodecFingerprint {
    fn from_video_codec_params(params: &VideoCodecParams) -> Self {
        match params {
            VideoCodecParams::H264 {
                clock_rate,
                profile_level_id,
                sps,
                pps,
                ..
            } => Self {
                kind: MediaKind::Video,
                mime_type: normalize_mime_type("video/H264"),
                clock_rate: *clock_rate,
                channels: None,
                fmtp: h264_fmtp(profile_level_id.as_deref()),
                codec_private_hash: hash_parts([sps.as_slice(), pps.as_slice()]),
            },
            VideoCodecParams::H265 {
                clock_rate,
                vps,
                sps,
                pps,
                ..
            } => Self {
                kind: MediaKind::Video,
                mime_type: normalize_mime_type("video/H265"),
                clock_rate: *clock_rate,
                channels: None,
                fmtp: None,
                codec_private_hash: hash_parts([vps.as_slice(), sps.as_slice(), pps.as_slice()]),
            },
            VideoCodecParams::VP8 { clock_rate, .. } => Self {
                kind: MediaKind::Video,
                mime_type: normalize_mime_type("video/VP8"),
                clock_rate: *clock_rate,
                channels: None,
                fmtp: None,
                codec_private_hash: None,
            },
            VideoCodecParams::VP9 { clock_rate, .. } => Self {
                kind: MediaKind::Video,
                mime_type: normalize_mime_type("video/VP9"),
                clock_rate: *clock_rate,
                channels: None,
                fmtp: Some(normalize_fmtp("profile-id=0")),
                codec_private_hash: None,
            },
            VideoCodecParams::AV1 { clock_rate, .. } => Self {
                kind: MediaKind::Video,
                mime_type: normalize_mime_type("video/AV1"),
                clock_rate: *clock_rate,
                channels: None,
                fmtp: Some(normalize_fmtp("profile-id=0")),
                codec_private_hash: None,
            },
        }
    }

    fn from_audio_codec_params(params: &AudioCodecParams) -> Self {
        let mime_type = normalize_mime_type(&format!("audio/{}", params.codec));
        Self {
            kind: MediaKind::Audio,
            mime_type,
            clock_rate: params.clock_rate,
            channels: Some(params.channels),
            fmtp: audio_fmtp(&params.codec),
            codec_private_hash: None,
        }
    }
}

fn normalize_mime_type(mime_type: &str) -> String {
    mime_type.to_ascii_lowercase()
}

fn normalize_fmtp(fmtp: &str) -> String {
    let mut params = fmtp
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>();
    params.sort();
    params.join(";")
}

fn h264_fmtp(profile_level_id: Option<&str>) -> Option<String> {
    let mut parts = vec![
        "level-asymmetry-allowed=1".to_string(),
        "packetization-mode=1".to_string(),
    ];
    if let Some(profile_level_id) = profile_level_id
        && !profile_level_id.is_empty()
    {
        parts.push(format!(
            "profile-level-id={}",
            profile_level_id.to_ascii_lowercase()
        ));
    }
    Some(normalize_fmtp(&parts.join(";")))
}

fn audio_fmtp(codec: &str) -> Option<String> {
    codec
        .eq_ignore_ascii_case("opus")
        .then(|| normalize_fmtp("minptime=10;useinbandfec=1"))
}

fn hash_parts<const N: usize>(parts: [&[u8]; N]) -> Option<u64> {
    if parts.iter().all(|part| part.is_empty()) {
        return None;
    }

    let mut hasher = DefaultHasher::new();
    for part in parts {
        part.hash(&mut hasher);
    }
    Some(hasher.finish())
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

impl TransportInfo {
    pub fn is_tcp(&self) -> bool {
        matches!(self, TransportInfo::Tcp { .. })
    }

    pub fn is_udp(&self) -> bool {
        matches!(self, TransportInfo::Udp { .. })
    }

    pub fn tcp_channels(&self) -> Option<(u8, u8)> {
        if let TransportInfo::Tcp {
            rtp_channel,
            rtcp_channel,
        } = self
        {
            Some((*rtp_channel, *rtcp_channel))
        } else {
            None
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct CodecInfo {
    pub video_codec: Option<rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters>,
    pub audio_codec: Option<rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters>,
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
            cli::Codec::AV1 => VideoCodecParams::AV1 {
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

impl From<VideoCodecParams> for rtc::rtp_transceiver::rtp_sender::RTCRtpCodec {
    fn from(params: VideoCodecParams) -> Self {
        match params {
            VideoCodecParams::H264 { clock_rate, .. } => {
                rtc::rtp_transceiver::rtp_sender::RTCRtpCodec {
                    mime_type: "video/H264".to_string(),
                    clock_rate,
                    channels: 0,
                    sdp_fmtp_line:
                        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                            .to_string(),
                    rtcp_feedback: video_rtcp_feedback(),
                }
            }
            VideoCodecParams::H265 { clock_rate, .. } => {
                rtc::rtp_transceiver::rtp_sender::RTCRtpCodec {
                    mime_type: "video/H265".to_string(),
                    clock_rate,
                    channels: 0,
                    sdp_fmtp_line: "".to_string(),
                    rtcp_feedback: video_rtcp_feedback(),
                }
            }
            VideoCodecParams::VP8 { clock_rate, .. } => {
                rtc::rtp_transceiver::rtp_sender::RTCRtpCodec {
                    mime_type: "video/VP8".to_string(),
                    clock_rate,
                    channels: 0,
                    sdp_fmtp_line: "".to_string(),
                    rtcp_feedback: video_rtcp_feedback(),
                }
            }
            VideoCodecParams::VP9 { clock_rate, .. } => {
                rtc::rtp_transceiver::rtp_sender::RTCRtpCodec {
                    mime_type: "video/VP9".to_string(),
                    clock_rate,
                    channels: 0,
                    sdp_fmtp_line: "profile-id=0".to_string(),
                    rtcp_feedback: video_rtcp_feedback(),
                }
            }
            VideoCodecParams::AV1 { clock_rate, .. } => {
                rtc::rtp_transceiver::rtp_sender::RTCRtpCodec {
                    mime_type: "video/AV1".to_string(),
                    clock_rate,
                    channels: 0,
                    sdp_fmtp_line: "profile-id=0".to_string(),
                    rtcp_feedback: video_rtcp_feedback(),
                }
            }
        }
    }
}

fn video_rtcp_feedback() -> Vec<rtc::rtp_transceiver::rtp_sender::RTCPFeedback> {
    vec![
        rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
            typ: "goog-remb".to_string(),
            parameter: "".to_string(),
        },
        rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
            typ: "transport-cc".to_string(),
            parameter: "".to_string(),
        },
        rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
            typ: "ccm".to_string(),
            parameter: "fir".to_string(),
        },
        rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
            typ: "nack".to_string(),
            parameter: "".to_string(),
        },
        rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
            typ: "nack".to_string(),
            parameter: "pli".to_string(),
        },
    ]
}

impl From<AudioCodecParams> for rtc::rtp_transceiver::rtp_sender::RTCRtpCodec {
    fn from(params: AudioCodecParams) -> Self {
        rtc::rtp_transceiver::rtp_sender::RTCRtpCodec {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_profile_treats_same_codec_as_replace_compatible() {
        let media_info = MediaInfo {
            video_codec: Some(VideoCodecParams::VP8 {
                payload_type: 96,
                clock_rate: 90000,
            }),
            audio_codec: Some(AudioCodecParams {
                codec: "opus".to_string(),
                payload_type: 111,
                clock_rate: 48000,
                channels: 2,
            }),
            video_transport: None,
            audio_transport: None,
        };

        let profile = MediaProfile::from_media_info(&media_info);
        let next = MediaProfile::from_media_info(&media_info);

        assert!(profile.is_replace_compatible_with(&next));
    }

    #[test]
    fn media_profile_rejects_video_codec_family_change() {
        let vp8 = MediaProfile::from_media_info(&MediaInfo {
            video_codec: Some(VideoCodecParams::VP8 {
                payload_type: 96,
                clock_rate: 90000,
            }),
            audio_codec: None,
            video_transport: None,
            audio_transport: None,
        });
        let vp9 = MediaProfile::from_media_info(&MediaInfo {
            video_codec: Some(VideoCodecParams::VP9 {
                payload_type: 98,
                clock_rate: 90000,
            }),
            audio_codec: None,
            video_transport: None,
            audio_transport: None,
        });

        assert!(!vp8.is_replace_compatible_with(&vp9));
    }

    #[test]
    fn media_profile_rejects_h264_private_data_change() {
        let baseline = MediaProfile::from_media_info(&MediaInfo {
            video_codec: Some(VideoCodecParams::H264 {
                payload_type: 96,
                clock_rate: 90000,
                profile_level_id: Some("42001f".to_string()),
                sps: vec![1, 2, 3],
                pps: vec![4, 5],
            }),
            audio_codec: None,
            video_transport: None,
            audio_transport: None,
        });
        let changed_sps = MediaProfile::from_media_info(&MediaInfo {
            video_codec: Some(VideoCodecParams::H264 {
                payload_type: 96,
                clock_rate: 90000,
                profile_level_id: Some("42001f".to_string()),
                sps: vec![9, 9, 9],
                pps: vec![4, 5],
            }),
            audio_codec: None,
            video_transport: None,
            audio_transport: None,
        });

        assert!(!baseline.is_replace_compatible_with(&changed_sps));
    }

    #[test]
    fn media_profile_rejects_audio_channel_change() {
        let stereo = MediaProfile::from_media_info(&MediaInfo {
            video_codec: None,
            audio_codec: Some(AudioCodecParams {
                codec: "opus".to_string(),
                payload_type: 111,
                clock_rate: 48000,
                channels: 2,
            }),
            video_transport: None,
            audio_transport: None,
        });
        let mono = MediaProfile::from_media_info(&MediaInfo {
            video_codec: None,
            audio_codec: Some(AudioCodecParams {
                codec: "opus".to_string(),
                payload_type: 111,
                clock_rate: 48000,
                channels: 1,
            }),
            video_transport: None,
            audio_transport: None,
        });

        assert!(!stereo.is_replace_compatible_with(&mono));
    }

    #[test]
    fn test_media_info_is_audio_only() {
        let media_info = MediaInfo {
            video_codec: None,
            audio_codec: Some(AudioCodecParams {
                codec: "opus".to_string(),
                payload_type: 111,
                clock_rate: 48000,
                channels: 2,
            }),
            video_transport: None,
            audio_transport: None,
        };

        assert!(media_info.is_audio_only());
        assert!(!media_info.is_video_only());
        assert!(!media_info.has_both());
    }

    #[test]
    fn test_media_info_normalize_audio_only() {
        let mut media_info = MediaInfo {
            video_codec: None,
            audio_codec: Some(AudioCodecParams {
                codec: "opus".to_string(),
                payload_type: 111,
                clock_rate: 48000,
                channels: 2,
            }),
            video_transport: Some(TransportInfo::Udp {
                rtp_send_port: Some(5004),
                rtp_recv_port: None,
                rtcp_send_port: Some(5005),
                rtcp_recv_port: None,
                server_addr: None,
            }),
            audio_transport: None,
        };

        media_info.normalize_audio_only();

        assert!(media_info.audio_transport.is_some());
        assert!(media_info.video_transport.is_none());
    }

    #[test]
    fn test_transport_info_is_tcp() {
        let tcp_transport = TransportInfo::Tcp {
            rtp_channel: 0,
            rtcp_channel: 1,
        };

        assert!(tcp_transport.is_tcp());
        assert!(!tcp_transport.is_udp());
        assert_eq!(tcp_transport.tcp_channels(), Some((0, 1)));
    }

    #[test]
    fn test_transport_info_is_udp() {
        let udp_transport = TransportInfo::Udp {
            rtp_send_port: Some(5004),
            rtp_recv_port: None,
            rtcp_send_port: Some(5005),
            rtcp_recv_port: None,
            server_addr: None,
        };

        assert!(udp_transport.is_udp());
        assert!(!udp_transport.is_tcp());
        assert_eq!(udp_transport.tcp_channels(), None);
    }

    #[test]
    fn video_codec_params_include_transport_cc_feedback() {
        let codecs = [
            VideoCodecParams::VP8 {
                payload_type: 96,
                clock_rate: 90000,
            },
            VideoCodecParams::VP9 {
                payload_type: 96,
                clock_rate: 90000,
            },
            VideoCodecParams::H264 {
                payload_type: 96,
                clock_rate: 90000,
                profile_level_id: None,
                sps: vec![],
                pps: vec![],
            },
            VideoCodecParams::H265 {
                payload_type: 96,
                clock_rate: 90000,
                vps: vec![],
                sps: vec![],
                pps: vec![],
            },
            VideoCodecParams::AV1 {
                payload_type: 96,
                clock_rate: 90000,
            },
        ];

        for codec in codecs {
            let rtp_codec = rtc::rtp_transceiver::rtp_sender::RTCRtpCodec::from(codec);

            assert!(
                rtp_codec
                    .rtcp_feedback
                    .iter()
                    .any(|feedback| feedback.typ == "transport-cc" && feedback.parameter.is_empty()),
                "{} missing transport-cc feedback: {:?}",
                rtp_codec.mime_type,
                rtp_codec.rtcp_feedback
            );
        }
    }
}
