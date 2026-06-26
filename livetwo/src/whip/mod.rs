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
pub use webrtc::format_ice_stats;
pub(crate) use webrtc::log_rtcp_feedback_packet;
pub use webrtc::setup_whip_peer;

const WAIT_FOR_PEER_CONNECTED_TIMEOUT: Duration = Duration::from_secs(15);

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
    info!("WHIP peer setup completed; waiting for WebRTC connection");

    wait_for_peer_connected(
        peer.clone(),
        peer_state_rx.clone(),
        peer_diagnostics.clone(),
    )
    .await?;
    info!("WebRTC peer connection connected");

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
    peer: Arc<dyn ::webrtc::peer_connection::PeerConnection>,
    state_rx: watch::Receiver<RTCPeerConnectionState>,
    diagnostics: Arc<webrtc::WhipPeerDiagnostics>,
) -> Result<()> {
    wait_for_peer_connected_with_timeout(
        state_rx,
        diagnostics,
        WAIT_FOR_PEER_CONNECTED_TIMEOUT,
        move || {
            let peer = peer.clone();
            async move { webrtc::format_ice_stats(peer).await }
        },
    )
    .await
}

async fn wait_for_peer_connected_with_timeout<F, Fut>(
    mut state_rx: watch::Receiver<RTCPeerConnectionState>,
    diagnostics: Arc<webrtc::WhipPeerDiagnostics>,
    timeout: Duration,
    ice_stats: F,
) -> Result<()>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = String>,
{
    let wait_result = tokio::time::timeout(timeout, async {
        loop {
            let state = *state_rx.borrow_and_update();
            match state {
                RTCPeerConnectionState::Connected => return Ok(()),
                RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Closed
                | RTCPeerConnectionState::Disconnected => {
                    return Err(anyhow!(
                        "WHIP peer connection ended before becoming connected: state={state}"
                    ));
                }
                _ => {}
            }

            state_rx
                .changed()
                .await
                .map_err(|_| anyhow!("WHIP peer connection state channel closed"))?;
        }
    })
    .await;

    match wait_result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            let ice_stats = ice_stats().await;
            Err(anyhow!(
                "{error}, {}, ice_stats=[{}]",
                diagnostics.format(),
                ice_stats
            ))
        }
        Err(_) => {
            let ice_stats = ice_stats().await;
            Err(anyhow!(
                "WHIP peer connection timed out waiting for connected after {:?}: {}, ice_stats=[{}]",
                timeout,
                diagnostics.format(),
                ice_stats
            ))
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
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn waits_for_connected_before_starting_media_transport() {
        let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
        let diagnostics = Arc::new(webrtc::WhipPeerDiagnostics::default());
        let started = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));

        let task = {
            let started = started.clone();
            let order = order.clone();
            tokio::spawn(async move {
                wait_for_peer_connected_with_timeout(
                    state_rx.clone(),
                    diagnostics,
                    Duration::from_secs(1),
                    || async { "ice-stats".to_string() },
                )
                .await?;

                started.fetch_add(1, Ordering::SeqCst);
                order.lock().unwrap().push("stats");
                started.fetch_add(1, Ordering::SeqCst);
                order.lock().unwrap().push("transport");
                Result::<()>::Ok(())
            })
        };

        tokio::time::sleep(Duration::from_millis(25)).await;
        assert_eq!(started.load(Ordering::SeqCst), 0);

        state_tx.send(RTCPeerConnectionState::Connected).unwrap();

        task.await.unwrap().unwrap();
        assert_eq!(started.load(Ordering::SeqCst), 2);
        assert_eq!(order.lock().unwrap().as_slice(), ["stats", "transport"]);
    }

    #[tokio::test]
    async fn returns_error_with_diagnostics_when_peer_fails_before_connected() {
        for state in [
            RTCPeerConnectionState::Failed,
            RTCPeerConnectionState::Closed,
            RTCPeerConnectionState::Disconnected,
        ] {
            let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
            let diagnostics = Arc::new(webrtc::WhipPeerDiagnostics::default());

            state_tx.send(state).unwrap();

            let error = wait_for_peer_connected_with_timeout(
                state_rx,
                diagnostics,
                Duration::from_secs(1),
                || async { "candidate_pair state=failed".to_string() },
            )
            .await
            .unwrap_err()
            .to_string();

            assert!(error.contains("before becoming connected"));
            assert!(error.contains("connection_states="));
            assert!(error.contains("candidate_pair state=failed"));
        }
    }

    #[tokio::test]
    async fn returns_error_with_diagnostics_when_wait_for_connected_times_out() {
        let (_state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
        let diagnostics = Arc::new(webrtc::WhipPeerDiagnostics::default());

        let error = wait_for_peer_connected_with_timeout(
            state_rx,
            diagnostics,
            Duration::from_millis(10),
            || async { "<no ice candidate stats>".to_string() },
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("timed out waiting"));
        assert!(error.contains("connection_states="));
        assert!(error.contains("<no ice candidate stats>"));
    }
}
