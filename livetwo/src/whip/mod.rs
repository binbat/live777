mod input;
mod track;
mod webrtc;

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use cli::create_child;
use libwish::Client;

use crate::transport;
use crate::utils::shutdown::{ShutdownSignal, wait_for_shutdown};
use crate::utils::stats::start_stats_monitor;

pub use input::InputSource;
pub use webrtc::setup_whip_peer;

pub async fn into(
    target_url: String,
    whip_url: String,
    token: Option<String>,
    command: Option<String>,
) -> Result<()> {
    info!("Starting WHIP session: {}", target_url);

    let shutdown = ShutdownSignal::new();
    let shutdown_clone = shutdown.clone();

    let (complete_tx, complete_rx) = unbounded_channel();
    let mut client = Client::new(whip_url.clone(), Client::get_auth_header_map(token.clone()));

    let child = Arc::new(create_child(command)?);

    let child_for_cleanup = child.clone();
    let shutdown_for_cleanup = shutdown.clone();

    tokio::spawn(async move {
        shutdown_for_cleanup.wait().await;
        if let Some(child_mutex) = child_for_cleanup.as_ref()
            && let Ok(mut child_guard) = child_mutex.lock()
        {
            info!("Killing child process");
            let _ = child_guard.kill();
        }
    });

    let mut input_source = input::setup_input_source(&target_url, complete_tx.clone()).await?;
    info!("Input source configured: {:?}", input_source.scheme());

    let (original_target, original_listen) = input_source.address_config();
    let original_config = (original_target.to_string(), original_listen.to_string());

    let port_update_rx = input_source.take_port_update_rx();

    let (peer, video_sender, audio_sender, stats) = webrtc::setup_whip_peer(
        &mut client,
        input_source.media_info(),
        complete_tx.clone(),
        target_url.clone(),
    )
    .await?;
    info!("WebRTC peer connection established");

    start_stats_monitor(peer.clone(), stats.clone(), shutdown.clone()).await;

    let stats_clone = stats.clone();
    let shutdown_stats = shutdown.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let summary = stats_clone.get_summary().await;
                    info!("{}", summary.format());
                }
                _ = shutdown_stats.wait() => {
                    info!("Stats reporter shutting down");
                    let final_summary = stats_clone.get_summary().await;
                    info!("Final Statistics:\n{}", final_summary.format());
                    break;
                }
            }
        }
    });

    let mut transport_handle: Option<JoinHandle<()>> = Some(
        transport::connect_input_to_webrtc(
            input_source,
            video_sender.clone(),
            audio_sender.clone(),
            peer.clone(),
        )
        .await?,
    );

    info!("Input connected to WebRTC");

    if let Some(mut rx) = port_update_rx {
        let video_sender_clone = video_sender.clone();
        let audio_sender_clone = audio_sender.clone();
        let peer_clone = peer.clone();
        let config_clone = original_config.clone();
        let shutdown_port = shutdown.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(port_update) = rx.recv() => {
                        info!(
                            "Port update received for connection #{}: {:?}",
                            port_update.connection_id, port_update.media_info
                        );

                        if port_update.connection_id == 1 {
                            continue;
                        }

                        if let Some(handle) = transport_handle.take() {
                            info!("Aborting old transport task");
                            handle.abort();
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        }

                        let (target_host, listen_host) = &config_clone;
                        let temp_input_source = InputSource::new_with_media_info(
                            input::InputScheme::RtspServer,
                            port_update.media_info.clone(),
                            target_host.clone(),
                            listen_host.clone(),
                        );

                        info!("Restarting transport layer with new ports");
                        match transport::connect_input_to_webrtc(
                            temp_input_source,
                            video_sender_clone.clone(),
                            audio_sender_clone.clone(),
                            peer_clone.clone(),
                        )
                        .await
                        {
                            Ok(new_handle) => {
                                transport_handle = Some(new_handle);
                                info!("Transport layer restarted successfully");
                            }
                            Err(e) => {
                                tracing::error!("Failed to restart transport layer: {}", e);
                            }
                        }
                    }
                    _ = shutdown_port.wait() => {
                        info!("Port update listener shutting down");
                        break;
                    }
                }
            }
        });
    }

    if child.as_ref().is_some() {
        let shutdown_child = shutdown.clone();
        let complete_tx_child = complete_tx.clone();
        let child_clone = child.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        if let Some(child_mutex) = child_clone.as_ref()
                            && let Ok(mut child_guard) = child_mutex.lock()
                                && let Ok(Some(status)) = child_guard.try_wait() {
                                    info!("Child process exited with status: {:?}", status);
                                    let _ = complete_tx_child.send(());
                                    break;
                                }


                    }
                    _ = shutdown_child.wait() => {
                        info!("Child monitor shutting down");
                        break;
                    }
                }
            }
        });
    }

    let reason = wait_for_shutdown(shutdown_clone, complete_rx).await;
    info!("Shutting down WHIP session, reason: {}", reason);

    graceful_shutdown(&mut client, peer).await;

    Ok(())
}

async fn graceful_shutdown(
    client: &mut Client,
    peer: Arc<::webrtc::peer_connection::RTCPeerConnection>,
) {
    info!("Starting WHIP graceful shutdown");

    let shutdown_timeout = Duration::from_secs(5);

    tokio::select! {
        _ = async {
            match client.remove_resource().await {
                Ok(_) => info!("WHIP resource removed successfully"),
                Err(e) => warn!("Failed to remove WHIP resource: {}", e),
            }

            match peer.close().await {
                Ok(_) => info!("PeerConnection closed successfully"),
                Err(e) => warn!("Failed to close peer connection: {}", e),
            }

            info!("WebRTC resources cleaned up");
        } => {
            info!("WHIP graceful shutdown completed");
        }
        _ = tokio::time::sleep(shutdown_timeout) => {
            warn!("WHIP graceful shutdown timed out after {:?}", shutdown_timeout);
        }
    }
}
