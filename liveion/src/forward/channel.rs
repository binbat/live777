/// DataChannel <-> UDP bidirectional forwarding
///
/// Each stream maps to one independent UDP endpoint configured via URL:
///   udp://<listen_host>:<listen_port>?host=<target_host>&port=<target_port>
///
/// - listen_host:listen_port  -- liveion binds here to receive replies from downstream
/// - target_host:target_port  -- liveion sends DataChannel messages to this address
///
/// Configuration example (conf/live777.toml):
///   [channel.streams.camera]
///   url = "udp://0.0.0.0:7774?host=127.0.0.1&port=1234"
///
///   [channel.streams.camera2]
///   url = "udp://0.0.0.0:7775?host=127.0.0.1&port=1235"
use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use crate::config::ChannelStream;

/// Buffer size for incoming UDP packets.
/// - WebRTC DataChannel SCTP max: 1024 * 64 = 65536 bytes
/// - RFC 8831 WebRTC DataChannel max: < 1024 * 16 = 16384 bytes
/// - IP UDP MTU: 1500 bytes
/// - Recommended single payload: < 1200 bytes
///
/// Control messages (e.g. PTZ commands) are well within this limit.
const UDP_BUF_SIZE: usize = 1024;

/// Spawn bidirectional forwarding tasks.
pub async fn spawn_channel(
    stream: String,
    mut dc_rx: broadcast::Receiver<Vec<u8>>,
    dc_tx: broadcast::Sender<Vec<u8>>,
    stream_cfg: ChannelStream,
) -> anyhow::Result<()> {
    let (listen_host, listen_port, target_host, target_port) = match stream_cfg.parse() {
        Some(v) => v,
        None => {
            warn!("channel [{}]: invalid url: {}", stream, stream_cfg.url);
            return Ok(());
        }
    };

    // Format socket addresses using url::Host to correctly handle IPv6 brackets
    let target = format!("{}:{}", target_host, target_port);
    let listen = format!("{}:{}", listen_host, listen_port);

    let socket = match UdpSocket::bind(&listen).await {
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
                        if let Err(e) = socket.send_to(&data, &target).await {
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
