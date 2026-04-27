/// DataChannel <-> UDP bidirectional forwarding for whepfrom
///
/// Symmetric to liveion's channel.rs, but on the WHEP subscriber side.
/// Messages received from liveion via DataChannel are forwarded to UDP,
/// and messages received from UDP are sent back to liveion via DataChannel.
///
/// URL format: udp://<listen_host>:<listen_port>?host=<target_host>&port=<target_port>
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Buffer size for incoming UDP packets.
/// - WebRTC DataChannel SCTP max: 1024 * 64 = 65536 bytes
/// - RFC 8831 WebRTC DataChannel max: < 1024 * 16 = 16384 bytes
/// - IP UDP MTU: 1500 bytes
/// - Recommended single payload: < 1200 bytes
const UDP_BUF_SIZE: usize = 1024;

/// Parse UDP URL into (listen_host, listen_port, target_host, target_port)
pub fn parse_channel_url(url: &str) -> Option<(String, u16, String, u16)> {
    let s = url.strip_prefix("udp://")?;
    let (host_port, query) = s.split_once('?')?;
    let (listen_host_raw, listen_port_str) = host_port.rsplit_once(':')?;
    let listen_port: u16 = listen_port_str.parse().ok()?;
    let listen_host_inner = listen_host_raw.trim_matches(|c| c == '[' || c == ']');
    let listen_host = if listen_host_inner.contains(':') {
        format!("[{}]", listen_host_inner)
    } else {
        listen_host_inner.to_string()
    };

    let mut target_host = String::new();
    let mut target_port: u16 = 0;
    for param in query.split('&') {
        if let Some(v) = param.strip_prefix("host=") {
            target_host = if v.parse::<std::net::Ipv6Addr>().is_ok() {
                format!("[{}]", v)
            } else {
                v.to_string()
            };
        } else if let Some(v) = param.strip_prefix("port=") {
            target_port = v.parse().ok()?;
        }
    }
    if target_host.is_empty() || target_port == 0 {
        return None;
    }
    Some((listen_host, listen_port, target_host, target_port))
}

/// Spawn bidirectional DataChannel <-> UDP forwarding.
/// `dc_recv`: messages received from liveion DataChannel
/// `dc_send`: sender to write messages back to liveion DataChannel
pub async fn spawn_channel(
    url: String,
    mut dc_recv: mpsc::UnboundedReceiver<Vec<u8>>,
    dc_send: mpsc::UnboundedSender<Vec<u8>>,
) -> anyhow::Result<()> {
    let (listen_host, listen_port, target_host, target_port) =
        parse_channel_url(&url).ok_or_else(|| anyhow::anyhow!("invalid channel url: {}", url))?;

    let target = format!("{}:{}", target_host, target_port);
    let listen = format!("{}:{}", listen_host, listen_port);

    let socket = match UdpSocket::bind(&listen).await {
        Ok(s) => {
            info!("whepfrom channel: listen={} target={}", listen, target);
            s
        }
        Err(e) => {
            warn!("whepfrom channel: bind {} failed: {}", listen, e);
            return Err(anyhow::anyhow!("bind {} failed: {}", listen, e));
        }
    };

    tokio::spawn(async move {
        let mut buf = vec![0u8; UDP_BUF_SIZE];
        loop {
            tokio::select! {
                // DataChannel -> UDP (messages from liveion WHIP group)
                msg = dc_recv.recv() => {
                    match msg {
                        Some(data) => {
                            if let Err(e) = socket.send_to(&data, &target).await {
                                warn!("whepfrom channel: send to {} failed: {}", target, e);
                            } else {
                                debug!("whepfrom channel: DC->UDP {} bytes -> {}", data.len(), target);
                            }
                        }
                        None => {
                            info!("whepfrom channel: DC recv closed");
                            break;
                        }
                    }
                },
                // UDP -> DataChannel (messages to liveion WHIP group)
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((n, addr)) => {
                            let data = buf[..n].to_vec();
                            debug!("whepfrom channel: UDP->DC {} bytes from {}", n, addr);
                            if dc_send.send(data).is_err() {
                                info!("whepfrom channel: DC send closed");
                                break;
                            }
                        }
                        Err(e) => {
                            warn!("whepfrom channel: recv_from failed: {}", e);
                        }
                    }
                },
            }
        }
    });

    Ok(())
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

        spawn_channel(url, dc_recv_rx, dc_send_tx).await.unwrap();

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
    }

    #[tokio::test]
    async fn test_udp_to_dc_forwarding() {
        let listen_port = portpicker::pick_unused_port().unwrap();
        let url = format!("udp://0.0.0.0:{}?host=127.0.0.1&port=19999", listen_port);

        let (_dc_recv_tx, dc_recv_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (dc_send_tx, mut dc_send_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        spawn_channel(url, dc_recv_rx, dc_send_tx).await.unwrap();

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
    }
}
