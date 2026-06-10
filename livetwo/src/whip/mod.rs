use std::sync::Arc;
use std::time::Duration;

use ::webrtc::peer_connection::RTCPeerConnectionState;
use anyhow::{Result, anyhow};
use std::process::ExitStatus;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

mod input;
mod track;
mod webrtc;

use cli::create_child;
use libwish::Client;
use rtsp::MediaProfile;

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
    let initial_profile = MediaProfile::from_media_info(input_source.media_info());

    let (peer, video_sender, audio_sender, stats, peer_state_rx, peer_diagnostics) =
        webrtc::setup_whip_peer(
            ct.clone(),
            &mut client,
            input_source.media_info(),
            target_url.clone(),
        )
        .await?;

    info!("Waiting for WebRTC peer connection to become connected");
    wait_for_peer_connected(
        peer_state_rx.clone(),
        Duration::from_secs(15),
        peer_diagnostics.clone(),
    )
    .await?;
    info!("WebRTC peer connection connected");

    info!("Starting stats monitor");
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

    info!("Starting input to WebRTC transport");
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
        let mut current_profile = initial_profile;
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

                        let next_profile = MediaProfile::from_media_info(&port_update.media_info);
                        if !current_profile.is_replace_compatible_with(&next_profile) {
                            tracing::error!(
                                "publisher restarted with incompatible codec, rebuilding media generation is required but current WHIP session cannot renegotiate; old profile: {:?}; new profile: {:?}",
                                current_profile,
                                next_profile,
                            );
                            if let Err(error) = peer_clone.close().await {
                                tracing::error!("Failed to close incompatible WHIP peer: {}", error);
                            }
                            ct_clone.cancel();
                            break;
                        }

                        info!(
                            "publisher restarted with same codec, reusing media generation for WHIP transport restart; profile: {:?}",
                            next_profile
                        );
                        current_profile = next_profile;

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
        tokio::select! {
            _ = ct.cancelled() => {
                graceful_shutdown("WHIP", &mut client, peer).await;
                Ok(())
            }
            result = wait_for_unexpected_peer_end(peer.clone(), peer_state_rx, peer_diagnostics) => {
                ct.cancel();
                graceful_shutdown("WHIP", &mut client, peer).await;
                result
            }
            status = wait_for_child_exit(child.clone()) => {
                let status = status?;
                info!("Child process exited with status: {:?}", status);
                ct.cancel();
                graceful_shutdown("WHIP", &mut client, peer).await;
                Err(anyhow!("WHIP child process exited before shutdown: {status}"))
            }
        }
    } else {
        tokio::select! {
            _ = ct.cancelled() => {
                graceful_shutdown("WHIP", &mut client, peer).await;
                Ok(())
            }
            result = wait_for_unexpected_peer_end(peer.clone(), peer_state_rx, peer_diagnostics) => {
                ct.cancel();
                graceful_shutdown("WHIP", &mut client, peer).await;
                result
            }
        }
    }
}

async fn wait_for_peer_connected(
    mut state_rx: watch::Receiver<RTCPeerConnectionState>,
    timeout: Duration,
    diagnostics: Arc<webrtc::WhipPeerDiagnostics>,
) -> Result<()> {
    if *state_rx.borrow() == RTCPeerConnectionState::Connected {
        return Ok(());
    }

    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            result = state_rx.changed() => {
                result.map_err(|_| anyhow!("WHIP peer connection state channel closed while waiting for connected, {}", diagnostics.format()))?;

                let state = *state_rx.borrow();
                if state == RTCPeerConnectionState::Connected {
                    return Ok(());
                }

                if matches!(
                    state,
                    RTCPeerConnectionState::Failed
                        | RTCPeerConnectionState::Closed
                        | RTCPeerConnectionState::Disconnected
                ) {
                    return Err(anyhow!(
                        "WHIP peer connection failed while waiting for connected: state={state}, {}",
                        diagnostics.format(),
                    ));
                }
            }
            _ = &mut deadline => {
                return Err(anyhow!(
                    "WHIP peer connection timed out after {}ms while waiting for connected, {}",
                    timeout.as_millis(),
                    diagnostics.format(),
                ));
            }
        }
    }
}

async fn wait_for_unexpected_peer_end(
    peer: Arc<dyn ::webrtc::peer_connection::PeerConnection>,
    mut state_rx: watch::Receiver<RTCPeerConnectionState>,
    diagnostics: Arc<webrtc::WhipPeerDiagnostics>,
) -> Result<()> {
    let mut saw_connected = *state_rx.borrow() == RTCPeerConnectionState::Connected;

    loop {
        state_rx
            .changed()
            .await
            .map_err(|_| anyhow!("WHIP peer connection state channel closed"))?;

        let state = *state_rx.borrow();
        if state == RTCPeerConnectionState::Connected {
            saw_connected = true;
        }

        if matches!(
            state,
            RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected
        ) {
            let ice_stats = webrtc::format_ice_stats(peer.clone()).await;
            return Err(anyhow!(
                "WHIP peer connection ended before shutdown: state={state}, connected_before={saw_connected}, {}, ice_stats=[{}]",
                diagnostics.format(),
                ice_stats
            ));
        }
    }
}

async fn wait_for_child_exit(child: Arc<Option<cli::ChildGuard>>) -> Result<ExitStatus> {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;

        if let Some(child_guard_wrapper) = child.as_ref() {
            let status = child_guard_wrapper
                .lock()
                .map_err(|_| anyhow!("WHIP child process mutex poisoned"))?
                .try_wait()?;
            if let Some(status) = status {
                return Ok(status);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_for_peer_connected_returns_when_state_is_already_connected() {
        let (_tx, rx) = watch::channel(RTCPeerConnectionState::Connected);

        wait_for_peer_connected(
            rx,
            Duration::from_millis(1),
            Arc::new(webrtc::WhipPeerDiagnostics::default()),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn wait_for_peer_connected_returns_when_state_changes_to_connected() {
        let (tx, rx) = watch::channel(RTCPeerConnectionState::New);
        tokio::spawn(async move {
            tx.send(RTCPeerConnectionState::Connecting).unwrap();
            tx.send(RTCPeerConnectionState::Connected).unwrap();
        });

        wait_for_peer_connected(
            rx,
            Duration::from_secs(1),
            Arc::new(webrtc::WhipPeerDiagnostics::default()),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn wait_for_peer_connected_errors_on_terminal_state() {
        let (tx, rx) = watch::channel(RTCPeerConnectionState::New);
        tokio::spawn(async move {
            tx.send(RTCPeerConnectionState::Failed).unwrap();
        });

        let error = wait_for_peer_connected(
            rx,
            Duration::from_secs(1),
            Arc::new(webrtc::WhipPeerDiagnostics::default()),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("state=failed"), "{error}");
        assert!(error.contains("connection_states="), "{error}");
    }

    #[tokio::test]
    async fn wait_for_peer_connected_errors_on_closed_state() {
        let (tx, rx) = watch::channel(RTCPeerConnectionState::New);
        tokio::spawn(async move {
            tx.send(RTCPeerConnectionState::Closed).unwrap();
        });

        let error = wait_for_peer_connected(
            rx,
            Duration::from_secs(1),
            Arc::new(webrtc::WhipPeerDiagnostics::default()),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("state=closed"), "{error}");
        assert!(error.contains("connection_states="), "{error}");
    }

    #[tokio::test]
    async fn wait_for_peer_connected_errors_on_disconnected_state() {
        let (tx, rx) = watch::channel(RTCPeerConnectionState::New);
        tokio::spawn(async move {
            tx.send(RTCPeerConnectionState::Disconnected).unwrap();
        });

        let error = wait_for_peer_connected(
            rx,
            Duration::from_secs(1),
            Arc::new(webrtc::WhipPeerDiagnostics::default()),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("state=disconnected"), "{error}");
        assert!(error.contains("connection_states="), "{error}");
    }

    #[tokio::test]
    async fn wait_for_peer_connected_errors_on_timeout() {
        let (_tx, rx) = watch::channel(RTCPeerConnectionState::New);

        let error = wait_for_peer_connected(
            rx,
            Duration::from_millis(1),
            Arc::new(webrtc::WhipPeerDiagnostics::default()),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("timed out"), "{error}");
        assert!(error.contains("connection_states="), "{error}");
    }
}
