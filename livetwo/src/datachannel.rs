/// DataChannel <-> UDP bidirectional forwarding for whepfrom and whipinto
///
/// Symmetric to liveion's channel.rs, but on the client side (WHEP subscriber
/// or WHIP publisher). Messages received from liveion via DataChannel are
/// forwarded to UDP, and messages received from UDP are sent back to liveion
/// via DataChannel.
///
/// URL format: udp://<listen_host>:<listen_port>?host=<target_host>&port=<target_port>
use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use webrtc::data_channel::{DataChannel, DataChannelEvent};

/// DataChannel label used to join liveion's per-stream channel group for
/// bidirectional control messaging, on both the WHEP and the WHIP leg.
pub const DATA_CHANNEL_LABEL: &str = "control";

/// Buffer size for incoming UDP packets.
/// - WebRTC DataChannel SCTP max: 1024 * 64 = 65536 bytes
/// - RFC 8831 WebRTC DataChannel max: < 1024 * 16 = 16384 bytes
/// - IP UDP MTU: 1500 bytes
const UDP_BUF_SIZE: usize = 1500;

/// Parse UDP URL into (listen_host, listen_port, target_host, target_port)
pub fn parse_channel_url(url: &str) -> Option<(String, u16, String, u16)> {
    let parsed = url::Url::parse(url).ok()?;
    if parsed.scheme() != "udp" {
        return None;
    }

    // url::Url::host_str() returns IPv6 addresses already wrapped in brackets (e.g. "[::1]"),
    // but plain IPv4/domain without brackets. Normalize both to bracketed form for socket addresses.
    let listen_host = parsed.host_str()?.to_string();
    let listen_host = if listen_host.starts_with('[') {
        listen_host
    } else if listen_host.contains(':') {
        format!("[{}]", listen_host)
    } else {
        listen_host
    };
    let listen_port = parsed.port()?;

    let mut target_host = String::new();
    let mut target_port: u16 = 0;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "host" => target_host = value.into_owned(),
            "port" => target_port = value.parse().ok()?,
            _ => {}
        }
    }
    if target_host.is_empty() || target_port == 0 {
        return None;
    }

    // query_pairs() returns raw IPv6 without brackets, add them for socket addresses
    let target_host = if target_host.contains(':') {
        format!("[{}]", target_host)
    } else {
        target_host
    };

    Some((listen_host, listen_port, target_host, target_port))
}

/// Spawn bidirectional DataChannel <-> UDP forwarding.
/// `dc_recv`: messages received from liveion DataChannel
/// `dc_send`: sender to write messages back to liveion DataChannel
/// `ct`: session-scoped cancellation — the bridge must stop (releasing the
/// listen socket) when the session ends, not only when the process-wide
/// token fires, or an in-process reconnect finds the port still occupied.
pub async fn spawn_channel(
    url: String,
    mut dc_recv: mpsc::UnboundedReceiver<Vec<u8>>,
    dc_send: mpsc::UnboundedSender<Vec<u8>>,
    ct: CancellationToken,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let (listen_host, listen_port, target_host, target_port) =
        parse_channel_url(&url).ok_or_else(|| anyhow::anyhow!("invalid channel url: {}", url))?;

    let target = format!("{}:{}", target_host, target_port);
    let listen = format!("{}:{}", listen_host, listen_port);

    let socket = match UdpSocket::bind(&listen).await {
        Ok(s) => {
            info!("channel: listen={} target={}", listen, target);
            s
        }
        Err(e) => {
            warn!("channel: bind {} failed: {}", listen, e);
            return Err(anyhow::anyhow!("bind {} failed: {}", listen, e));
        }
    };

    let task = tokio::spawn(async move {
        let mut buf = vec![0u8; UDP_BUF_SIZE];
        loop {
            tokio::select! {
                biased;
                _ = ct.cancelled() => {
                    info!("channel: session ended, releasing {}", listen);
                    break;
                }
                // DataChannel -> UDP (messages from liveion)
                msg = dc_recv.recv() => {
                    match msg {
                        Some(data) => {
                            if let Err(e) = socket.send_to(&data, &target).await {
                                warn!("channel: send to {} failed: {}", target, e);
                            } else {
                                debug!("channel: DC->UDP {} bytes -> {}", data.len(), target);
                            }
                        }
                        None => {
                            info!("channel: DC recv closed");
                            break;
                        }
                    }
                },
                // UDP -> DataChannel (messages to liveion)
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((n, addr)) => {
                            let data = buf[..n].to_vec();
                            debug!("channel: UDP->DC {} bytes from {}", n, addr);
                            if dc_send.send(data).is_err() {
                                info!("channel: DC send closed");
                                break;
                            }
                        }
                        Err(e) => {
                            warn!("channel: recv_from failed: {}", e);
                        }
                    }
                },
            }
        }
    });

    Ok(task)
}

/// Poll a DataChannel both ways: inbound messages go to `dc_recv_tx`,
/// messages from `dc_send_rx` are sent over the channel. `owner` tags the
/// log lines (e.g. "whepfrom" / "whipinto").
pub fn run_data_channel_loop(
    dc: Arc<dyn DataChannel>,
    dc_recv_tx: mpsc::UnboundedSender<Vec<u8>>,
    mut dc_send_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    owner: &'static str,
) {
    tokio::spawn(async move {
        // Wait for OnOpen
        loop {
            match dc.poll().await {
                Some(DataChannelEvent::OnOpen) => {
                    info!("{}: DataChannel opened", owner);
                    break;
                }
                Some(DataChannelEvent::OnClose) => {
                    info!("{}: DataChannel closed before open", owner);
                    return;
                }
                None => {
                    info!("{}: DataChannel poll ended before open", owner);
                    return;
                }
                _ => {}
            }
        }

        loop {
            tokio::select! {
                event = dc.poll() => match event {
                    Some(DataChannelEvent::OnMessage(msg))
                        if dc_recv_tx.send(msg.data.to_vec()).is_err() =>
                    {
                        debug!("{}: DataChannel recv channel closed", owner);
                        break;
                    }
                    Some(DataChannelEvent::OnClose) => {
                        info!("{}: DataChannel closed", owner);
                        break;
                    }
                    None => {
                        info!("{}: DataChannel poll ended", owner);
                        break;
                    }
                    _ => {}
                },
                msg = dc_send_rx.recv() => match msg {
                    Some(data) => {
                        if let Err(e) = dc.send(bytes::BytesMut::from(&data[..])).await {
                            warn!("{}: DataChannel send failed: {}", owner, e);
                            break;
                        }
                    }
                    None => {
                        info!("{}: DataChannel send channel closed", owner);
                        break;
                    }
                },
            }
        }
    });
}

/// Cancels the wrapped token on drop, so every exit path of the owning
/// session — early error returns included — tears down session-scoped tasks.
pub struct CancelOnDrop(pub CancellationToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UdpSocket;
    use tokio::sync::mpsc;

    #[test]
    fn test_parse_channel_url_ipv4() {
        let (listen_host, listen_port, target_host, target_port) =
            parse_channel_url("udp://0.0.0.0:9001?host=127.0.0.1&port=9000").unwrap();
        assert_eq!(listen_host, "0.0.0.0");
        assert_eq!(listen_port, 9001);
        assert_eq!(target_host, "127.0.0.1");
        assert_eq!(target_port, 9000);
    }

    #[test]
    fn test_parse_channel_url_ipv6() {
        let (listen_host, listen_port, target_host, target_port) =
            parse_channel_url("udp://[::]:9001?host=::1&port=9000").unwrap();
        assert_eq!(listen_host, "[::]");
        assert_eq!(listen_port, 9001);
        assert_eq!(target_host, "[::1]");
        assert_eq!(target_port, 9000);
    }

    #[test]
    fn test_parse_channel_url_invalid_scheme() {
        assert!(parse_channel_url("tcp://0.0.0.0:9001?host=127.0.0.1&port=9000").is_none());
    }

    #[test]
    fn test_parse_channel_url_missing_target() {
        assert!(parse_channel_url("udp://0.0.0.0:9001").is_none());
    }

    #[tokio::test]
    async fn test_dc_to_udp_forwarding() {
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let receiver_port = receiver.local_addr().unwrap().port();

        let url = format!("udp://0.0.0.0:0?host=127.0.0.1&port={}", receiver_port);

        let (dc_recv_tx, dc_recv_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (dc_send_tx, _dc_send_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        let ct = CancellationToken::new();
        let handle = spawn_channel(url, dc_recv_rx, dc_send_tx, ct.clone())
            .await
            .unwrap();

        let msg = b"hello from datachannel";
        dc_recv_tx.send(msg.to_vec()).unwrap();

        let mut buf = vec![0u8; 1024];
        let (n, _) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            receiver.recv_from(&mut buf),
        )
        .await
        .expect("timeout")
        .unwrap();

        assert_eq!(&buf[..n], msg);

        ct.cancel();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_udp_to_dc_forwarding() {
        let listen_port = portpicker::pick_unused_port().unwrap();
        let url = format!("udp://0.0.0.0:{}?host=127.0.0.1&port=19999", listen_port);

        let (_dc_recv_tx, dc_recv_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (dc_send_tx, mut dc_send_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        let ct = CancellationToken::new();
        let handle = spawn_channel(url, dc_recv_rx, dc_send_tx, ct.clone())
            .await
            .unwrap();

        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let msg = b"hello from udp";
        sender
            .send_to(msg, format!("127.0.0.1:{}", listen_port))
            .await
            .unwrap();

        let received = tokio::time::timeout(std::time::Duration::from_secs(2), dc_send_rx.recv())
            .await
            .expect("timeout")
            .unwrap();

        assert_eq!(received, msg);

        ct.cancel();
        handle.await.unwrap();
    }

    /// The bridge must release its listen port when the session token is
    /// cancelled, so an in-process reconnect can rebind the same port.
    #[tokio::test]
    async fn test_cancel_releases_listen_port() {
        let listen_port = portpicker::pick_unused_port().unwrap();
        let url = format!("udp://0.0.0.0:{}?host=127.0.0.1&port=19999", listen_port);

        let (_dc_recv_tx, dc_recv_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (dc_send_tx, _dc_send_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        let ct = CancellationToken::new();
        let handle = spawn_channel(url, dc_recv_rx, dc_send_tx, ct.clone())
            .await
            .unwrap();

        assert!(
            UdpSocket::bind(format!("0.0.0.0:{}", listen_port))
                .await
                .is_err()
        );
        ct.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("channel task did not stop after cancel")
            .unwrap();
        assert!(
            UdpSocket::bind(format!("0.0.0.0:{}", listen_port))
                .await
                .is_ok(),
            "listen port still occupied after channel stopped"
        );
    }
}
