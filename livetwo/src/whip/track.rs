use anyhow::{Result, anyhow};
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::media_engine::*;
use rtc::rtp::packet::Packet;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use rtc_shared::marshal::Unmarshal;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tracing::{debug, error, trace, warn};
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
use webrtc::peer_connection::PeerConnection;

use crate::payload::{Forward, RePayload, RePayloadCodec};

pub async fn setup_video_track(
    peer: Arc<dyn PeerConnection>,
    video_codec_params: &rtsp::VideoCodecParams,
    input_id: String,
) -> Result<Option<UnboundedSender<Vec<u8>>>> {
    let video_codec: RTCRtpCodec = video_codec_params.clone().into();
    let video_track_id = format!("{}-video", input_id);
    let media_track = MediaStreamTrack::new(
        input_id.clone(),
        video_track_id.clone(),
        video_track_id.clone(),
        RtpCodecKind::Video,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(rand::random::<u32>()),
                ..Default::default()
            },
            codec: video_codec.clone(),
            ..Default::default()
        }],
    );
    let video_track = Arc::new(TrackLocalStaticRTP::new(media_track));

    peer.add_track(video_track.clone())
        .await
        .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    let (video_tx, mut video_rx) = unbounded_channel::<Vec<u8>>();
    let video_codec_params = video_codec_params.clone();

    tokio::spawn(async move {
        debug!("Video codec: {}", video_codec.mime_type);

        let mut handler: Box<dyn RePayload + Send> = match video_codec.mime_type.as_str() {
            MIME_TYPE_VP8 | MIME_TYPE_VP9 => {
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

                for packet in handler.payload(packet) {
                    trace!(
                        "Sending video packet: seq={}, ts={}, marker={}",
                        packet.header.sequence_number,
                        packet.header.timestamp,
                        packet.header.marker
                    );

                    if let Err(e) = video_track.write_rtp(packet).await {
                        error!("Failed to write RTP: {}", e);
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
    let audio_track_id = format!("{}-audio", input_id);
    let media_track = MediaStreamTrack::new(
        input_id.clone(),
        audio_track_id.clone(),
        audio_track_id.clone(),
        RtpCodecKind::Audio,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(rand::random::<u32>()),
                ..Default::default()
            },
            codec: audio_codec.clone(),
            ..Default::default()
        }],
    );
    let audio_track = Arc::new(TrackLocalStaticRTP::new(media_track));

    peer.add_track(audio_track.clone())
        .await
        .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    let (audio_tx, mut audio_rx) = unbounded_channel::<Vec<u8>>();

    tokio::spawn(async move {
        debug!("Audio codec: {}", audio_codec.mime_type);
        let mut handler: Box<dyn RePayload + Send> = match audio_codec.mime_type.as_str() {
            MIME_TYPE_OPUS => Box::new(RePayloadCodec::new(audio_codec.mime_type.clone())),
            _ => Box::new(Forward::new()),
        };

        while let Some(data) = audio_rx.recv().await {
            if audio_codec.mime_type == MIME_TYPE_G722 {
                // In v0.20, TrackLocalStaticRTP only accepts RTP packets.
                // G722 raw data needs to be wrapped in RTP packets first.
                warn!("G722 raw write not yet supported in v0.20");
            } else if let Ok(packet) = Packet::unmarshal(&mut data.as_slice()) {
                trace!("Received audio packet: {}", packet);
                for packet in handler.payload(packet) {
                    trace!("Sending audio packet: {}", packet);
                    let _ = audio_track.write_rtp(packet).await;
                }
            }
        }
    });

    Ok(Some(audio_tx))
}
