/// DataChannel <-> UDP bidirectional forwarding
///
/// Each stream maps to one independent UDP endpoint configured with explicit
/// listen and target socket addresses.
///
/// Configuration example (conf/live777.toml):
///   [stream.camera]
///   [stream.camera.channel]
///   listen = "0.0.0.0:7774"
///   target = "127.0.0.1:1234"
///
///   [stream.camera2]
///   [stream.camera2.channel]
///   listen = "0.0.0.0:7775"
///   target = "127.0.0.1:1235"
use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::config::ChannelConfig;

/// Buffer size for incoming UDP packets.
/// - WebRTC DataChannel SCTP max: 1024 * 64 = 65536 bytes
/// - RFC 8831 WebRTC DataChannel max: < 1024 * 16 = 16384 bytes
/// - IP UDP MTU: 1500 bytes
/// - Recommended single payload: < 1200 bytes
///
/// Control messages (e.g. PTZ commands) are well within this limit.
const UDP_BUF_SIZE: usize = 1500;

/// Spawn bidirectional forwarding tasks.
pub async fn spawn_channel(
    stream: String,
    mut dc_rx: broadcast::Receiver<Vec<u8>>,
    dc_tx: broadcast::Sender<Vec<u8>>,
    stream_cfg: ChannelConfig,
) -> anyhow::Result<()> {
    let listen = stream_cfg.listen;
    let target = stream_cfg.target;

    let socket = match UdpSocket::bind(listen).await {
        Ok(s) => {
            info!("channel [{}]: listen={} target={}", stream, listen, target);
            s
        }
        Err(e) => {
            warn!(
                "channel [{}]: bind socket failed on {}: {}",
                stream, listen, e
            );
            return Err(anyhow::anyhow!(
                "channel [{}]: bind {} failed: {}",
                stream,
                listen,
                e
            ));
        }
    };

    // Bidirectional forwarding using tokio::select! to handle both directions concurrently
    let stream_dc = stream.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; UDP_BUF_SIZE];
        loop {
            tokio::select! {
                // DataChannel -> UDP
                result = dc_rx.recv() => match result {
                    Ok(data) => {
                        if let Err(e) = socket.send_to(&data, target).await {
                            warn!("channel [{}]: send to {} failed: {}", stream_dc, target, e);
                        } else {
                            debug!("channel [{}]: DC->UDP {} bytes -> {}", stream_dc, data.len(), target);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("channel [{}]: lagged, dropped {} messages", stream_dc, n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("channel [{}]: channel closed", stream_dc);
                        break;
                    }
                },
                // UDP -> DataChannel (passthrough, no wrapping)
                result = socket.recv_from(&mut buf) => match result {
                    Ok((n, addr)) => {
                        let data = buf[..n].to_vec();
                        debug!("channel [{}]: UDP->DC {} bytes from {}", stream, n, addr);
                        if let Err(e) = dc_tx.send(data) {
                            warn!("channel [{}]: forward to DC failed: {}", stream, e);
                        }
                    }
                    Err(e) => {
                        warn!("channel [{}]: recv_from failed: {}", stream, e);
                    }
                },
            }
        }
    });

    Ok(())
}
