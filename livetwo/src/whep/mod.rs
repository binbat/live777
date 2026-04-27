mod channel;
mod output;
mod webrtc;

use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::mpsc::unbounded_channel;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use ::webrtc::peer_connection::RTCPeerConnection;
use cli::create_child;
use libwish::Client;
use tokio::sync::Notify;

use crate::transport;
use crate::utils::shutdown::graceful_shutdown;
use crate::utils::stats::start_stats_monitor;

pub use output::OutputTarget;
pub use webrtc::setup_whep_peer;

pub async fn from(
    ct: CancellationToken,
    target_url: String,
    whep_url: String,
    sdp_file: Option<String>,
    token: Option<String>,
    command: Option<String>,
    channel_url: Option<String>,
) -> Result<()> {
    info!("Starting WHEP session: {}", target_url);

    let (video_send, mut video_recv) = unbounded_channel::<Vec<u8>>();
    let (audio_send, mut audio_recv) = unbounded_channel::<Vec<u8>>();
    let codec_info = Arc::new(tokio::sync::Mutex::new(rtsp::CodecInfo::new()));

    let mut client = Client::new(whep_url.clone(), Client::get_auth_header_map(token.clone()));

    let (peer, answer, stats, dc_recv_rx, dc_send_tx) = webrtc::setup_whep_peer(
        ct.clone(),
        &mut client,
        video_send,
        audio_send,
        codec_info.clone(),
    )
    .await?;
    info!("WebRTC peer connection established");

    // Start DataChannel <-> UDP forwarding if channel_url is configured
    if let Some(url) = channel_url {
        channel::spawn_channel(url, dc_recv_rx, dc_send_tx).await?;
    }

    start_stats_monitor(ct.clone(), peer.clone(), stats.clone()).await;

    let stats_clone = stats.clone();
    let ct_clone = ct.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let summary: crate::utils::stats::StatsSummary = stats_clone.get_summary().await;
                    info!("{}", summary.format());
                }
                _ = ct_clone.cancelled() => {
                    info!("Stats reporter shutting down");
                    let final_summary: crate::utils::stats::StatsSummary = stats_clone.get_summary().await;
                    info!("Final Statistics:\n{}", final_summary.format());
                    break;
                }
            }
        }
    });

    tokio::time::sleep(Duration::from_secs(1)).await;
    let codec_info = codec_info.lock().await;
    debug!("Codec info: {:?}", codec_info);

    let (video_broadcast_tx, _) = broadcast::channel::<Vec<u8>>(1000);
    let (audio_broadcast_tx, _) = broadcast::channel::<Vec<u8>>(1000);

    let video_broadcast_tx = Arc::new(video_broadcast_tx);
    let audio_broadcast_tx = Arc::new(audio_broadcast_tx);

    let video_broadcast_tx_clone = video_broadcast_tx.clone();
    let ct_clone = ct.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(data) = video_recv.recv() => {
                    let _ = video_broadcast_tx_clone.send(data);
                }
                _ = ct_clone.cancelled() => {
                    info!("Video broadcast forwarder shutting down");
                    break;
                }
            }
        }
    });

    let audio_broadcast_tx_clone = audio_broadcast_tx.clone();
    let ct_clone = ct.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(data) = audio_recv.recv() => {
                    let _ = audio_broadcast_tx_clone.send(data);
                }
                _ = ct_clone.cancelled() => {
                    info!("Audio broadcast forwarder shutting down");
                    break;
                }
            }
        }
    });

    let notify = Arc::new(Notify::new());
    let target_url_clone = target_url.clone();
    let answer_sdp = answer.sdp.clone();
    let codec_info_clone = codec_info.clone();
    let notify_clone = notify.clone();
    let sdp_file_clone = sdp_file.clone();

    let ct_clone = ct.clone();
    let output_handle = tokio::spawn(async move {
        output::setup_output_target(
            ct_clone.clone(),
            &target_url_clone,
            &answer_sdp,
            sdp_file_clone,
            &codec_info_clone,
            notify_clone,
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let child = Arc::new(create_child(command)?);

    let output_target = output_handle.await??;
    info!("Output target configured: {:?}", output_target.scheme());

    let initial_transport_handle = start_initial_transport_task(
        ct.clone(),
        1,
        video_broadcast_tx.subscribe(),
        audio_broadcast_tx.subscribe(),
        output_target,
        peer.clone(),
    );

    if let Some(mut port_update_rx) = initial_transport_handle.port_update_rx {
        let peer_clone = peer.clone();
        let video_broadcast_tx_clone = video_broadcast_tx.clone();
        let audio_broadcast_tx_clone = audio_broadcast_tx.clone();
        let ct_clone = ct.clone();
        tokio::spawn(async move {
            let mut transport_handles: Vec<tokio::task::JoinHandle<()>> =
                vec![initial_transport_handle.task_handle];

            loop {
                tokio::select! {
                    Some(port_update) = port_update_rx.recv() => {
                        info!(
                            "Port update received for connection #{}: {:?}",
                            port_update.connection_id, port_update.media_info
                        );

                        if port_update.connection_id == 1 {
                            continue;
                        }

                        info!(
                            "Starting transport task for reconnection #{}",
                            port_update.connection_id
                        );
                        let handle = start_transport_task(
                            ct_clone.clone(),
                            port_update.connection_id,
                            video_broadcast_tx_clone.subscribe(),
                            audio_broadcast_tx_clone.subscribe(),
                            port_update.media_info,
                            peer_clone.clone(),
                        );

                        transport_handles.push(handle);
                        info!(
                            "Transport task started for connection #{}",
                            port_update.connection_id
                        );

                        transport_handles.retain(|h| !h.is_finished());
                        info!("Active transport tasks: {}", transport_handles.len());
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
                        if let Some(child_mutex) = child_clone.as_ref()
                            && let Ok(mut child_guard) = child_mutex.lock()
                                && let Ok(Some(status)) = child_guard.try_wait() {
                                    info!("Child process exited with status: {:?}", status);
                                    ct_clone.cancel();
                                    break;
                                }


                    }
                    _ = ct_clone.cancelled() => {
                        info!("Child monitor shutting down");
                        break;
                    }
                }
            }
        });
    }

    ct.cancelled().await;
    graceful_shutdown("WHEP", &mut client, peer).await;

    Ok(())
}

struct InitialTransportHandle {
    task_handle: tokio::task::JoinHandle<()>,
    port_update_rx: Option<tokio::sync::mpsc::UnboundedReceiver<rtsp::PortUpdate>>,
}

fn start_initial_transport_task(
    ct: CancellationToken,
    connection_id: u32,
    mut video_rx: broadcast::Receiver<Vec<u8>>,
    mut audio_rx: broadcast::Receiver<Vec<u8>>,
    mut output_target: OutputTarget,
    peer: Arc<RTCPeerConnection>,
) -> InitialTransportHandle {
    let port_update_rx = output_target.take_port_update_rx();

    let task_handle = tokio::spawn(async move {
        info!("Transport task #{} started", connection_id);

        let (video_tx, video_rx_unbounded) = unbounded_channel();
        let (audio_tx, audio_rx_unbounded) = unbounded_channel();

        let ct_clone = ct.clone();
        let video_forwarder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = video_rx.recv() => {
                        match result {
                            Ok(data) => {
                                if video_tx.send(data).is_err() {
                                    info!("Connection #{} video channel closed", connection_id);
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(
                                    "Connection #{} video lagged by {} messages",
                                    connection_id, n
                                );
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                info!("Connection #{} video broadcast closed", connection_id);
                                break;
                            }
                        }
                    }
                    _ = ct_clone.cancelled() => {
                        info!("Connection #{} video forwarder shutting down", connection_id);
                        break;
                    }
                }
            }
        });

        let ct_clone = ct.clone();
        let audio_forwarder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = audio_rx.recv() => {
                        match result {
                            Ok(data) => {
                                if audio_tx.send(data).is_err() {
                                    info!("Connection #{} audio channel closed", connection_id);
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(
                                    "Connection #{} audio lagged by {} messages",
                                    connection_id, n
                                );
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                info!("Connection #{} audio broadcast closed", connection_id);
                                break;
                            }
                        }
                    }
                    _ = ct_clone.cancelled() => {
                        info!("Connection #{} audio forwarder shutting down", connection_id);
                        break;
                    }
                }
            }
        });

        transport::connect_webrtc_to_output(
            video_rx_unbounded,
            audio_rx_unbounded,
            output_target,
            peer,
        )
        .await;

        let _ = tokio::join!(video_forwarder, audio_forwarder);

        info!("Transport task #{} stopped", connection_id);
    });

    InitialTransportHandle {
        task_handle,
        port_update_rx,
    }
}

fn start_transport_task(
    ct: CancellationToken,
    connection_id: u32,
    mut video_rx: broadcast::Receiver<Vec<u8>>,
    mut audio_rx: broadcast::Receiver<Vec<u8>>,
    media_info: rtsp::MediaInfo,
    peer: Arc<RTCPeerConnection>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("Transport task #{} started", connection_id);

        let (video_tx, video_rx_unbounded) = unbounded_channel();
        let (audio_tx, audio_rx_unbounded) = unbounded_channel();

        let ct_clone = ct.clone();
        let video_forwarder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = video_rx.recv() => {
                        match result {
                            Ok(data) => {
                                if video_tx.send(data).is_err() {
                                    info!("Connection #{} video channel closed", connection_id);
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(
                                    "Connection #{} video lagged by {} messages",
                                    connection_id, n
                                );
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                info!("Connection #{} video broadcast closed", connection_id);
                                break;
                            }
                        }
                    }
                    _ = ct_clone.cancelled() => {
                        info!("Connection #{} video forwarder shutting down", connection_id);
                        break;
                    }
                }
            }
        });

        let ct_clone = ct.clone();
        let audio_forwarder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = audio_rx.recv() => {
                        match result {
                            Ok(data) => {
                                if audio_tx.send(data).is_err() {
                                    info!("Connection #{} audio channel closed", connection_id);
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(
                                    "Connection #{} audio lagged by {} messages",
                                    connection_id, n
                                );
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                info!("Connection #{} audio broadcast closed", connection_id);
                                break;
                            }
                        }
                    }
                    _ = ct_clone.cancelled() => {
                        info!("Connection #{} audio forwarder shutting down", connection_id);
                        break;
                    }
                }
            }
        });

        let output_target = OutputTarget::from_media_info(media_info);

        transport::connect_webrtc_to_output(
            video_rx_unbounded,
            audio_rx_unbounded,
            output_target,
            peer,
        )
        .await;

        let _ = tokio::join!(video_forwarder, audio_forwarder);

        info!("Transport task #{} stopped", connection_id);
    })
}
