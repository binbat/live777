use webrtc::{
    rtp_transceiver::{
        rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters},
        PayloadType, RTCPFeedback,
    },
    sdp::{Error, MediaDescription, SessionDescription},
};

pub fn codecs_capability_from_media_description(
    m: &MediaDescription,
) -> Result<Vec<RTCRtpCodecCapability>, Error> {
    let codecs_parameters = codecs_from_media_description(m)?;
    Ok(codecs_parameters
        .into_iter()
        .map(|c| c.capability)
        .collect())
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
