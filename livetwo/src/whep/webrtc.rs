use std::sync::Arc;

use anyhow::{Result, anyhow};
use libwish::Client;
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
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
    ct: CancellationToken,
    client: &mut Client,
    video_send: UnboundedSender<Vec<u8>>,
    audio_send: UnboundedSender<Vec<u8>>,
    codec_info: Arc<tokio::sync::Mutex<rtsp::CodecInfo>>,
) -> Result<(
    Arc<RTCPeerConnection>,
    RTCSessionDescription,
    Arc<RtcpStats>,
    mpsc::UnboundedReceiver<Vec<u8>>,
    mpsc::UnboundedSender<Vec<u8>>,
)> {
    let (dc_recv_tx, dc_recv_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (dc_send_tx, dc_send_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let peer = create_peer(
        ct,
        video_send,
        audio_send,
        codec_info.clone(),
        dc_recv_tx,
        dc_send_rx,
    )
    .await?;

    utils::webrtc::setup_connection(peer.clone(), client).await?;

    let answer = peer
        .remote_description()
        .await
        .ok_or_else(|| anyhow!("No remote description"))?;

    let stats = Arc::new(RtcpStats::new());

    setup_rtcp_listener_for_senders(peer.clone(), stats.clone()).await;

    Ok((peer, answer, stats, dc_recv_rx, dc_send_tx))
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
    ct: CancellationToken,
    video_send: UnboundedSender<Vec<u8>>,
    audio_send: UnboundedSender<Vec<u8>>,
    codec_info: Arc<tokio::sync::Mutex<rtsp::CodecInfo>>,
    dc_recv_tx: mpsc::UnboundedSender<Vec<u8>>,
    mut dc_send_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) -> Result<Arc<RTCPeerConnection>> {
    let (api, config) = utils::webrtc::create_api().await?;

    let peer = Arc::new(
        api.build()
            .new_peer_connection(config)
            .await
            .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?,
    );

    // Create DataChannel to participate in liveion's WHEP group
    let dc = peer
        .create_data_channel("control", None)
        .await
        .map_err(|e| anyhow!("create_data_channel failed: {:?}", e))?;

    // detach 模式：在 on_open 里 detach，然后用 raw read/write loop
    let dc_for_detach = dc.clone();
    dc.on_open(Box::new(move || {
        info!("whepfrom: DataChannel opened");
        let dc_recv_tx = dc_recv_tx.clone();
        Box::pin(async move {
            let raw = match dc_for_detach.detach().await {
                Ok(raw) => raw,
                Err(e) => {
                    warn!("whepfrom: DataChannel detach failed: {}", e);
                    return;
                }
            };

            // raw read loop: DataChannel -> dc_recv_tx
            let raw_r = raw.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                loop {
                    match raw_r.read(&mut buf).await {
                        Ok(0) => {
                            info!("whepfrom: DataChannel read loop ended");
                            break;
                        }
                        Ok(n) => {
                            let _ = dc_recv_tx.send(buf[..n].to_vec());
                        }
                        Err(e) => {
                            info!("whepfrom: DataChannel read error: {}", e);
                            break;
                        }
                    }
                }
            });

            // raw write loop: dc_send_rx -> DataChannel
            tokio::spawn(async move {
                while let Some(data) = dc_send_rx.recv().await {
                    if let Err(e) = raw.write(&data.into()).await {
                        warn!("whepfrom: DataChannel write failed: {}", e);
                        break;
                    }
                }
            });
        })
    }));

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

    utils::webrtc::setup_handlers(ct, peer.clone()).await;

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
