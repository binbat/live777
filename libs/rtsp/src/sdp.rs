use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use sdp::SessionDescription;
use sdp::description::common::Attribute;
use std::io::Cursor;
use tracing::{debug, warn};
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;

use crate::constants::media_type;
use crate::types::{AudioCodecParams, MediaInfo, VideoCodecParams};

pub fn parse_media_info_from_sdp(sdp_bytes: &[u8]) -> Result<MediaInfo> {
    let sdp =
        sdp_types::Session::parse(sdp_bytes).map_err(|e| anyhow!("Failed to parse SDP: {}", e))?;

    let (video_codec, audio_codec) = parse_codecs_from_sdp(&sdp)?;

    Ok(MediaInfo {
        video_codec,
        audio_codec,
        video_transport: None,
        audio_transport: None,
    })
}

pub fn parse_codecs_from_sdp(
    sdp: &sdp_types::Session,
) -> Result<(Option<VideoCodecParams>, Option<AudioCodecParams>)> {
    let mut video_codec = None;
    let mut audio_codec = None;

    for media in &sdp.medias {
        if media.media == media_type::VIDEO {
            video_codec = parse_video_codec(media);
        } else if media.media == media_type::AUDIO {
            audio_codec = parse_audio_codec(media);
        }
    }

    Ok((video_codec, audio_codec))
}

fn parse_video_codec(media: &sdp_types::Media) -> Option<VideoCodecParams> {
    let rtpmap = media.attributes.iter().find(|a| a.attribute == "rtpmap")?;

    let value = rtpmap.value.as_ref()?;
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let payload_type = parts[0].parse().ok()?;
    let codec_parts: Vec<&str> = parts[1].split('/').collect();
    if codec_parts.is_empty() {
        return None;
    }

    let codec_name = codec_parts[0].to_uppercase();
    let clock_rate = codec_parts
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(90000);

    match codec_name.as_str() {
        "H264" => {
            let (sps, pps) = extract_h264_params(media).unwrap_or_else(|| (Vec::new(), Vec::new()));
            let profile_level_id = extract_profile_level_id(media);

            Some(VideoCodecParams::H264 {
                payload_type,
                clock_rate,
                profile_level_id,
                sps,
                pps,
            })
        }
        "H265" | "HEVC" => {
            let (vps, sps, pps) =
                extract_h265_params(media).unwrap_or_else(|| (Vec::new(), Vec::new(), Vec::new()));

            Some(VideoCodecParams::H265 {
                payload_type,
                clock_rate,
                vps,
                sps,
                pps,
            })
        }
        "VP8" => Some(VideoCodecParams::VP8 {
            payload_type,
            clock_rate,
        }),
        "VP9" => Some(VideoCodecParams::VP9 {
            payload_type,
            clock_rate,
        }),
        _ => {
            warn!("Unsupported video codec: {}", codec_name);
            None
        }
    }
}

fn parse_audio_codec(media: &sdp_types::Media) -> Option<AudioCodecParams> {
    let rtpmap = media.attributes.iter().find(|a| a.attribute == "rtpmap")?;

    let value = rtpmap.value.as_ref()?;
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let payload_type = parts[0].parse().ok()?;
    let codec_parts: Vec<&str> = parts[1].split('/').collect();
    if codec_parts.len() < 2 {
        return None;
    }

    Some(AudioCodecParams {
        codec: codec_parts[0].to_string(),
        payload_type,
        clock_rate: codec_parts[1].parse().ok()?,
        channels: codec_parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(2),
    })
}

pub fn extract_h264_params(media: &sdp_types::Media) -> Option<(Vec<u8>, Vec<u8>)> {
    let fmtp = media
        .attributes
        .iter()
        .find(|a| a.attribute == "fmtp")
        .and_then(|a| a.value.as_ref());

    if let Some(fmtp_value) = fmtp {
        for param in fmtp_value.split(';') {
            let param = param.trim();

            if let Some(sprop) = param.strip_prefix("sprop-parameter-sets=") {
                let parts: Vec<&str> = sprop.split(',').collect();
                if parts.len() >= 2 {
                    let sps = BASE64.decode(parts[0]).unwrap_or_default();
                    let pps = BASE64.decode(parts[1]).unwrap_or_default();

                    if !sps.is_empty() && !pps.is_empty() {
                        debug!(
                            "Extracted H.264 params: SPS={} bytes, PPS={} bytes",
                            sps.len(),
                            pps.len()
                        );
                        return Some((sps, pps));
                    }
                }
            }
        }
    }

    debug!("No H.264 params found in SDP");
    None
}

pub fn extract_h265_params(media: &sdp_types::Media) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let fmtp_line = media
        .attributes
        .iter()
        .find(|a| a.attribute == "fmtp")?
        .value
        .as_ref()?;

    debug!("Raw H.265 fmtp line: {}", fmtp_line);

    let mut vps = Vec::new();
    let mut sps = Vec::new();
    let mut pps = Vec::new();

    for part in fmtp_line.split(';') {
        let part = part.trim();

        if part.is_empty() {
            continue;
        }

        debug!("Processing H.265 fmtp part: '{}'", part);

        let part = if part.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            part.split_whitespace()
                .skip(1)
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            part.to_string()
        };

        if part.is_empty() {
            continue;
        }

        if let Some(b64) = part.strip_prefix("sprop-vps=") {
            let b64_clean = b64.trim();
            debug!("Found VPS Base64: '{}'", b64_clean);
            match BASE64.decode(b64_clean) {
                Ok(decoded) => {
                    vps = decoded;
                    debug!("Decoded VPS: {} bytes", vps.len());
                }
                Err(e) => {
                    warn!(
                        "Failed to decode VPS Base64: '{}' - error: {}",
                        b64_clean, e
                    );
                }
            }
        } else if let Some(b64) = part.strip_prefix("sprop-sps=") {
            let b64_clean = b64.trim();
            debug!("Found SPS Base64: '{}'", b64_clean);
            match BASE64.decode(b64_clean) {
                Ok(decoded) => {
                    sps = decoded;
                    debug!("Decoded SPS: {} bytes", sps.len());
                }
                Err(e) => {
                    warn!(
                        "Failed to decode SPS Base64: '{}' - error: {}",
                        b64_clean, e
                    );
                }
            }
        } else if let Some(b64) = part.strip_prefix("sprop-pps=") {
            let b64_clean = b64.trim();
            debug!("Found PPS Base64: '{}'", b64_clean);
            match BASE64.decode(b64_clean) {
                Ok(decoded) => {
                    pps = decoded;
                    debug!("Decoded PPS: {} bytes", pps.len());
                }
                Err(e) => {
                    warn!(
                        "Failed to decode PPS Base64: '{}' - error: {}",
                        b64_clean, e
                    );
                }
            }
        }
    }
    if !vps.is_empty() || !sps.is_empty() || !pps.is_empty() {
        debug!(
            "Extracted H.265 params: VPS={} bytes, SPS={} bytes, PPS={} bytes",
            vps.len(),
            sps.len(),
            pps.len()
        );
        Some((vps, sps, pps))
    } else {
        debug!("No H.265 params found in SDP");
        None
    }
}

fn extract_profile_level_id(media: &sdp_types::Media) -> Option<String> {
    media
        .attributes
        .iter()
        .find(|a| a.attribute == "fmtp")
        .and_then(|a| a.value.as_ref())
        .and_then(|value| {
            for param in value.split(';') {
                if let Some(profile) = param.trim().strip_prefix("profile-level-id=") {
                    return Some(profile.to_string());
                }
            }
            None
        })
}

pub fn filter_sdp(
    webrtc_sdp: &str,
    video_codec: Option<&RTCRtpCodecParameters>,
    audio_codec: Option<&RTCRtpCodecParameters>,
) -> Result<String> {
    let mut reader = Cursor::new(webrtc_sdp.as_bytes());
    let mut session = SessionDescription::unmarshal(&mut reader)
        .map_err(|e| anyhow!("Failed to parse SDP: {:?}", e))?;

    session.media_descriptions.retain_mut(|media| {
        if media.media_name.media == media_type::VIDEO {
            if video_codec.is_none() {
                return false;
            } else if let Some(video_codec) = video_codec {
                let pt = video_codec.payload_type.to_string();

                media.media_name.formats.retain(|fmt| fmt == &pt);

                media.attributes.retain(|attr| match attr.key.as_str() {
                    "rtpmap" => attr
                        .value
                        .as_ref()
                        .map(|v| v.starts_with(&pt))
                        .unwrap_or(false),
                    "fmtp" => attr
                        .value
                        .as_ref()
                        .map(|v| v.starts_with(&pt))
                        .unwrap_or(false),
                    "rtcp-fb" => attr
                        .value
                        .as_ref()
                        .map(|v| v.starts_with(&pt))
                        .unwrap_or(false),

                    "sendrecv" | "recvonly" | "sendonly" | "inactive" => true,
                    _ => false,
                });

                media.media_name.protos = vec!["RTP".to_string(), "AVP".to_string()];

                media.attributes.push(Attribute {
                    key: "control".to_string(),
                    value: Some("streamid=0".to_string()),
                });
            }
        } else if media.media_name.media == media_type::AUDIO {
            if audio_codec.is_none() {
                return false;
            } else if let Some(audio_codec) = audio_codec {
                let pt = audio_codec.payload_type.to_string();

                media.media_name.formats.retain(|fmt| fmt == &pt);

                media.attributes.retain(|attr| match attr.key.as_str() {
                    "rtpmap" => attr
                        .value
                        .as_ref()
                        .map(|v| v.starts_with(&pt))
                        .unwrap_or(false),
                    "fmtp" => attr
                        .value
                        .as_ref()
                        .map(|v| v.starts_with(&pt))
                        .unwrap_or(false),
                    "rtcp-fb" => attr
                        .value
                        .as_ref()
                        .map(|v| v.starts_with(&pt))
                        .unwrap_or(false),
                    "sendrecv" | "recvonly" | "sendonly" | "inactive" => true,
                    _ => false,
                });

                media.media_name.protos = vec!["RTP".to_string(), "AVP".to_string()];

                media.attributes.push(Attribute {
                    key: "control".to_string(),
                    value: Some("streamid=1".to_string()),
                });
            }
        }

        true
    });

    session.attributes.retain(|attr| {
        !attr.key.starts_with("group")
            && !attr.key.starts_with("fingerprint")
            && !attr.key.starts_with("end-of-candidates")
            && !attr.key.starts_with("setup")
            && !attr.key.starts_with("mid")
            && !attr.key.starts_with("ice-ufrag")
            && !attr.key.starts_with("ice-pwd")
            && !attr.key.starts_with("extmap")
            && !attr.key.starts_with("extmap-allow-mixed")
    });

    let filtered = session.marshal();
    tracing::info!("Filtered SDP for RTSP:\n{}", filtered);
    Ok(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_h264_params() {
        let sdp = r#"v=0
o=- 0 0 IN IP4 127.0.0.1
s=Test
c=IN IP4 127.0.0.1
t=0 0
m=video 5004 RTP/AVP 96
a=rtpmap:96 H264/90000
a=fmtp:96 profile-level-id=42001f;sprop-parameter-sets=Z0IAH5WoFAFuQA==,aM4yyA==
"#;

        let session = sdp_types::Session::parse(sdp.as_bytes()).unwrap();
        let media = &session.medias[0];

        let params = extract_h264_params(media);
        assert!(params.is_some());
        if let Some((sps, pps)) = params {
            assert!(!sps.is_empty());
            assert!(!pps.is_empty());
        }
    }

    #[test]
    fn test_parse_video_codec() {
        let sdp = r#"v=0
o=- 0 0 IN IP4 127.0.0.1
s=Test
c=IN IP4 127.0.0.1
t=0 0
m=video 5004 RTP/AVP 96
a=rtpmap:96 VP8/90000
"#;

        let session = sdp_types::Session::parse(sdp.as_bytes()).unwrap();
        let codec = parse_video_codec(&session.medias[0]);

        assert!(codec.is_some());
        if let Some(VideoCodecParams::VP8 {
            payload_type,
            clock_rate,
        }) = codec
        {
            assert_eq!(payload_type, 96);
            assert_eq!(clock_rate, 90000);
        } else {
            panic!("Expected VP8 codec");
        }
    }
}
