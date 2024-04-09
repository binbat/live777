use webrtc::{
    rtp_transceiver::{
        rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType},
        PayloadType, RTCPFeedback,
    },
    sdp::{Error, MediaDescription, SessionDescription},
};

pub(crate) struct MediaInfo {
    pub(crate) _codec: Vec<RTCRtpCodecParameters>,
    pub(crate) video_transceiver: (u8, u8, bool), // (send,recv,svc)
    pub(crate) audio_transceiver: (u8, u8),       // (send,recv)
}

impl TryFrom<SessionDescription> for MediaInfo {
    type Error = anyhow::Error;

    fn try_from(value: SessionDescription) -> Result<Self, Self::Error> {
        let media_descriptions = value.media_descriptions;
        let mut codec = Vec::new();
        let mut video_transceiver = (0, 0, false);
        let mut audio_transceiver = (0, 0, false);
        for md in &media_descriptions {
            let media = md.media_name.media.clone();
            let update = match RTPCodecType::from(media.as_str()) {
                RTPCodecType::Video => &mut video_transceiver,
                RTPCodecType::Audio => &mut audio_transceiver,
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
        })
    }
}

// from https://github.com/webrtc-rs/webrtc/blob/master/webrtc/src/peer_connection/sdp/mod.rs
pub fn codecs_from_media_description(
    m: &MediaDescription,
) -> Result<Vec<RTCRtpCodecParameters>, Error> {
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
                return Err(err);
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
            capability: RTCRtpCodecCapability {
                mime_type: m.media_name.media.clone() + "/" + codec.name.as_str(),
                clock_rate: codec.clock_rate,
                channels,
                sdp_fmtp_line: codec.fmtp.clone(),
                rtcp_feedback: feedback,
            },
            payload_type,
            stats_id: String::new(),
        })
    }

    Ok(out)
}
