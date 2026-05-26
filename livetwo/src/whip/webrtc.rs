use std::sync::Arc;

use anyhow::{Result, anyhow};
use libwish::Client;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use tokio::sync::{Notify, mpsc::UnboundedSender};
use tokio_util::sync::CancellationToken;
use tracing::debug;
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, RTCConfigurationBuilder, RTCIceServer,
    Registry,
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
)> {
    let gather_complete = Arc::new(Notify::new());
    let (peer, video_sender, audio_sender) =
        create_peer(ct.clone(), media_info, input_id, gather_complete.clone()).await?;

    utils::webrtc::setup_connection(peer.clone(), client, gather_complete).await?;

    let stats = Arc::new(RtcpStats::new());

    Ok((peer, video_sender, audio_sender, stats))
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
)> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    let registry = Registry::new();
    let registry = register_default_interceptors(registry, &mut m)?;

    let handler = utils::webrtc::create_event_handler(ct, gather_complete);

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

    Ok((peer, video_tx, audio_tx))
}

fn is_supported_audio_codec(codec: &str) -> bool {
    matches!(codec.to_uppercase().as_str(), "OPUS" | "G722")
}
