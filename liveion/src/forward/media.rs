use rtc::rtp_transceiver::PayloadType;
use rtc::rtp_transceiver::rtp_sender::{
    RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RtpCodecKind,
};
use sdp::{MediaDescription, SessionDescription};

#[derive(Debug, Clone)]
pub(crate) struct MediaInfo {
    pub(crate) _codec: Vec<RTCRtpCodecParameters>,
    pub(crate) video_transceiver: (u8, u8, bool), // (send,recv,svc)
    pub(crate) audio_transceiver: (u8, u8),       // (send,recv)
    pub(crate) has_data_channel: bool,
}

impl MediaInfo {
    pub(crate) fn codec_for_kind(&self, kind: RtpCodecKind) -> Option<RTCRtpCodec> {
        self._codec
            .iter()
            .find(|codec| {
                let mime = codec.rtp_codec.mime_type.to_lowercase();
                match kind {
                    RtpCodecKind::Video => mime.starts_with("video/"),
                    RtpCodecKind::Audio => mime.starts_with("audio/"),
                    RtpCodecKind::Unspecified => false,
                }
            })
            .map(|codec| codec.rtp_codec.clone())
    }

    pub(crate) fn profile(&self) -> MediaProfile {
        MediaProfile {
            video: self
                .codec_for_kind(RtpCodecKind::Video)
                .map(|codec| CodecFingerprint::from_rtp_codec(RtpCodecKind::Video, &codec)),
            audio: self
                .codec_for_kind(RtpCodecKind::Audio)
                .map(|codec| CodecFingerprint::from_rtp_codec(RtpCodecKind::Audio, &codec)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MediaProfile {
    pub(crate) video: Option<CodecFingerprint>,
    pub(crate) audio: Option<CodecFingerprint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CodecFingerprint {
    pub(crate) kind: RtpCodecKind,
    pub(crate) mime_type: String,
    pub(crate) clock_rate: u32,
    pub(crate) channels: Option<u16>,
    pub(crate) fmtp: Option<String>,
    pub(crate) codec_private_hash: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MediaGenerationDecision {
    pub(crate) generation_id: u64,
    pub(crate) changed: bool,
}

impl MediaProfile {
    pub(crate) fn is_replace_compatible_with(&self, next: &MediaProfile) -> bool {
        self.video == next.video && self.audio == next.audio
    }
}

impl CodecFingerprint {
    pub(crate) fn from_rtp_codec(kind: RtpCodecKind, codec: &RTCRtpCodec) -> Self {
        Self {
            kind,
            mime_type: codec.mime_type.to_ascii_lowercase(),
            clock_rate: codec.clock_rate,
            channels: (kind == RtpCodecKind::Audio).then_some(codec.channels),
            fmtp: normalize_fmtp(&codec.sdp_fmtp_line),
            codec_private_hash: None,
        }
    }
}

impl MediaGenerationDecision {
    pub(crate) fn decide(
        current_generation_id: u64,
        previous: Option<&MediaProfile>,
        next: &MediaProfile,
    ) -> Self {
        match previous {
            Some(previous) if previous.is_replace_compatible_with(next) => Self {
                generation_id: current_generation_id,
                changed: false,
            },
            Some(_) => Self {
                generation_id: current_generation_id.saturating_add(1),
                changed: true,
            },
            None => Self {
                generation_id: current_generation_id,
                changed: false,
            },
        }
    }
}

fn normalize_fmtp(fmtp: &str) -> Option<String> {
    let mut params = fmtp
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>();
    if params.is_empty() {
        return None;
    }
    params.sort();
    Some(params.join(";"))
}

impl TryFrom<SessionDescription> for MediaInfo {
    type Error = anyhow::Error;

    fn try_from(value: SessionDescription) -> Result<Self, Self::Error> {
        let media_descriptions = value.media_descriptions;
        let mut codec = Vec::new();
        let mut video_transceiver = (0, 0, false);
        let mut audio_transceiver = (0, 0, false);
        let mut has_data_channel = false;
        for md in &media_descriptions {
            if md.media_name.media == "application"
                && md
                    .media_name
                    .formats
                    .iter()
                    .any(|f| f == "webrtc-datachannel")
            {
                has_data_channel = true;
            }
            let media = md.media_name.media.clone();
            let update = match RtpCodecKind::from(media.as_str()) {
                RtpCodecKind::Video => &mut video_transceiver,
                RtpCodecKind::Audio => &mut audio_transceiver,
                _ => {
                    continue;
                }
            };
            codec.append(&mut codecs_from_media_description(md)?);
            for attribute in &md.attributes {
                match attribute.key.as_str() {
                    "sendonly" => {
                        update.0 += 1;
                    }
                    "recvonly" => {
                        update.1 += 1;
                    }
                    "sendrecv" => {
                        update.0 += 1;
                        update.1 += 1;
                    }
                    "simulcast" => {
                        update.2 = true;
                    }
                    _ => {}
                }
            }
        }
        Ok(Self {
            _codec: codec,
            video_transceiver,
            audio_transceiver: (audio_transceiver.0, audio_transceiver.1),
            has_data_channel,
        })
    }
}

// from https://github.com/webrtc-rs/webrtc/blob/master/webrtc/src/peer_connection/sdp/mod.rs
pub fn codecs_from_media_description(
    m: &MediaDescription,
) -> anyhow::Result<Vec<RTCRtpCodecParameters>> {
    let s = SessionDescription {
        media_descriptions: vec![m.clone()],
        ..Default::default()
    };

    let mut out = vec![];
    for payload_str in &m.media_name.formats {
        let payload_type: PayloadType = payload_str.parse::<u8>()?;
        let codec = match s.get_codec_for_payload_type(payload_type) {
            Ok(codec) => codec,
            Err(err) => {
                if payload_type == 0 {
                    continue;
                }
                return Err(err.into());
            }
        };

        let channels = codec.encoding_parameters.parse::<u16>().unwrap_or(0);

        let mut feedback = vec![];
        for raw in &codec.rtcp_feedback {
            let split: Vec<&str> = raw.split(' ').collect();

            let entry = if split.len() == 2 {
                RTCPFeedback {
                    typ: split[0].to_string(),
                    parameter: split[1].to_string(),
                }
            } else {
                RTCPFeedback {
                    typ: split[0].to_string(),
                    parameter: String::new(),
                }
            };

            feedback.push(entry);
        }

        out.push(RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: m.media_name.media.clone() + "/" + codec.name.as_str(),
                clock_rate: codec.clock_rate,
                channels,
                sdp_fmtp_line: codec.fmtp.clone(),
                rtcp_feedback: feedback,
            },
            payload_type,
        })
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_for_kind_returns_g722_from_publish_media_info() {
        let media_info = MediaInfo {
            _codec: vec![RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: "audio/G722".to_string(),
                    clock_rate: 8000,
                    channels: 0,
                    sdp_fmtp_line: "".to_string(),
                    rtcp_feedback: vec![],
                },
                payload_type: 9,
            }],
            video_transceiver: (0, 0, false),
            audio_transceiver: (1, 0),
            has_data_channel: false,
        };

        let codec = media_info
            .codec_for_kind(RtpCodecKind::Audio)
            .expect("G722 codec should be available from publish media info");

        assert_eq!(codec.mime_type, "audio/G722");
        assert_eq!(codec.clock_rate, 8000);
    }

    #[test]
    fn media_profile_rejects_fmtp_change_for_same_video_codec() {
        let old = MediaProfile {
            video: Some(CodecFingerprint::from_rtp_codec(
                RtpCodecKind::Video,
                &RTCRtpCodec {
                    mime_type: "video/H264".to_string(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: "packetization-mode=1;profile-level-id=42001f".to_string(),
                    rtcp_feedback: vec![],
                },
            )),
            audio: None,
        };
        let new = MediaProfile {
            video: Some(CodecFingerprint::from_rtp_codec(
                RtpCodecKind::Video,
                &RTCRtpCodec {
                    mime_type: "video/H264".to_string(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: "packetization-mode=1;profile-level-id=640032".to_string(),
                    rtcp_feedback: vec![],
                },
            )),
            audio: None,
        };

        assert!(!old.is_replace_compatible_with(&new));
    }

    #[test]
    fn next_generation_reuses_compatible_profile_and_increments_incompatible_profile() {
        let vp8 = MediaProfile {
            video: Some(CodecFingerprint::from_rtp_codec(
                RtpCodecKind::Video,
                &RTCRtpCodec {
                    mime_type: "video/VP8".to_string(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: "".to_string(),
                    rtcp_feedback: vec![],
                },
            )),
            audio: None,
        };
        let vp9 = MediaProfile {
            video: Some(CodecFingerprint::from_rtp_codec(
                RtpCodecKind::Video,
                &RTCRtpCodec {
                    mime_type: "video/VP9".to_string(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: "profile-id=0".to_string(),
                    rtcp_feedback: vec![],
                },
            )),
            audio: None,
        };

        assert_eq!(
            MediaGenerationDecision::decide(3, Some(&vp8), &vp8).generation_id,
            3
        );
        assert_eq!(
            MediaGenerationDecision::decide(3, Some(&vp8), &vp9).generation_id,
            4
        );
        assert!(MediaGenerationDecision::decide(3, Some(&vp8), &vp9).changed);
    }
}
