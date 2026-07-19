use std::sync::Arc;

use anyhow::Result;
use libwish::Client;
use rtc_rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use rtc_rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtc_rtcp::payload_feedbacks::receiver_estimated_maximum_bitrate::ReceiverEstimatedMaximumBitrate;
use rtc_rtcp::receiver_report::ReceiverReport;
use rtc_rtcp::transport_feedbacks::transport_layer_cc::TransportLayerCc;
use rtc_rtcp::transport_feedbacks::transport_layer_nack::TransportLayerNack;
use tokio::sync::{Notify, mpsc::UnboundedSender, watch};
use tracing::debug;
use webrtc::peer_connection::{PeerConnection, RTCPeerConnectionState};

use crate::utils;
use crate::utils::stats::RtcpStats;
use crate::whip::core::{self, PublishDiagnostics, PublishPeerOptions};
use crate::whip::track;

pub async fn setup_whip_peer(
    client: &mut Client,
    media_info: &rtsp::MediaInfo,
    input_id: String,
) -> Result<(
    Arc<dyn PeerConnection>,
    Option<UnboundedSender<Vec<u8>>>,
    Option<UnboundedSender<Vec<u8>>>,
    Arc<RtcpStats>,
    watch::Receiver<RTCPeerConnectionState>,
    Arc<PublishDiagnostics>,
)> {
    let gather_complete = Arc::new(Notify::new());

    let publish =
        core::create_publish_peer(gather_complete.clone(), PublishPeerOptions::default()).await?;
    let peer = publish.peer;

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

    utils::webrtc::setup_connection(peer.clone(), client, gather_complete).await?;
    publish.diagnostics.set_sdp_summaries(
        peer.local_description()
            .await
            .map(|description| utils::webrtc::summarize_sdp(&description.sdp))
            .unwrap_or_else(|| "<no local description>".to_string()),
        peer.remote_description()
            .await
            .map(|description| utils::webrtc::summarize_sdp(&description.sdp))
            .unwrap_or_else(|| "<no remote description>".to_string()),
    );

    let stats = Arc::new(RtcpStats::new());

    Ok((
        peer,
        video_tx,
        audio_tx,
        stats,
        publish.state_rx,
        publish.diagnostics,
    ))
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
