mod rtcp;
mod tcp;
mod udp;

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, UnboundedSender};
use tokio_util::sync::CancellationToken;
use tracing::debug;
use webrtc::peer_connection::PeerConnection;

use crate::whep::OutputTarget;
use crate::whip::InputSource;

pub use rtcp::spawn_rtcp_listener;
pub use tcp::TcpHandler;
pub use udp::UdpHandler;

pub async fn connect_input_to_webrtc(
    ct: CancellationToken,
    mut input_source: InputSource,
    video_sender: Option<UnboundedSender<Vec<u8>>>,
    audio_sender: Option<UnboundedSender<Vec<u8>>>,
    peer: Arc<dyn PeerConnection>,
) -> Result<()> {
    if let Some((tx, rx)) = input_source.take_channels() {
        debug!("Setting up TCP interleaved transport");
        let handler = TcpHandler::new(input_source.media_info());
        handler.spawn_input_to_webrtc(rx, video_sender, audio_sender, peer.clone());
        handler.spawn_webrtc_rtcp_to_output(ct.clone(), peer.clone(), tx);
    } else {
        debug!("Setting up UDP transport");
        let handler = UdpHandler::new();

        handler
            .spawn_input_to_webrtc(
                input_source.media_info(),
                input_source.listen_host(),
                video_sender,
                audio_sender,
            )
            .await?;

        handler
            .spawn_webrtc_rtcp_to_output(
                input_source.media_info(),
                input_source.target_host(),
                peer.clone(),
            )
            .await?;

        handler
            .spawn_output_rtcp_to_webrtc(
                input_source.media_info(),
                input_source.target_host(),
                peer.clone(),
            )
            .await;
    }

    Ok(())
}

pub async fn connect_webrtc_to_output(
    video_recv: Receiver<Vec<u8>>,
    audio_recv: Receiver<Vec<u8>>,
    mut output_target: OutputTarget,
    peer: Arc<dyn PeerConnection>,
) -> Result<()> {
    if let Some((tx, rx)) = output_target.take_channels() {
        debug!("Setting up TCP interleaved transport");
        let handler = TcpHandler::new(output_target.media_info());
        handler.spawn_webrtc_to_output(video_recv, audio_recv, tx);
        handler.spawn_output_rtcp_to_webrtc(rx, peer);
    } else {
        debug!("Setting up UDP transport");
        let handler = UdpHandler::new();

        handler
            .spawn_webrtc_to_output(
                video_recv,
                audio_recv,
                output_target.media_info(),
                output_target.target_host(),
            )
            .await?;
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        handler
            .spawn_webrtc_rtcp_to_output(
                output_target.media_info(),
                output_target.target_host(),
                peer.clone(),
            )
            .await
            .unwrap_or_else(|e| {
                tracing::error!("Failed to start RTCP sender: {}", e);
            });

        handler
            .spawn_output_rtcp_to_webrtc(
                output_target.media_info(),
                output_target.target_host(),
                peer,
            )
            .await;
    }

    Ok(())
}
