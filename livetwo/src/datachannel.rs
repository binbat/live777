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

/// Default DataChannel label used to join the matching cable on the server,
/// on both the WHEP and the WHIP leg. Clients can override it per run.
pub const DEFAULT_CABLE_LABEL: &str = "-";

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

/// Routes the single outbound queue (UDP -> DataChannel) to the currently
/// active channel. live777 closes a client-created channel whose label
/// matches a server-created cable; the accepted server-created channel then
/// takes over the queue.
///
/// Channels are tracked in the order they opened; the most recently opened
/// is the active target. When it closes (or a send to it fails) the router
/// falls back to the next most recent still-open channel, so the outbound
/// queue never gets stranded pointing at a dead channel just because a
/// replacement channel opened earlier than the one being torn down.
#[derive(Clone, Default)]
pub struct CableRouter {
    active: Arc<std::sync::Mutex<Vec<Arc<dyn DataChannel>>>>,
}

impl CableRouter {
    pub fn new() -> Self {
        Self::default()
    }

    fn activate(&self, dc: &Arc<dyn DataChannel>) {
        let mut guard = self.active.lock().expect("cable router lock poisoned");
        if !guard.iter().any(|c| Arc::ptr_eq(c, dc)) {
            guard.push(dc.clone());
        }
    }

    fn deactivate(&self, dc: &Arc<dyn DataChannel>) {
        self.active
            .lock()
            .expect("cable router lock poisoned")
            .retain(|c| !Arc::ptr_eq(c, dc));
    }

    fn current(&self) -> Option<Arc<dyn DataChannel>> {
        self.active
            .lock()
            .expect("cable router lock poisoned")
            .last()
            .cloned()
    }
}

/// Poll a DataChannel's inbound side: messages go to `dc_recv_tx`. While the
/// channel is open it registers with `router`, so the outbound queue drained
/// by [`run_cable_send_dispatch`] targets the live channel.
/// `owner` tags the log lines (e.g. "whepfrom" / "whipinto").
pub fn run_data_channel_loop(
    dc: Arc<dyn DataChannel>,
    dc_recv_tx: mpsc::UnboundedSender<Vec<u8>>,
    router: CableRouter,
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

        router.activate(&dc);
        loop {
            match dc.poll().await {
                Some(DataChannelEvent::OnMessage(msg)) => {
                    if dc_recv_tx.send(msg.data.to_vec()).is_err() {
                        debug!("{}: DataChannel recv channel closed", owner);
                        break;
                    }
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
            }
        }
        router.deactivate(&dc);
    });
}

/// Drain the outbound queue (UDP -> DataChannel) into the router's active
/// channel. Messages are dropped — and logged — while no channel is active
/// (e.g. the gap between the server closing the client-created channel and
/// the accepted cable channel opening).
pub fn run_cable_send_dispatch(
    mut dc_send_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    router: CableRouter,
    owner: &'static str,
) {
    tokio::spawn(async move {
        while let Some(data) = dc_send_rx.recv().await {
            match router.current() {
                Some(dc) => {
                    if let Err(e) = dc.send(bytes::BytesMut::from(&data[..])).await {
                        warn!("{}: DataChannel send failed: {}", owner, e);
                        router.deactivate(&dc);
                    }
                }
                None => {
                    warn!("{}: no active cable channel; dropping message", owner);
                }
            }
        }
        info!("{}: DataChannel send channel closed", owner);
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
    use webrtc::data_channel::{RTCDataChannelId, RTCDataChannelState};

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

    /// Minimal DataChannel stub for exercising `CableRouter` routing without
    /// a real peer connection.
    struct MockDc;

    #[async_trait::async_trait]
    impl DataChannel for MockDc {
        async fn label(&self) -> webrtc::error::Result<String> {
            Ok("mock".to_string())
        }
        async fn ordered(&self) -> webrtc::error::Result<bool> {
            Ok(true)
        }
        async fn max_packet_life_time(&self) -> webrtc::error::Result<Option<u16>> {
            Ok(None)
        }
        async fn max_retransmits(&self) -> webrtc::error::Result<Option<u16>> {
            Ok(None)
        }
        async fn protocol(&self) -> webrtc::error::Result<String> {
            Ok(String::new())
        }
        async fn negotiated(&self) -> webrtc::error::Result<bool> {
            Ok(false)
        }
        fn id(&self) -> RTCDataChannelId {
            0
        }
        async fn ready_state(&self) -> webrtc::error::Result<RTCDataChannelState> {
            Ok(RTCDataChannelState::Open)
        }
        async fn buffered_amount_high_threshold(&self) -> webrtc::error::Result<u32> {
            Ok(u32::MAX)
        }
        async fn set_buffered_amount_high_threshold(
            &self,
            _threshold: u32,
        ) -> webrtc::error::Result<()> {
            Ok(())
        }
        async fn buffered_amount_low_threshold(&self) -> webrtc::error::Result<u32> {
            Ok(0)
        }
        async fn set_buffered_amount_low_threshold(
            &self,
            _threshold: u32,
        ) -> webrtc::error::Result<()> {
            Ok(())
        }
        async fn send(&self, _data: bytes::BytesMut) -> webrtc::error::Result<()> {
            Ok(())
        }
        async fn send_text(&self, _text: &str) -> webrtc::error::Result<()> {
            Ok(())
        }
        async fn poll(&self) -> Option<DataChannelEvent> {
            None
        }
        async fn close(&self) -> webrtc::error::Result<()> {
            Ok(())
        }
    }

    /// Deactivating the most recently opened channel must fall back to the
    /// next most recent still-open one — the race where the server-created
    /// cable channel activated before the client's own channel, and the
    /// client channel's later close used to strand the outbound queue.
    #[test]
    fn test_cable_router_falls_back_after_deactivate() {
        let router = CableRouter::new();
        let older: Arc<dyn DataChannel> = Arc::new(MockDc);
        let newer: Arc<dyn DataChannel> = Arc::new(MockDc);

        router.activate(&older);
        assert!(Arc::ptr_eq(&router.current().unwrap(), &older));

        router.activate(&newer);
        assert!(Arc::ptr_eq(&router.current().unwrap(), &newer));

        router.deactivate(&newer);
        assert!(Arc::ptr_eq(&router.current().unwrap(), &older));

        router.deactivate(&older);
        assert!(router.current().is_none());
    }

    /// Deactivating an older channel must not disturb the newer active one.
    #[test]
    fn test_cable_router_deactivate_is_targeted() {
        let router = CableRouter::new();
        let older: Arc<dyn DataChannel> = Arc::new(MockDc);
        let newer: Arc<dyn DataChannel> = Arc::new(MockDc);
        router.activate(&older);
        router.activate(&newer);
        router.deactivate(&older);
        assert!(Arc::ptr_eq(&router.current().unwrap(), &newer));
    }

    /// Re-activating an already tracked channel must not duplicate it, so a
    /// single deactivate fully removes it.
    #[test]
    fn test_cable_router_activate_is_idempotent() {
        let router = CableRouter::new();
        let a: Arc<dyn DataChannel> = Arc::new(MockDc);
        let b: Arc<dyn DataChannel> = Arc::new(MockDc);
        router.activate(&a);
        router.activate(&a);
        router.activate(&b);
        router.deactivate(&b);
        assert!(Arc::ptr_eq(&router.current().unwrap(), &a));
        router.deactivate(&a);
        assert!(router.current().is_none());
    }
}
