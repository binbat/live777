use anyhow::{Result, anyhow};
use libwish::Client;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, trace, warn};
use webrtc::{
    peer_connection::{RTCPeerConnection, sdp::session_description::RTCSessionDescription},
    rtcp::payload_feedbacks::{
        full_intra_request::FullIntraRequest, picture_loss_indication::PictureLossIndication,
    },
    rtp_transceiver::{
        RTCRtpTransceiverInit, rtp_codec::RTPCodecType,
        rtp_transceiver_direction::RTCRtpTransceiverDirection,
    },
    util::MarshalSize,
};

use crate::utils;
use crate::utils::stats::RtcpStats;

pub async fn setup_whep_peer(
    client: &mut Client,
    video_send: UnboundedSender<Vec<u8>>,
    audio_send: UnboundedSender<Vec<u8>>,
    complete_tx: UnboundedSender<()>,
    codec_info: Arc<tokio::sync::Mutex<rtsp::CodecInfo>>,
) -> Result<(
    Arc<RTCPeerConnection>,
    RTCSessionDescription,
    Arc<RtcpStats>,
)> {
    let peer = create_peer(
        video_send,
        audio_send,
        complete_tx.clone(),
        codec_info.clone(),
    )
    .await?;

    utils::webrtc::setup_connection(peer.clone(), client).await?;

    let answer = peer
        .remote_description()
        .await
        .ok_or_else(|| anyhow!("No remote description"))?;

    let stats = Arc::new(RtcpStats::new());

    setup_rtcp_listener_for_senders(peer.clone(), stats.clone()).await;

    Ok((peer, answer, stats))
}

async fn setup_rtcp_listener_for_senders(peer: Arc<RTCPeerConnection>, stats: Arc<RtcpStats>) {
    let senders = peer.get_senders().await;

    for sender in senders {
        if let Some(track) = sender.track().await {
            let track_kind = track.kind();
            let stats_clone = stats.clone();

            tokio::spawn(async move {
                info!("WHEP: Started RTCP monitor for {} sender", track_kind);

                loop {
                    match sender.read_rtcp().await {
                        Ok((packets, _)) => {
                            for packet in packets {
                                // PLI - Picture Loss Indication
                                if packet
                                    .as_any()
                                    .downcast_ref::<PictureLossIndication>()
                                    .is_some()
                                {
                                    stats_clone.increment_pli();
                                    debug!(
                                        "WHEP: Sent PLI to browser for {} (total: {})",
                                        track_kind,
                                        stats_clone.get_pli_count()
                                    );
                                }

                                // FIR - Full Intra Request
                                if packet.as_any().downcast_ref::<FullIntraRequest>().is_some() {
                                    stats_clone.increment_fir();
                                    debug!(
                                        "WHEP: Sent FIR to browser for {} (total: {})",
                                        track_kind,
                                        stats_clone.get_fir_count()
                                    );
                                }

                                // NACK
                                if packet
                                    .as_any()
                                    .downcast_ref::<webrtc::rtcp::transport_feedbacks::transport_layer_nack::TransportLayerNack>()
                                    .is_some()
                                {
                                    stats_clone.increment_nack();
                                    debug!(
                                        "WHEP: Sent NACK to browser for {} (total: {})",
                                        track_kind,
                                        stats_clone.get_nack_count()
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!("WHEP: Error reading RTCP from {} sender: {}", track_kind, e);
                            break;
                        }
                    }
                }

                info!("WHEP: RTCP monitor stopped for {} sender", track_kind);
            });
        }
    }
}

async fn create_peer(
    video_send: UnboundedSender<Vec<u8>>,
    audio_send: UnboundedSender<Vec<u8>>,
    complete_tx: UnboundedSender<()>,
    codec_info: Arc<tokio::sync::Mutex<rtsp::CodecInfo>>,
) -> Result<Arc<RTCPeerConnection>> {
    let (api, config) = utils::webrtc::create_api().await?;

    let peer = Arc::new(
        api.build()
            .new_peer_connection(config)
            .await
            .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?,
    );

    peer.add_transceiver_from_kind(
        RTPCodecType::Video,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            send_encodings: vec![],
        }),
    )
    .await
    .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    peer.add_transceiver_from_kind(
        RTPCodecType::Audio,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            send_encodings: vec![],
        }),
    )
    .await
    .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    utils::webrtc::setup_handlers(peer.clone(), complete_tx).await;

    peer.on_track(Box::new({
        let codec_info = codec_info.clone();
        move |track, _, _| {
            let video_sender = video_send.clone();
            let audio_sender = audio_send.clone();
            let codec = track.codec().clone();
            let track_kind = track.kind();

            let codec_info = codec_info.clone();
            tokio::spawn(async move {
                let mut codec_info = codec_info.lock().await;
                if track_kind == RTPCodecType::Video {
                    debug!("Updating video codec info: {:?}", codec);
                    codec_info.video_codec = Some(codec.clone());
                } else if track_kind == RTPCodecType::Audio {
                    debug!("Updating audio codec info: {:?}", codec);
                    codec_info.audio_codec = Some(codec.clone());
                }
            });

            let sender = match track_kind {
                RTPCodecType::Video => Some(video_sender),
                RTPCodecType::Audio => Some(audio_sender),
                _ => None,
            };

            if let Some(sender) = sender {
                tokio::spawn(async move {
                    let mut b = [0u8; 1500];
                    while let Ok((rtp_packet, _)) = track.read(&mut b).await {
                        trace!("Received RTP packet: {:?}", rtp_packet);
                        let size = rtp_packet.marshal_size();
                        let data = b[0..size].to_vec();
                        let _ = sender.send(data);
                    }
                });
            }
            Box::pin(async {})
        }
    }));

    Ok(peer)
}
