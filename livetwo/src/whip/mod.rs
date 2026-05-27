use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

mod input;
mod track;
mod webrtc;

use cli::create_child;
use libwish::Client;

use crate::transport;
use crate::utils::shutdown::graceful_shutdown;
use crate::utils::stats::start_stats_monitor;

pub use input::InputSource;
pub(crate) use webrtc::log_rtcp_feedback_packet;
pub use webrtc::setup_whip_peer;

pub async fn into(
    ct: CancellationToken,
    target_url: String,
    whip_url: String,
    token: Option<String>,
    command: Option<String>,
) -> Result<()> {
    info!("Starting WHIP session: {}", target_url);

    let mut client = Client::new(whip_url.clone(), Client::get_auth_header_map(token.clone()));

    let child = Arc::new(create_child(command)?);

    let mut input_source = input::setup_input_source(ct.clone(), &target_url).await?;
    info!("Input source configured: {:?}", input_source.scheme());

    let (original_target, original_listen) = input_source.address_config();
    let original_config = (original_target.to_string(), original_listen.to_string());

    let port_update_rx = input_source.take_port_update_rx();

    let (peer, video_sender, audio_sender, stats) = webrtc::setup_whip_peer(
        ct.clone(),
        &mut client,
        input_source.media_info(),
        target_url.clone(),
    )
    .await?;
    info!("WebRTC peer connection established");

    start_stats_monitor(ct.clone(), peer.clone(), stats.clone()).await;

    let stats_clone = stats.clone();
    let ct_clone = ct.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let summary = stats_clone.get_summary().await;
                    info!("{}", summary.format());
                }
                _ = ct_clone.cancelled() => {
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
        let ct_clone = ct.clone();
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
                    _ = ct_clone.cancelled() => {
                        info!("Port update listener shutting down");
                        break;
                    }
                }
            }
        });
    }

    if child.as_ref().is_some() {
        let child_clone = child.clone();
        let ct_clone = ct.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        if let Some(child_guard_wrapper) = child_clone.as_ref()
                            && let Ok(mut child_guard) = child_guard_wrapper.lock()
                            && let Ok(Some(status)) = child_guard.try_wait() {
                                info!("Child process exited with status: {:?}", status);
                                ct_clone.cancel();
                                break;
                            }
                    }
                    _ = ct_clone.cancelled() => {
                        break;
                    }
                }
            }
        });
    }

    ct.cancelled().await;
    graceful_shutdown("WHIP", &mut client, peer).await;

    Ok(())
}
