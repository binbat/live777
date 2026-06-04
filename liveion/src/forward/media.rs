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
}
