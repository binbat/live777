use anyhow::{Result, anyhow};
use libwish::Client;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, info, warn};
use webrtc::{
    api::{APIBuilder, interceptor_registry::register_default_interceptors, media_engine::*},
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    peer_connection::{RTCPeerConnection, configuration::RTCConfiguration},
    rtcp::payload_feedbacks::{
        full_intra_request::FullIntraRequest, picture_loss_indication::PictureLossIndication,
    },
};

use crate::utils;
use crate::utils::stats::RtcpStats;
use crate::whip::track;

pub async fn setup_whip_peer(
    client: &mut Client,
    media_info: &rtsp::MediaInfo,
    complete_tx: UnboundedSender<()>,
    input_id: String,
) -> Result<(
    Arc<RTCPeerConnection>,
    Option<UnboundedSender<Vec<u8>>>,
    Option<UnboundedSender<Vec<u8>>>,
    Arc<RtcpStats>,
)> {
    let (peer, video_sender, audio_sender) =
        create_peer(media_info, complete_tx.clone(), input_id).await?;

    utils::webrtc::setup_connection(peer.clone(), client).await?;

    let stats = Arc::new(RtcpStats::new());

    setup_rtcp_listener_for_senders(peer.clone(), stats.clone()).await;

    utils::webrtc::setup_handlers(peer.clone(), complete_tx).await;

    Ok((peer, video_sender, audio_sender, stats))
}

async fn setup_rtcp_listener_for_senders(peer: Arc<RTCPeerConnection>, stats: Arc<RtcpStats>) {
    let senders = peer.get_senders().await;

    for sender in senders {
        if let Some(track) = sender.track().await {
            let track_kind = track.kind();
            let stats_clone = stats.clone();

            tokio::spawn(async move {
                debug!("Started RTCP listener for {} sender", track_kind);

                loop {
                    match sender.read_rtcp().await {
                        Ok((packets, _)) => {
                            for packet in packets {
                                if let Some(pli) =
                                    packet.as_any().downcast_ref::<PictureLossIndication>()
                                {
                                    stats_clone.increment_pli();
                                    debug!(
                                        "WHIP: Received PLI from browser for {} (total: {})",
                                        track_kind,
                                        stats_clone.get_pli_count()
                                    );
                                    debug!("PLI details: media_ssrc={}", pli.media_ssrc);
                                }

                                if let Some(fir) =
                                    packet.as_any().downcast_ref::<FullIntraRequest>()
                                {
                                    stats_clone.increment_fir();
                                    debug!(
                                        "WHIP: Received FIR from browser for {} (total: {})",
                                        track_kind,
                                        stats_clone.get_fir_count()
                                    );
                                    debug!("FIR details: media_ssrc={}", fir.media_ssrc);
                                }

                                if packet
                                    .as_any()
                                    .downcast_ref::<webrtc::rtcp::transport_feedbacks::transport_layer_nack::TransportLayerNack>()
                                    .is_some()
                                {
                                    stats_clone.increment_nack();
                                    debug!(
                                        "WHIP: Received NACK from browser for {} (total: {})",
                                        track_kind,
                                        stats_clone.get_nack_count()
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!("WHIP: Error reading RTCP from {} sender: {}", track_kind, e);
                            break;
                        }
                    }
                }

                info!("WHIP: RTCP listener stopped for {} sender", track_kind);
            });
        }
    }
}

async fn create_peer(
    media_info: &rtsp::MediaInfo,
    complete_tx: UnboundedSender<()>,
    input_id: String,
) -> Result<(
    Arc<RTCPeerConnection>,
    Option<UnboundedSender<Vec<u8>>>,
    Option<UnboundedSender<Vec<u8>>>,
)> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();

    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            username: "".to_string(),
            credential: "".to_string(),
        }],
        ..Default::default()
    };

    let peer = Arc::new(
        api.new_peer_connection(config)
            .await
            .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?,
    );

    utils::webrtc::setup_handlers(peer.clone(), complete_tx).await;

    let video_tx = if let Some(ref video_codec_params) = media_info.video_codec {
        track::setup_video_track(peer.clone(), video_codec_params, input_id.clone()).await?
    } else {
        None
    };

    let audio_tx = if let Some(ref audio_codec_params) = media_info.audio_codec {
        if is_supported_audio_codec(&audio_codec_params.codec) {
            track::setup_audio_track(peer.clone(), audio_codec_params, input_id).await?
        } else {
            debug!(
                "Audio codec '{}' not supported, skipping",
                audio_codec_params.codec
            );
            None
        }
    } else {
        None
    };

    Ok((peer, video_tx, audio_tx))
}

fn is_supported_audio_codec(codec: &str) -> bool {
    matches!(
        codec.to_uppercase().as_str(),
        "OPUS" | "G722" | "PCMU" | "PCMA"
    )
}
