use std::sync::Arc;

use anyhow::{Result, anyhow};
use libwish::Client;
use rtc::peer_connection::configuration::interceptor_registry::{
    configure_nack, configure_rtcp_reports, configure_simulcast_extension_headers, configure_twcc,
};
use rtc_rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use rtc_rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtc_rtcp::payload_feedbacks::receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate;
use rtc_rtcp::receiver_report::ReceiverReport;
use rtc_rtcp::transport_feedbacks::transport_layer_cc::TransportLayerCc;
use rtc_rtcp::transport_feedbacks::transport_layer_nack::TransportLayerNack;
use tokio::sync::{Notify, mpsc::UnboundedSender, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer, RTCPeerConnectionState, Registry,
};

use crate::utils;
use crate::utils::stats::RtcpStats;
use crate::whip::track;

pub async fn setup_whip_peer(
    ct: CancellationToken,
    client: &mut Client,
    media_info: &rtsp::MediaInfo,
    input_id: String,
) -> Result<(
    Arc<dyn PeerConnection>,
    Option<UnboundedSender<Vec<u8>>>,
    Option<UnboundedSender<Vec<u8>>>,
    Arc<RtcpStats>,
    watch::Receiver<RTCPeerConnectionState>,
)> {
    let gather_complete = Arc::new(Notify::new());
    let (peer, video_sender, audio_sender, state_rx) =
        create_peer(ct.clone(), media_info, input_id, gather_complete.clone()).await?;

    utils::webrtc::setup_connection(peer.clone(), client, gather_complete).await?;

    let stats = Arc::new(RtcpStats::new());

    Ok((peer, video_sender, audio_sender, stats, state_rx))
}

async fn create_peer(
    ct: CancellationToken,
    media_info: &rtsp::MediaInfo,
    input_id: String,
    gather_complete: Arc<Notify>,
) -> Result<(
    Arc<dyn PeerConnection>,
    Option<UnboundedSender<Vec<u8>>>,
    Option<UnboundedSender<Vec<u8>>>,
    watch::Receiver<RTCPeerConnectionState>,
)> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    let registry = Registry::new();
    let registry = configure_nack(registry, &mut m);
    let registry = configure_rtcp_reports(registry);
    configure_simulcast_extension_headers(&mut m)?;
    let registry = configure_twcc(registry, &mut m)?;
    info!("WHIP peer configured with NACK, RTCP reports, and full TWCC");

    let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
    let handler: Arc<dyn PeerConnectionEventHandler> = Arc::new(WhipPeerHandler {
        _ct: ct,
        gather_complete,
        state_tx,
    });

    let config = RTCConfigurationBuilder::new()
        .with_ice_servers(vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            username: "".to_string(),
            credential: "".to_string(),
        }])
        .build();

    let peer: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .with_handler(handler)
            .with_udp_addrs(vec!["0.0.0.0:0"])
            .with_configuration(config)
            .build()
            .await
            .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?,
    );

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

    Ok((peer, video_tx, audio_tx, state_rx))
}

struct WhipPeerHandler {
    _ct: CancellationToken,
    gather_complete: Arc<Notify>,
    state_tx: watch::Sender<RTCPeerConnectionState>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for WhipPeerHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        info!("WHIP connection state changed: {}", state);
        let _ = self.state_tx.send(state);
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            info!("WHIP ICE gathering complete");
            self.gather_complete.notify_one();
        }
    }
}

fn is_supported_audio_codec(codec: &str) -> bool {
    matches!(codec.to_uppercase().as_str(), "OPUS" | "G722")
}

pub(crate) fn log_rtcp_feedback_packet(source: &str, packet: &dyn rtc_rtcp::packet::Packet) {
    let any = packet.as_any();
    if any.downcast_ref::<TransportLayerCc>().is_some() {
        debug!("{source}: received RTCP transport-cc feedback");
    } else if let Some(remb) = any.downcast_ref::<ReceiverEstimatedMaximumBitrate>() {
        debug!(
            "{source}: received RTCP goog-remb feedback: {} bps",
            remb.bitrate
        );
    } else if let Some(rr) = any.downcast_ref::<ReceiverReport>() {
        debug!(
            "{source}: received RTCP receiver report with {} report blocks",
            rr.reports.len()
        );
    } else if any.downcast_ref::<PictureLossIndication>().is_some() {
        debug!("{source}: received RTCP PLI");
    } else if any.downcast_ref::<FullIntraRequest>().is_some() {
        debug!("{source}: received RTCP FIR");
    } else if any.downcast_ref::<TransportLayerNack>().is_some() {
        debug!("{source}: received RTCP NACK");
    }
}
