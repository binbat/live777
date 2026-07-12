use anyhow::{Result, anyhow};
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::media_engine::*;
use rtc::rtp::packet::Packet;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use rtc_shared::marshal::Unmarshal;
use sdp::SessionDescription;
use std::io::Cursor;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::time::{Duration, Instant};
use tracing::{debug, error, info, trace, warn};
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
use webrtc::peer_connection::PeerConnection;
use webrtc::rtp_transceiver::RtpSender;

use crate::payload::{Forward, RePayload, RePayloadCodec};

const SENDER_PT_REFRESH_INTERVAL: Duration = Duration::from_millis(100);

/// Parse a semicolon-separated fmtp string into key/value pairs.
fn parse_fmtp(fmtp: &str) -> Vec<(&str, &str)> {
    fmtp.split(';')
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            let mut iter = part.splitn(2, '=');
            let key = iter.next()?;
            let value = iter.next().unwrap_or_default();
            Some((key.trim(), value.trim()))
        })
        .collect()
}

/// Return the channel count only if it is non-zero.
fn nonzero_channels(channels: u16) -> Option<u32> {
    if channels == 0 {
        None
    } else {
        Some(channels as u32)
    }
}

/// Check whether `offered_fmtp` satisfies all parameters in `required_fmtp`.
///
/// In SDP negotiation the remote answer may include additional parameters
/// beyond what our local codec declared (e.g. `level-idx` or `tier` for AV1),
/// so extra keys in `offered_fmtp` are ignored. Any parameter that appears in
/// `required_fmtp` must be present in `offered_fmtp` with the exact same value.
fn fmtp_satisfies(required_fmtp: &str, offered_fmtp: &str) -> bool {
    if required_fmtp.is_empty() {
        return true;
    }
    let offered = parse_fmtp(offered_fmtp);
    parse_fmtp(required_fmtp)
        .iter()
        .all(|(key, value)| offered.iter().any(|(k, v)| k == key && v == value))
}

/// Resolve the negotiated payload type for a sender's track codec.
///
/// The `webrtc` stack may reassign the track's payload type during SDP
/// negotiation. `write_rtp` now rejects packets whose payload type is not in
/// the sender's negotiated codec list, so callers must rewrite the packet PT
/// to the negotiated value before writing.
///
/// We first try to read the PT directly from the remote SDP answer, because
/// that is the PT the receiver expects. If the answer is not yet available or
/// parsing fails, we fall back to inspecting the sender's parameters.
async fn sender_payload_type(
    peer: &Arc<dyn PeerConnection>,
    sender: &Arc<dyn RtpSender>,
    codec: &RTCRtpCodec,
) -> Option<u8> {
    if let Some(pt) = answer_payload_type(peer, codec).await {
        return Some(pt);
    }

    // Fallback: use the sender's own view of the negotiated codecs.
    let params = sender.get_parameters().await.ok()?;
    params
        .rtp_parameters
        .codecs
        .iter()
        .find(|candidate| {
            candidate
                .rtp_codec
                .mime_type
                .eq_ignore_ascii_case(&codec.mime_type)
                && candidate.rtp_codec.clock_rate == codec.clock_rate
                && (candidate.rtp_codec.channels == 0
                    || candidate.rtp_codec.channels == codec.channels)
        })
        .map(|candidate| candidate.payload_type)
}

/// Look up the payload type assigned to our codec in the remote SDP answer.
///
/// The answer tells the sender which PT to use for each m-line. For the
/// simple WHIP/WHEP topologies used here there is at most one audio and one
/// video m-line, so matching by media kind and codec name/clock-rate is
/// sufficient.
async fn answer_payload_type(peer: &Arc<dyn PeerConnection>, codec: &RTCRtpCodec) -> Option<u8> {
    let remote = peer.remote_description().await?;
    let sdp = SessionDescription::unmarshal(&mut Cursor::new(remote.sdp.as_bytes())).ok()?;

    let expected_kind = codec.mime_type.split('/').next()?.to_lowercase();
    let expected_name = codec.mime_type.split('/').nth(1)?.to_lowercase();

    for media in &sdp.media_descriptions {
        let media_kind = media.media_name.media.to_lowercase();
        if media_kind != expected_kind {
            continue;
        }

        // The answer lists PTs in order of preference; use the first one that
        // matches our codec name, clock rate and fmtp parameters.
        for pt_str in &media.media_name.formats {
            let Ok(pt) = pt_str.parse::<u8>() else {
                continue;
            };
            let rtpmap_prefix = format!("{pt} ");
            let Some(rtpmap) = media.attributes.iter().find(|attr| {
                attr.key == "rtpmap"
                    && attr
                        .value
                        .as_ref()
                        .is_some_and(|v| v.starts_with(&rtpmap_prefix))
            }) else {
                continue;
            };
            let Some(value) = rtpmap.value.as_ref() else {
                continue;
            };
            let mut parts = value.split_whitespace();
            let _pt = parts.next()?;
            let codec_spec = parts.next()?;
            let mut spec_parts = codec_spec.split('/');
            let name = spec_parts.next()?.to_lowercase();
            let clock_rate = spec_parts
                .next()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0);
            // rtpmap may include an optional channel count for audio codecs,
            // e.g. "opus/48000/2". Compare it when both sides declare one.
            let channels = spec_parts.next().and_then(|s| s.parse::<u32>().ok());

            if name != expected_name || clock_rate != codec.clock_rate {
                continue;
            }
            if let (Some(expected), Some(offered)) = (nonzero_channels(codec.channels), channels)
                && expected != offered
            {
                continue;
            }

            // Match fmtp parameters when the source codec specifies them.
            if !codec.sdp_fmtp_line.is_empty() {
                let fmtp_prefix = format!("{pt} ");
                let fmtp = media.attributes.iter().find(|attr| {
                    attr.key == "fmtp"
                        && attr
                            .value
                            .as_ref()
                            .is_some_and(|v| v.starts_with(&fmtp_prefix))
                });
                let candidate_fmtp = fmtp
                    .and_then(|attr| attr.value.as_ref())
                    .map(|v| v.strip_prefix(&fmtp_prefix).unwrap_or(v))
                    .unwrap_or_default();
                if !fmtp_satisfies(&codec.sdp_fmtp_line, candidate_fmtp) {
                    continue;
                }
            }

            return Some(pt);
        }
    }

    None
}

/// Refresh the payload type from the negotiated answer if the cache has expired.
async fn refresh_payload_type(
    peer: &Arc<dyn PeerConnection>,
    sender: &Arc<dyn RtpSender>,
    codec: &RTCRtpCodec,
    current_pt: &mut u8,
    refreshed_at: &mut Instant,
) {
    if refreshed_at.elapsed() >= SENDER_PT_REFRESH_INTERVAL {
        if let Some(pt) = sender_payload_type(peer, sender, codec).await {
            *current_pt = pt;
        }
        *refreshed_at = Instant::now();
    }
}

struct RtpPacketLog {
    payload_type: u8,
    sequence_number: u16,
    timestamp: u32,
    ssrc: u32,
    payload_len: usize,
}

impl From<&Packet> for RtpPacketLog {
    fn from(packet: &Packet) -> Self {
        Self {
            payload_type: packet.header.payload_type,
            sequence_number: packet.header.sequence_number,
            timestamp: packet.header.timestamp,
            ssrc: packet.header.ssrc,
            payload_len: packet.payload.len(),
        }
    }
}

fn log_write_rtp_error(kind: &str, packet: &RtpPacketLog, error: &dyn std::fmt::Display) {
    let error = error.to_string();
    let error_kind = classify_write_rtp_error(&error);

    let message = format!(
        "Failed to write {kind} RTP: error_kind={error_kind}, payload_type={}, sequence_number={}, timestamp={}, ssrc={}, payload_len={}",
        packet.payload_type,
        packet.sequence_number,
        packet.timestamp,
        packet.ssrc,
        packet.payload_len,
    );

    if is_fatal_write_rtp_error_kind(error_kind) {
        error!("{message}");
    } else {
        debug!("{message}");
    }
}

fn classify_write_rtp_error(error: &str) -> &'static str {
    if error.contains("Disconnected") {
        "disconnected"
    } else if error.contains("Full") {
        "channel_full"
    } else if error.contains("track is not binding yet") {
        "track_not_bound"
    } else if error.contains("DTLS transport has not started yet") {
        "dtls_not_started"
    } else {
        "write_failed"
    }
}

fn is_fatal_write_rtp_error_kind(error_kind: &str) -> bool {
    !matches!(error_kind, "disconnected" | "dtls_not_started")
}

pub async fn setup_video_track(
    peer: Arc<dyn PeerConnection>,
    video_codec_params: &rtsp::VideoCodecParams,
    input_id: String,
) -> Result<Option<UnboundedSender<Vec<u8>>>> {
    let video_codec: RTCRtpCodec = video_codec_params.clone().into();
    let initial_video_payload_type = match &video_codec_params {
        rtsp::VideoCodecParams::H264 { payload_type, .. }
        | rtsp::VideoCodecParams::H265 { payload_type, .. }
        | rtsp::VideoCodecParams::VP8 { payload_type, .. }
        | rtsp::VideoCodecParams::VP9 { payload_type, .. }
        | rtsp::VideoCodecParams::AV1 { payload_type, .. } => *payload_type,
    };
    let video_track_id = format!("{}-video", input_id);
    let video_ssrc = rand::random::<u32>();
    let media_track = MediaStreamTrack::new(
        input_id.clone(),
        video_track_id.clone(),
        video_track_id.clone(),
        RtpCodecKind::Video,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(video_ssrc),
                ..Default::default()
            },
            codec: video_codec.clone(),
            ..Default::default()
        }],
    );
    let video_track = Arc::new(TrackLocalStaticRTP::new(media_track));

    let video_sender = peer
        .add_track(video_track.clone())
        .await
        .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    let (video_tx, mut video_rx) = unbounded_channel::<Vec<u8>>();
    let video_codec_params = video_codec_params.clone();
    let peer = peer.clone();

    tokio::spawn(async move {
        debug!("Video codec: {}", video_codec.mime_type);
        let mut first_write = true;
        let mut payload_type = initial_video_payload_type;
        let mut payload_type_refreshed_at = Instant::now() - SENDER_PT_REFRESH_INTERVAL;

        let mut handler: Box<dyn RePayload + Send> = match video_codec.mime_type.as_str() {
            MIME_TYPE_VP8 | MIME_TYPE_VP9 | MIME_TYPE_AV1 => {
                Box::new(RePayloadCodec::new(video_codec.mime_type.clone()))
            }
            MIME_TYPE_H264 => {
                let mut repayloader = RePayloadCodec::new(video_codec.mime_type.clone());
                if let rtsp::VideoCodecParams::H264 { sps, pps, .. } = &video_codec_params {
                    debug!(
                        "Setting H.264 params - SPS: {} bytes, PPS: {} bytes",
                        sps.len(),
                        pps.len()
                    );
                    repayloader.set_h264_params(sps.clone(), pps.clone());
                } else {
                    warn!("Video codec params mismatch: expected H264");
                }
                Box::new(repayloader)
            }
            MIME_TYPE_HEVC => {
                let mut repayloader = RePayloadCodec::new(video_codec.mime_type.clone());
                if let rtsp::VideoCodecParams::H265 { vps, sps, pps, .. } = &video_codec_params {
                    debug!(
                        "Setting H.265 params - VPS: {} bytes, SPS: {} bytes, PPS: {} bytes",
                        vps.len(),
                        sps.len(),
                        pps.len()
                    );
                    repayloader.set_h265_params(vps.clone(), sps.clone(), pps.clone());
                } else {
                    warn!("Video codec params mismatch: expected H265");
                }
                Box::new(repayloader)
            }
            _ => Box::new(Forward::new()),
        };

        while let Some(data) = video_rx.recv().await {
            if let Ok(packet) = Packet::unmarshal(&mut data.as_slice()) {
                trace!(
                    "Received video packet: seq={}, ts={}, marker={}",
                    packet.header.sequence_number, packet.header.timestamp, packet.header.marker
                );

                for mut packet in handler.payload(packet) {
                    trace!(
                        "Sending video packet: seq={}, ts={}, marker={}",
                        packet.header.sequence_number,
                        packet.header.timestamp,
                        packet.header.marker
                    );

                    packet.header.ssrc = video_ssrc;
                    refresh_payload_type(
                        &peer,
                        &video_sender,
                        &video_codec,
                        &mut payload_type,
                        &mut payload_type_refreshed_at,
                    )
                    .await;
                    packet.header.payload_type = payload_type;

                    let packet_log = RtpPacketLog::from(&packet);
                    if let Err(e) = video_track.write_rtp(packet).await {
                        log_write_rtp_error("video", &packet_log, &e);
                    } else if first_write {
                        info!("First video RTP packet written to WebRTC sender");
                        first_write = false;
                    }
                }
            }
        }
    });

    Ok(Some(video_tx))
}

pub async fn setup_audio_track(
    peer: Arc<dyn PeerConnection>,
    audio_codec_params: &rtsp::AudioCodecParams,
    input_id: String,
) -> Result<Option<UnboundedSender<Vec<u8>>>> {
    let audio_codec: RTCRtpCodec = audio_codec_params.clone().into();
    let initial_audio_payload_type = audio_codec_params.payload_type;
    let audio_track_id = format!("{}-audio", input_id);
    let audio_ssrc = rand::random::<u32>();
    let media_track = MediaStreamTrack::new(
        input_id.clone(),
        audio_track_id.clone(),
        audio_track_id.clone(),
        RtpCodecKind::Audio,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(audio_ssrc),
                ..Default::default()
            },
            codec: audio_codec.clone(),
            ..Default::default()
        }],
    );
    let audio_track = Arc::new(TrackLocalStaticRTP::new(media_track));

    let audio_sender = peer
        .add_track(audio_track.clone())
        .await
        .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    let (audio_tx, mut audio_rx) = unbounded_channel::<Vec<u8>>();
    let peer = peer.clone();

    tokio::spawn(async move {
        debug!("Audio codec: {}", audio_codec.mime_type);
        let mut first_write = true;
        let mut payload_type = initial_audio_payload_type;
        let mut payload_type_refreshed_at = Instant::now() - SENDER_PT_REFRESH_INTERVAL;
        let mut handler: Box<dyn RePayload + Send> = match audio_codec.mime_type.as_str() {
            MIME_TYPE_OPUS => Box::new(RePayloadCodec::new(audio_codec.mime_type.clone())),
            _ => Box::new(Forward::new()),
        };

        while let Some(data) = audio_rx.recv().await {
            if let Ok(packet) = Packet::unmarshal(&mut data.as_slice()) {
                trace!("Received audio packet: {}", packet);
                for mut packet in handler.payload(packet) {
                    trace!("Sending audio packet: {}", packet);
                    packet.header.ssrc = audio_ssrc;
                    refresh_payload_type(
                        &peer,
                        &audio_sender,
                        &audio_codec,
                        &mut payload_type,
                        &mut payload_type_refreshed_at,
                    )
                    .await;
                    packet.header.payload_type = payload_type;

                    let packet_log = RtpPacketLog::from(&packet);
                    match audio_track.write_rtp(packet).await {
                        Ok(()) if first_write => {
                            info!("First audio RTP packet written to WebRTC sender");
                            first_write = false;
                        }
                        Ok(()) => {}
                        Err(e) => log_write_rtp_error("audio", &packet_log, &e),
                    }
                }
            }
        }
    });

    Ok(Some(audio_tx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_dtls_transport_not_started_as_non_fatal_debug_error() {
        let error_kind =
            classify_write_rtp_error("I/O error: the DTLS transport has not started yet");

        assert_eq!(error_kind, "dtls_not_started");
        assert!(!is_fatal_write_rtp_error_kind(error_kind));
    }

    #[test]
    fn fmtp_satisfies_empty_required_is_always_compatible() {
        assert!(fmtp_satisfies("", "profile-id=0"));
        assert!(fmtp_satisfies("", ""));
    }

    #[test]
    fn fmtp_satisfies_requires_exact_value_match() {
        assert!(fmtp_satisfies("profile-id=0", "profile-id=0"));
        assert!(!fmtp_satisfies("profile-id=0", "profile-id=2"));
    }

    #[test]
    fn fmtp_satisfies_ignores_extra_offered_parameters() {
        // The remote answer may be more specific than our local codec.
        assert!(fmtp_satisfies(
            "profile-id=0",
            "profile-id=0;level-idx=5;tier=0"
        ));
    }

    #[test]
    fn fmtp_satisfies_requires_all_required_parameters() {
        assert!(!fmtp_satisfies("profile-id=0;level-idx=5", "profile-id=0"));
    }

    #[test]
    fn parse_fmtp_handles_whitespace_and_empty_parts() {
        assert_eq!(
            parse_fmtp("profile-id=0 ; level-idx=5;;"),
            vec![("profile-id", "0"), ("level-idx", "5")]
        );
    }

    #[test]
    fn nonzero_channels_filters_zero() {
        assert_eq!(nonzero_channels(0), None);
        assert_eq!(nonzero_channels(1), Some(1));
        assert_eq!(nonzero_channels(2), Some(2));
    }
}
