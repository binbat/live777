/// Cable <-> UDP bidirectional forwarding
///
/// Each cable maps to UDP bridges configured as `udp://` endpoints on the
/// stream's `sources` (broadcast seat) and `targets` (subscriber seat).
/// `listen` and `target` are independent — one without the other makes a
/// one-way bridge.
///
/// Configuration example (conf/live777.toml):
///   [stream.camera]
///   [[stream.camera.cables]]
///   label = "control"
///   [[stream.camera.sources]]
///   [[stream.camera.sources.cables]]
///   label = "control"
///   url = "udp://0.0.0.0:7774?host=127.0.0.1&port=1234"
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::DcUdpEndpoint;

/// Buffer size for incoming UDP packets.
/// - WebRTC DataChannel SCTP max: 1024 * 64 = 65536 bytes
/// - RFC 8831 WebRTC DataChannel max: < 1024 * 16 = 16384 bytes
/// - IP UDP MTU: 1500 bytes
/// - Recommended single payload: < 1200 bytes
///
/// Control messages (e.g. PTZ commands) are well within this limit.
const UDP_BUF_SIZE: usize = 1500;

/// Rebind attempts for the listen socket. Cable bridges outlive their
/// forward (the cable hub is stream-level), so the retry window only covers
/// a previous hub's bridge task releasing the port asynchronously after its
/// cancel, or slow external holders letting go.
const BIND_RETRY_ATTEMPTS: u32 = 5;
const BIND_RETRY_INTERVAL: Duration = Duration::from_millis(200);

/// Spawn the bridge task. `dc_rx`/`dc_tx` are the cable bus ends resolved by
/// the caller from the endpoint's seat: `dc_rx` is present when
/// `endpoint.target` is (cable -> UDP export), `dc_tx` when `endpoint.listen`
/// is (UDP -> cable injection). The task runs until `cancel` fires or the bus
/// closes; its socket is released on exit.
pub(crate) async fn spawn_channel(
    name: String,
    endpoint: &DcUdpEndpoint,
    dc_rx: Option<broadcast::Receiver<Vec<u8>>>,
    dc_tx: Option<broadcast::Sender<Vec<u8>>>,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let socket = match endpoint.listen {
        Some(listen) => {
            bind_with_retry(&name, listen, BIND_RETRY_ATTEMPTS, BIND_RETRY_INTERVAL).await?
        }
        // Export-only bridge: no listen address, send from an ephemeral port
        // of the target's address family.
        None => {
            let any: SocketAddr = if endpoint.target.is_some_and(|t| t.is_ipv6()) {
                "[::]:0".parse().expect("valid v6 any addr")
            } else {
                "0.0.0.0:0".parse().expect("valid v4 any addr")
            };
            UdpSocket::bind(any).await?
        }
    };
    info!(
        "channel [{}]: listen={} target={}",
        name,
        socket.local_addr()?,
        endpoint
            .target
            .map(|t| t.to_string())
            .unwrap_or_else(|| "-".to_string()),
    );

    tokio::spawn(run_channel(
        name,
        socket,
        endpoint.target,
        dc_rx,
        dc_tx,
        cancel,
    ));
    Ok(())
}

async fn bind_with_retry(
    name: &str,
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
                    name, listen, attempt, attempts, err
                );
                if attempt == attempts {
                    return Err(anyhow::anyhow!(
                        "channel [{}]: bind {} failed after {} attempts: {}",
                        name,
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
/// concurrently; a missing bus end disables that arm, making the bridge
/// one-way. The task exits — releasing the listen socket — when the owning
/// cable hub is cancelled or the bus closes. Cancellation is load-bearing:
/// a stuck bus sender (e.g. a leaked data-channel task) would otherwise keep
/// the bus open and this socket bound forever.
async fn run_channel(
    name: String,
    socket: UdpSocket,
    target: Option<SocketAddr>,
    mut dc_rx: Option<broadcast::Receiver<Vec<u8>>>,
    dc_tx: Option<broadcast::Sender<Vec<u8>>>,
    cancel: CancellationToken,
) {
    let listen = socket.local_addr().ok();
    let mut buf = vec![0u8; UDP_BUF_SIZE];
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                info!("channel [{}]: cancelled, releasing {:?}", name, listen);
                break;
            }
            // Cable -> UDP
            result = async { dc_rx.as_mut().expect("arm gated on is_some").recv().await },
                if dc_rx.is_some() && target.is_some() =>
            {
                let target = target.expect("arm gated on is_some");
                match result {
                    Ok(data) => {
                        if let Err(e) = socket.send_to(&data, target).await {
                            warn!("channel [{}]: send to {} failed: {}", name, target, e);
                        } else {
                            debug!("channel [{}]: DC->UDP {} bytes -> {}", name, data.len(), target);
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("channel [{}]: lagged, dropped {} messages", name, n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("channel [{}]: cable bus closed", name);
                        break;
                    }
                }
            }
            // UDP -> Cable (passthrough, no wrapping)
            result = socket.recv_from(&mut buf), if dc_tx.is_some() => {
                match result {
                    Ok((n, addr)) => {
                        let data = buf[..n].to_vec();
                        debug!("channel [{}]: UDP->DC {} bytes from {}", name, n, addr);
                        if let Some(dc_tx) = &dc_tx {
                            // A bus send fails only when the bus has no active
                            // receivers; the message is dropped, the bridge
                            // keeps running.
                            if let Err(e) = dc_tx.send(data) {
                                warn!("channel [{}]: forward to DC failed (dropped): {}", name, e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("channel [{}]: recv_from failed: {}", name, e);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DcUdpEndpoint;

    fn msg_events() -> (broadcast::Sender<Vec<u8>>, broadcast::Receiver<Vec<u8>>) {
        broadcast::channel(4)
    }

    fn udp_config(listen: Option<SocketAddr>, target: Option<SocketAddr>) -> DcUdpEndpoint {
        DcUdpEndpoint { listen, target }
    }

    /// The bridge releases its listen port on cancellation, so a later bridge
    /// can rebind the same address.
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
            Some(target),
            Some(rx),
            Some(tx),
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
            Some(target),
            Some(rx),
            Some(tx.clone()),
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
            Some("127.0.0.1:9".parse().unwrap()),
            Some(rx),
            Some(tx),
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

    /// Inject-only bridge (listen without target): no cable->UDP arm.
    #[tokio::test]
    async fn inject_only_bridge_has_no_egress() {
        let bridge_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let bridge_addr = bridge_socket.local_addr().unwrap();
        let (tx, rx) = msg_events();
        let mut bus_rx = tx.subscribe();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_channel(
            "s".to_string(),
            bridge_socket,
            None, // no target: export arm disabled
            None, // no bus receiver needed
            Some(tx),
            cancel.clone(),
        ));
        drop(rx);

        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender.send_to(b"in", bridge_addr).await.unwrap();
        let data = tokio::time::timeout(Duration::from_secs(1), bus_rx.recv())
            .await
            .expect("timeout waiting for bus message")
            .unwrap();
        assert_eq!(data, b"in");

        cancel.cancel();
        task.await.unwrap();
    }

    /// Export-only bridge (target without listen): sends from an ephemeral
    /// port, never receives.
    #[tokio::test]
    async fn export_only_bridge_sends_from_ephemeral_port() {
        let target_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = target_socket.local_addr().unwrap();

        let (tx, rx) = msg_events();
        let cancel = CancellationToken::new();
        spawn_channel(
            "s".to_string(),
            &udp_config(None, Some(target)),
            Some(rx),
            None, // no listen: injection arm disabled
            cancel.clone(),
        )
        .await
        .unwrap();

        tx.send(b"out".to_vec()).unwrap();
        let mut buf = [0u8; 16];
        let (n, from) =
            tokio::time::timeout(Duration::from_secs(1), target_socket.recv_from(&mut buf))
                .await
                .expect("timeout waiting for UDP packet")
                .unwrap();
        assert_eq!(&buf[..n], b"out");
        assert_ne!(from.port(), 0);

        cancel.cancel();
    }

    #[tokio::test]
    async fn exits_when_bus_closes() {
        let bridge_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        // The bridge reads and writes on two different bus ends (production
        // wires them per role); closing the read side must end the task.
        let (bus_tx, bus_rx) = msg_events();
        let (dc_tx, _dc_rx) = msg_events();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_channel(
            "s".to_string(),
            bridge_socket,
            Some("127.0.0.1:9".parse().unwrap()),
            Some(bus_rx),
            Some(dc_tx),
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
            &udp_config(Some(listen), Some("127.0.0.1:9".parse().unwrap())),
            Some(rx),
            Some(tx),
            CancellationToken::new(),
        )
        .await;
        assert!(result.is_err());
    }
}
