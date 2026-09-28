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
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
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

/// Rebind attempts for the listen socket. A stream reset stops the old bridge
/// and waits for the socket release before re-initializing (see
/// `PeerForwardInternal::close`), so the first attempt normally succeeds; the
/// retry window only covers external holders of the port letting go slowly.
const BIND_RETRY_ATTEMPTS: u32 = 5;
const BIND_RETRY_INTERVAL: Duration = Duration::from_millis(200);

/// Handle to a running channel bridge. `stop` cancels the task and waits for
/// it to release the UDP socket, so a subsequent re-init can rebind the same
/// listen port deterministically.
pub(crate) struct ChannelHandle {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl ChannelHandle {
    pub(crate) async fn stop(self) {
        self.cancel.cancel();
        if let Err(err) = self.task.await {
            warn!("channel bridge task join error: {}", err);
        }
    }
}

/// Spawn the bidirectional forwarding task and return its handle.
pub(crate) async fn spawn_channel(
    stream: String,
    dc_rx: broadcast::Receiver<Vec<u8>>,
    dc_tx: broadcast::Sender<Vec<u8>>,
    stream_cfg: ChannelConfig,
) -> anyhow::Result<ChannelHandle> {
    let listen = stream_cfg.listen;
    let target = stream_cfg.target;

    let socket = bind_with_retry(&stream, listen, BIND_RETRY_ATTEMPTS, BIND_RETRY_INTERVAL).await?;
    info!("channel [{}]: listen={} target={}", stream, listen, target);

    let cancel = CancellationToken::new();
    let task = tokio::spawn(run_channel(
        stream,
        socket,
        target,
        dc_rx,
        dc_tx,
        cancel.clone(),
    ));
    Ok(ChannelHandle { cancel, task })
}

async fn bind_with_retry(
    stream: &str,
    listen: SocketAddr,
    attempts: u32,
    interval: Duration,
) -> anyhow::Result<UdpSocket> {
    let attempts = attempts.max(1);
    for attempt in 1..=attempts {
        match UdpSocket::bind(listen).await {
            Ok(socket) => return Ok(socket),
            Err(err) => {
                warn!(
                    "channel [{}]: bind {} failed (attempt {}/{}): {}",
                    stream, listen, attempt, attempts, err
                );
                if attempt == attempts {
                    return Err(anyhow::anyhow!(
                        "channel [{}]: bind {} failed after {} attempts: {}",
                        stream,
                        listen,
                        attempts,
                        err
                    ));
                }
                tokio::time::sleep(interval).await;
            }
        }
    }
    unreachable!("attempts is clamped to at least 1")
}

/// Bidirectional forwarding using tokio::select! to handle both directions
/// concurrently. The task exits — releasing the listen socket — when the
/// owning forward is cancelled or the DataChannel bus closes. Cancellation is
/// load-bearing: a stuck bus sender (e.g. a leaked data-channel task) would
/// otherwise keep the bus open and this socket bound forever, and the next
/// stream reset would fail its rebind.
async fn run_channel(
    stream: String,
    socket: UdpSocket,
    target: SocketAddr,
    mut dc_rx: broadcast::Receiver<Vec<u8>>,
    dc_tx: broadcast::Sender<Vec<u8>>,
    cancel: CancellationToken,
) {
    let listen = socket.local_addr().ok();
    let mut buf = vec![0u8; UDP_BUF_SIZE];
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!("channel [{}]: cancelled, releasing {:?}", stream, listen);
                break;
            }
            // DataChannel -> UDP
            result = dc_rx.recv() => match result {
                Ok(data) => {
                    if let Err(e) = socket.send_to(&data, target).await {
                        warn!("channel [{}]: send to {} failed: {}", stream, target, e);
                    } else {
                        debug!("channel [{}]: DC->UDP {} bytes -> {}", stream, data.len(), target);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("channel [{}]: lagged, dropped {} messages", stream, n);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    info!("channel [{}]: channel closed", stream);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg_events() -> (broadcast::Sender<Vec<u8>>, broadcast::Receiver<Vec<u8>>) {
        broadcast::channel(4)
    }

    /// The bridge releases its listen port on cancellation, so a fresh bridge
    /// can rebind the same address (regression: a stream reset used to fail
    /// its rebind because the old bridge only exited on bus close).
    #[tokio::test]
    async fn cancel_releases_listen_port() {
        let probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let listen = probe.local_addr().unwrap();
        drop(probe);

        let socket = UdpSocket::bind(listen).await.unwrap();
        let target: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let (tx, rx) = msg_events();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_channel(
            "s".to_string(),
            socket,
            target,
            rx,
            tx,
            cancel.clone(),
        ));

        assert!(UdpSocket::bind(listen).await.is_err());
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("bridge did not stop after cancel")
            .unwrap();
        assert!(
            UdpSocket::bind(listen).await.is_ok(),
            "listen port still occupied after bridge stopped"
        );
    }

    #[tokio::test]
    async fn forwards_bus_to_udp() {
        let bridge_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = target_socket.local_addr().unwrap();

        let (tx, rx) = msg_events();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_channel(
            "s".to_string(),
            bridge_socket,
            target,
            rx,
            tx.clone(),
            cancel.clone(),
        ));

        tx.send(b"ping".to_vec()).unwrap();
        let mut buf = [0u8; 16];
        let (n, _) =
            tokio::time::timeout(Duration::from_secs(1), target_socket.recv_from(&mut buf))
                .await
                .expect("timeout waiting for UDP packet")
                .unwrap();
        assert_eq!(&buf[..n], b"ping");

        cancel.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn forwards_udp_to_bus() {
        let bridge_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let bridge_addr = bridge_socket.local_addr().unwrap();
        let (tx, rx) = msg_events();
        let mut bus_rx = tx.subscribe();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_channel(
            "s".to_string(),
            bridge_socket,
            "127.0.0.1:9".parse().unwrap(),
            rx,
            tx,
            cancel.clone(),
        ));

        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender.send_to(b"pong", bridge_addr).await.unwrap();
        let data = tokio::time::timeout(Duration::from_secs(1), bus_rx.recv())
            .await
            .expect("timeout waiting for bus message")
            .unwrap();
        assert_eq!(data, b"pong");

        cancel.cancel();
        task.await.unwrap();
    }

    #[tokio::test]
    async fn exits_when_bus_closes() {
        let bridge_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        // The bridge reads and writes on two different buses (production
        // wires it to the publish/subscribe pair); closing the read side
        // must end the task.
        let (bus_tx, bus_rx) = msg_events();
        let (dc_tx, _dc_rx) = msg_events();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_channel(
            "s".to_string(),
            bridge_socket,
            "127.0.0.1:9".parse().unwrap(),
            bus_rx,
            dc_tx,
            cancel,
        ));

        drop(bus_tx);
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("bridge did not exit after bus closed")
            .unwrap();
    }

    #[tokio::test]
    async fn bind_with_retry_gives_up_on_occupied_port() {
        let holder = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let listen = holder.local_addr().unwrap();

        let err = bind_with_retry("s", listen, 2, Duration::from_millis(1))
            .await
            .unwrap_err();
        assert!(format!("{err:?}").contains("after 2 attempts"));
    }

    #[tokio::test]
    async fn bind_with_retry_succeeds_on_free_port() {
        let probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let listen = probe.local_addr().unwrap();
        drop(probe);

        let socket = bind_with_retry("s", listen, 1, Duration::from_millis(1))
            .await
            .unwrap();
        assert_eq!(socket.local_addr().unwrap(), listen);
    }

    #[tokio::test]
    async fn spawn_channel_reports_occupied_port() {
        let holder = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let listen = holder.local_addr().unwrap();

        let (tx, rx) = msg_events();
        let result = spawn_channel(
            "s".to_string(),
            rx,
            tx,
            ChannelConfig {
                listen,
                target: "127.0.0.1:9".parse().unwrap(),
            },
        )
        .await;
        assert!(result.is_err());
    }
}
