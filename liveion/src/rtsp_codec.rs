use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters};

/// Convert an audio codec announcement into a WebRTC codec description.
///
/// Shared by [`crate::stream::source::sdp_source`] and
/// [`crate::stream::source::rtsp_source`].
pub(crate) fn audio_codec_to_rtc(codec: &rtsp::AudioCodecParams) -> RTCRtpCodecParameters {
    let mime_type = format!("audio/{}", codec.codec.to_uppercase());

    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type,
            clock_rate: codec.clock_rate,
            channels: codec.channels,
            sdp_fmtp_line: if codec.codec.to_lowercase() == "opus" {
                "minptime=10;useinbandfec=1".to_string()
            } else {
                String::new()
            },
            rtcp_feedback: vec![],
        },
        payload_type: codec.payload_type,
    }
}

/// Convert an RTSP video codec announcement into a WebRTC codec description.
///
/// Shared between [`crate::rtsp_server`] (server-side SDP construction) and
/// [`crate::stream::source::rtsp_source`] (client-side RTSP pull).  Both
/// callers need the same mapping, so it lives here rather than in either
/// subsystem.
pub(crate) fn video_codec_to_rtc(codec: &rtsp::VideoCodecParams) -> RTCRtpCodecParameters {
    use rtsp::VideoCodecParams;

    let (mime, pt, clock_rate, fmtp) = match codec {
        VideoCodecParams::H264 {
            payload_type,
            clock_rate,
            profile_level_id,
            packetization_mode,
            sps,
            pps,
        } => {
            let profile = profile_level_id.as_deref().unwrap_or("42001f");
            let mode = packetization_mode.unwrap_or(1);
            let mut fmtp = format!(
                "level-asymmetry-allowed=1;packetization-mode={};profile-level-id={}",
                mode, profile
            );
            if !sps.is_empty() && !pps.is_empty() {
                fmtp.push_str(&format!(
                    ";sprop-parameter-sets={},{}",
                    BASE64.encode(sps),
                    BASE64.encode(pps)
                ));
            }
            ("video/H264", *payload_type, *clock_rate, fmtp)
        }
        VideoCodecParams::H265 {
            payload_type,
            clock_rate,
            vps,
            sps,
            pps,
            ..
        } => {
            let mut parts = Vec::new();
            if !vps.is_empty() {
                parts.push(format!("sprop-vps={}", BASE64.encode(vps)));
            }
            if !sps.is_empty() {
                parts.push(format!("sprop-sps={}", BASE64.encode(sps)));
            }
            if !pps.is_empty() {
                parts.push(format!("sprop-pps={}", BASE64.encode(pps)));
            }
            let fmtp = if parts.is_empty() {
                String::new()
            } else {
                parts.join(";")
            };
            ("video/H265", *payload_type, *clock_rate, fmtp)
        }
        VideoCodecParams::VP8 {
            payload_type,
            clock_rate,
        } => ("video/VP8", *payload_type, *clock_rate, String::new()),
        VideoCodecParams::VP9 {
            payload_type,
            clock_rate,
        } => (
            "video/VP9",
            *payload_type,
            *clock_rate,
            "profile-id=0".to_string(),
        ),
        VideoCodecParams::AV1 {
            payload_type,
            clock_rate,
            profile_id,
        } => (
            "video/AV1",
            *payload_type,
            *clock_rate,
            format!("profile-id={}", profile_id.as_deref().unwrap_or("0")),
        ),
    };

    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: mime.to_string(),
            clock_rate,
            channels: 0,
            sdp_fmtp_line: fmtp,
            rtcp_feedback: rtsp::video_rtcp_feedback(),
        },
        payload_type: pt,
    }
}
