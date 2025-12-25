use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, trace, warn};

const CONTROL_BUFFER_SIZE: usize = 1024;

/// Start UDP control receiver that forwards control messages to DataChannel
/// 
/// This is a generic UDP-to-DataChannel bridge that:
/// - Listens on a UDP port for control messages
/// - Forwards raw bytes to the DataChannel broadcast system
/// - Supports bidirectional communication (optional feedback)
pub async fn start(
    control_port: u16,
    stream_id: String,
    datachannel_tx: broadcast::Sender<Vec<u8>>,
    mut datachannel_rx: broadcast::Receiver<Vec<u8>>,
    mut shutdown_rx: mpsc::Receiver<()>,
) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(format!("0.0.0.0:{}", control_port)).await?;
    info!(
        stream_id = %stream_id,
        port = control_port,
        "UDP control receiver started"
    );

    let socket = Arc::new(socket);
    let socket_clone = socket.clone();

    // Spawn task for receiving UDP control messages and forwarding to DataChannel
    let stream_id_clone = stream_id.clone();
    let udp_to_dc_task = tokio::spawn(async move {
        let mut buffer = vec![0u8; CONTROL_BUFFER_SIZE];
        let mut packet_count = 0u64;

        loop {
            match socket_clone.recv_from(&mut buffer).await {
                Ok((size, _peer_addr)) => {
                    packet_count += 1;

                    if packet_count % 100 == 0 {
                        trace!(
                            stream_id = %stream_id_clone,
                            packets = packet_count,
                            "UDP control packets received"
                        );
                    }

                    // Forward raw bytes to DataChannel
                    if let Err(e) = datachannel_tx.send(buffer[..size].to_vec()) {
                        warn!(
                            stream_id = %stream_id_clone,
                            error = %e,
                            "Failed to forward control message to DataChannel"
                        );
                    }
                }
                Err(e) => {
                    error!(
                        stream_id = %stream_id_clone,
                        error = %e,
                        "UDP recv error"
                    );
                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                }
            }
        }
    });

    // Spawn task for receiving DataChannel messages and sending back via UDP (optional feedback)
    let stream_id_clone = stream_id.clone();
    let dc_to_udp_task = tokio::spawn(async move {
        let mut feedback_count = 0u64;
        let last_peer_addr: Option<std::net::SocketAddr> = None;

        loop {
            match datachannel_rx.recv().await {
                Ok(data) => {
                    feedback_count += 1;

                    // Try to send feedback to the last known peer
                    // In a real scenario, you might want to maintain a list of peers
                    if let Some(peer_addr) = last_peer_addr {
                        if let Err(e) = socket.send_to(&data, peer_addr).await {
                            warn!(
                                stream_id = %stream_id_clone,
                                error = %e,
                                "Failed to send feedback via UDP"
                            );
                        } else if feedback_count % 100 == 0 {
                            trace!(
                                stream_id = %stream_id_clone,
                                feedbacks = feedback_count,
                                "UDP feedback messages sent"
                            );
                        }
                    } else {
                        trace!(
                            stream_id = %stream_id_clone,
                            "No peer address known, skipping feedback"
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        stream_id = %stream_id_clone,
                        error = %e,
                        "DataChannel receive error"
                    );
                    break;
                }
            }
        }
    });

    // Wait for shutdown signal
    let _ = shutdown_rx.recv().await;
    info!(
        stream_id = %stream_id,
        port = control_port,
        "UDP control receiver shutting down"
    );

    // Abort tasks
    udp_to_dc_task.abort();
    dc_to_udp_task.abort();

    Ok(())
}
