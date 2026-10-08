/// Integration test: liveion UDP channel <-> whepfrom DataChannel <-> UDP
///
/// This test verifies end-to-end DataChannel <-> UDP forwarding without a WHIP
/// publisher. liveion's own UDP channel (stream.<name>.channel) is initialized at
/// stream creation time, and whepfrom bridges its DataChannel to UDP via
/// the --channel flag.
///
/// Topology:
///
///   UDP sender --> liveion UDP listen (8702)
///       |
///   liveion subscribe broadcast --> all WHEP subscribers' DataChannels
///       |
///   whepfrom DataChannel --> whepfrom UDP target (8701)
///
/// And the reverse:
///
///   UDP sender --> whepfrom UDP listen (8700)
///       |
///   whepfrom DataChannel --> liveion publish broadcast
///       |
///   liveion UDP channel --> liveion UDP target (8703)
#[cfg(any(feature = "source", feature = "source-all"))]
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[cfg(any(feature = "source", feature = "source-all"))]
use tokio::net::{TcpListener, UdpSocket};
#[cfg(any(feature = "source", feature = "source-all"))]
use tokio_util::sync::CancellationToken;

#[cfg(any(feature = "source", feature = "source-all"))]
mod common;
#[cfg(any(feature = "source", feature = "source-all"))]
use common::shutdown_signal;

/// Metrics registration is process-global and panics on a second call;
/// nextest isolates test processes, but plain `cargo test` shares one, so
/// every test that needs the registry must go through this single `Once`.
#[cfg(any(feature = "source", feature = "source-all"))]
static METRICS_REGISTER: std::sync::Once = std::sync::Once::new();

#[cfg(any(feature = "source", feature = "source-all"))]
fn init_tracing() {
    use std::sync::Once;
    static TRACING_INIT: Once = Once::new();
    TRACING_INIT.call_once(|| {
        let filter = std::env::var("RUST_LOG")
            .unwrap_or_else(|_| "live777=info,liveion=info,livetwo=info,libwish=info".to_string());
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

#[cfg(any(feature = "source", feature = "source-all"))]
async fn wait_for_session_connected(addr: &SocketAddr, stream_id: &str) -> bool {
    for _ in 0..200 {
        let body = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap()
            .json::<Vec<api::response::Stream>>()
            .await
            .unwrap_or_default();

        if let Some(stream) = body.into_iter().find(|s| s.id == stream_id)
            && !stream.subscribe.sessions.is_empty()
            && stream.subscribe.sessions[0].state
                == api::response::RTCPeerConnectionState::Connected
        {
            return true;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    false
}

#[cfg(any(feature = "source", feature = "source-all"))]
#[tokio::test]
async fn test_whepfrom_datachannel_udp_forwarding() {
    METRICS_REGISTER.call_once(liveion::metrics_register);
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let stream_id = "test-dc-channel";

    // ── 1. Static ports ────────────────────────────────────────────────────────
    let whepfrom_ch_listen: u16 = 8700;
    let whepfrom_ch_target: u16 = 8701;
    let liveion_ch_listen: u16 = 8702;
    let liveion_ch_target: u16 = 8703;

    // ── 2. Start liveion with UDP channel config ────────────────────────────────
    let mut cfg = liveion::config::Config::default();
    cfg.stream.streams.insert(
        stream_id.to_string(),
        liveion::config::StreamEntry {
            sources: vec![],
            strategy: None,
            hooks: Default::default(),
            channel: Some(liveion::config::ChannelConfig {
                listen: format!("0.0.0.0:{liveion_ch_listen}").parse().unwrap(),
                target: format!("127.0.0.1:{liveion_ch_target}").parse().unwrap(),
            }),
            ..Default::default()
        },
    );
    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    // The stream is provisioned from the config above, so it already exists
    // (a POST create would be a 409 conflict) and its UDP channel is up.

    // ── 4. Start whepfrom with --channel ───────────────────────────────────────
    let ct = CancellationToken::new();
    let whep_channel_url =
        format!("udp://0.0.0.0:{whepfrom_ch_listen}?host=127.0.0.1&port={whepfrom_ch_target}");
    let handle_whepfrom = tokio::spawn(livetwo::whep::from(
        ct.clone(),
        format!("rtp://{ip}"),
        format!("http://{addr}{}", api::path::whep(stream_id)),
        None,
        None,
        None,
        Some(whep_channel_url),
        Vec::new(),
    ));

    assert!(
        wait_for_session_connected(&addr, stream_id).await,
        "WHEP subscriber (whepfrom) failed to connect"
    );

    // Bind receivers before sending so no packets are dropped
    let whepfrom_target = UdpSocket::bind(format!("127.0.0.1:{whepfrom_ch_target}"))
        .await
        .unwrap();
    let liveion_target = UdpSocket::bind(format!("127.0.0.1:{liveion_ch_target}"))
        .await
        .unwrap();

    // Give DataChannel time to open and detach
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    let udp_sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut buf = vec![0u8; 256];

    // ── 5. Test: UDP → liveion listen → DC → whepfrom target ───────────────────
    let msg_liveion_to_whepfrom = b"liveion->dc->whepfrom";
    udp_sender
        .send_to(
            msg_liveion_to_whepfrom,
            format!("127.0.0.1:{liveion_ch_listen}"),
        )
        .await
        .unwrap();

    let (n, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        whepfrom_target.recv_from(&mut buf),
    )
    .await
    .expect("timeout waiting for message at whepfrom target")
    .unwrap();
    assert_eq!(
        &buf[..n],
        msg_liveion_to_whepfrom,
        "unexpected data at whepfrom target"
    );

    // ── 6. Test: UDP → whepfrom listen → DC → liveion target ───────────────────
    let msg_whepfrom_to_liveion = b"whepfrom->dc->liveion";
    udp_sender
        .send_to(
            msg_whepfrom_to_liveion,
            format!("127.0.0.1:{whepfrom_ch_listen}"),
        )
        .await
        .unwrap();

    let (n, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        liveion_target.recv_from(&mut buf),
    )
    .await
    .expect("timeout waiting for message at liveion target")
    .unwrap();
    assert_eq!(
        &buf[..n],
        msg_whepfrom_to_liveion,
        "unexpected data at liveion target"
    );

    // ── 6b. The round trip is visible in the Prometheus metrics ─────────────
    // Both directions were delivered exactly once: `in` counts the message
    // received from whepfrom's channel, `out` the one written to it.
    let metrics = reqwest::get(format!("http://{addr}{}", api::path::METRICS))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for (direction, msg) in [
        ("in", msg_whepfrom_to_liveion.len() as u64),
        ("out", msg_liveion_to_whepfrom.len() as u64),
    ] {
        // The exposition lists labels alphabetically: direction before stream.
        let series = format!(
            r#"live777_stream_datachannel_bytes_total{{direction="{direction}",stream="{stream_id}"}}"#
        );
        let value = metrics
            .lines()
            .filter(|line| !line.starts_with('#'))
            .find_map(|line| match line.split_once(' ') {
                Some((key, value)) if key == series => value.parse::<u64>().ok(),
                _ => None,
            })
            .unwrap_or(0);
        assert_eq!(
            value, msg,
            "per-stream datachannel bytes mismatch for direction={direction}:\n{metrics}"
        );
    }

    // ── 7. Teardown ─────────────────────────────────────────────────────────────
    ct.cancel();
    let result_whepfrom = handle_whepfrom.await.unwrap();
    assert!(result_whepfrom.is_ok());
}

/// Send one message each way across the liveion <-> whepfrom DataChannel
/// bridge and assert both arrive.
#[cfg(any(feature = "source", feature = "source-all"))]
async fn assert_channel_roundtrip(
    label: &str,
    liveion_ch_listen: u16,
    whepfrom_ch_listen: u16,
    whepfrom_target: &UdpSocket,
    liveion_target: &UdpSocket,
) {
    let udp_sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut buf = vec![0u8; 256];

    let msg = format!("liveion->dc->whepfrom ({label})");
    udp_sender
        .send_to(msg.as_bytes(), format!("127.0.0.1:{liveion_ch_listen}"))
        .await
        .unwrap();
    let (n, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        whepfrom_target.recv_from(&mut buf),
    )
    .await
    .unwrap_or_else(|_| panic!("{label}: timeout waiting for message at whepfrom target"))
    .unwrap();
    assert_eq!(
        &buf[..n],
        msg.as_bytes(),
        "{label}: bad data at whepfrom target"
    );

    let msg = format!("whepfrom->dc->liveion ({label})");
    udp_sender
        .send_to(msg.as_bytes(), format!("127.0.0.1:{whepfrom_ch_listen}"))
        .await
        .unwrap();
    let (n, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        liveion_target.recv_from(&mut buf),
    )
    .await
    .unwrap_or_else(|_| panic!("{label}: timeout waiting for message at liveion target"))
    .unwrap();
    assert_eq!(
        &buf[..n],
        msg.as_bytes(),
        "{label}: bad data at liveion target"
    );
}

/// Wait for a client task to wind down after its peer was closed
/// server-side; cancel it if the close notification got lost.
#[cfg(any(feature = "source", feature = "source-all"))]
async fn wait_or_cancel<T>(ct: &CancellationToken, handle: tokio::task::JoinHandle<T>) {
    tokio::pin!(handle);
    if tokio::time::timeout(std::time::Duration::from_secs(10), &mut handle)
        .await
        .is_err()
    {
        ct.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), &mut handle).await;
    }
}

/// Regression test for a dead-UDP-bridge after stream reset. Deleting the
/// publisher session resets this provisioned stream (teardown -> standby),
/// which tears down every session and re-initializes the UDP channel bridge
/// on the *same* listen port. Before the bridge was tied to the forward
/// lifecycle, leaked data-channel tasks kept the stream bus open, the old
/// bridge never released the port, the rebind failed with EADDRINUSE (warn
/// only), and the bridge stayed dead until the process restarted — the
/// "datachannel reconnect conflict" reported by users.
#[cfg(any(feature = "source", feature = "source-all"))]
#[tokio::test]
async fn test_udp_channel_survives_stream_reset() {
    init_tracing();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let stream_id = "test-dc-channel-reset";

    // Distinct from the other test in this binary (8700-8703) and from
    // datachannel_loadtest (8700-8711).
    let whepfrom_ch_listen: u16 = 8720;
    let whepfrom_ch_target: u16 = 8721;
    let liveion_ch_listen: u16 = 8722;
    let liveion_ch_target: u16 = 8723;

    // ── 1. liveion with a provisioned stream + UDP channel ────────────────
    let mut cfg = liveion::config::Config::default();
    cfg.stream.streams.insert(
        stream_id.to_string(),
        liveion::config::StreamEntry {
            sources: vec![],
            strategy: None,
            hooks: Default::default(),
            channel: Some(liveion::config::ChannelConfig {
                listen: format!("0.0.0.0:{liveion_ch_listen}").parse().unwrap(),
                target: format!("127.0.0.1:{liveion_ch_target}").parse().unwrap(),
            }),
            ..Default::default()
        },
    );
    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    // ── 2. Publisher via an SDP-file input (same path as `whipinto -i`) ───
    // No RTP is ever sent; the WHIP session connects over ICE/DTLS alone.
    let rtp_listen_port: u16 = 8724;
    let sdp_path = std::env::temp_dir().join(format!(
        "live777-test-{stream_id}-{}.sdp",
        std::process::id()
    ));
    std::fs::write(
        &sdp_path,
        format!(
            "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=No Name\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=video {rtp_listen_port} RTP/AVP 96\r\na=rtpmap:96 VP8/90000\r\n"
        ),
    )
    .unwrap();

    let ct_pub = CancellationToken::new();
    let handle_whip = tokio::spawn(livetwo::whip::into(
        ct_pub.clone(),
        sdp_path.to_string_lossy().to_string(),
        format!("http://{addr}{}", api::path::whip(stream_id)),
        None,
        None,
        None,
        Vec::new(),
    ));

    // Wait for the publish session and learn its id.
    let mut publish_session_id = None;
    for _ in 0..200 {
        let body = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap()
            .json::<Vec<api::response::Stream>>()
            .await
            .unwrap_or_default();
        if let Some(stream) = body.into_iter().find(|s| s.id == stream_id)
            && let Some(session) = stream.publish.sessions.first()
            && session.state == api::response::RTCPeerConnectionState::Connected
        {
            publish_session_id = Some(session.id.clone());
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    let publish_session_id =
        publish_session_id.expect("WHIP publish session did not reach Connected");

    // ── 3. First subscriber with a data channel ────────────────────────────
    let ct1 = CancellationToken::new();
    let whep_channel_url =
        format!("udp://0.0.0.0:{whepfrom_ch_listen}?host=127.0.0.1&port={whepfrom_ch_target}");
    let handle_whepfrom1 = tokio::spawn(livetwo::whep::from(
        ct1.clone(),
        format!("rtp://{ip}"),
        format!("http://{addr}{}", api::path::whep(stream_id)),
        None,
        None,
        None,
        Some(whep_channel_url.clone()),
        Vec::new(),
    ));
    assert!(
        wait_for_session_connected(&addr, stream_id).await,
        "first WHEP subscriber failed to connect"
    );

    let whepfrom_target = UdpSocket::bind(format!("127.0.0.1:{whepfrom_ch_target}"))
        .await
        .unwrap();
    let liveion_target = UdpSocket::bind(format!("127.0.0.1:{liveion_ch_target}"))
        .await
        .unwrap();
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    assert_channel_roundtrip(
        "before reset",
        liveion_ch_listen,
        whepfrom_ch_listen,
        &whepfrom_target,
        &liveion_target,
    )
    .await;

    // ── 4. Delete the publish session -> provisioned stream reset ─────────
    let res = reqwest::Client::new()
        .delete(format!(
            "http://{addr}{}",
            api::path::session(stream_id, &publish_session_id)
        ))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "publish session DELETE failed: {}",
        res.status()
    );

    // The reset closed every session. The subscriber notices the peer close
    // and winds down on its own; the publisher is parked on its RTP input
    // socket with no packets arriving, so cancel it directly.
    ct_pub.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), handle_whip).await;
    wait_or_cancel(&ct1, handle_whepfrom1).await;

    // Drop stale datagrams from before the reset.
    let mut scratch = vec![0u8; 256];
    while whepfrom_target.try_recv_from(&mut scratch).is_ok() {}
    while liveion_target.try_recv_from(&mut scratch).is_ok() {}

    // ── 5. Second subscriber: the bridge must be live again ────────────────
    let ct2 = CancellationToken::new();
    let handle_whepfrom2 = tokio::spawn(livetwo::whep::from(
        ct2.clone(),
        format!("rtp://{ip}"),
        format!("http://{addr}{}", api::path::whep(stream_id)),
        None,
        None,
        None,
        Some(whep_channel_url),
        Vec::new(),
    ));
    assert!(
        wait_for_session_connected(&addr, stream_id).await,
        "second WHEP subscriber failed to connect after stream reset"
    );
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    assert_channel_roundtrip(
        "after reset",
        liveion_ch_listen,
        whepfrom_ch_listen,
        &whepfrom_target,
        &liveion_target,
    )
    .await;

    // ── 6. Teardown ─────────────────────────────────────────────────────────
    ct2.cancel();
    let _ = handle_whepfrom2.await;
    let _ = std::fs::remove_file(&sdp_path);
}

/// Integration test: whipinto DataChannel <-> liveion <-> whepfrom DataChannel
///
/// The publisher-side counterpart of test_whepfrom_datachannel_udp_forwarding:
/// whipinto joins the stream's channel group by creating the "control"
/// DataChannel on its WHIP publish session (--channel). liveion's publish peer
/// bridges it into the stream buses, so the publisher's and a subscriber's
/// channels exchange messages both ways:
///
///   UDP sender --> whipinto UDP listen (8730)
///       |
///   whipinto DataChannel --> liveion subscribe broadcast
///       |
///   whepfrom DataChannel --> whepfrom UDP target (8733)
///
/// And the reverse:
///
///   UDP sender --> whepfrom UDP listen (8732)
///       |
///   whepfrom DataChannel --> liveion publish broadcast
///       |
///   whipinto DataChannel --> whipinto UDP target (8731)
#[cfg(any(feature = "source", feature = "source-all"))]
#[tokio::test]
async fn test_whipinto_datachannel_udp_forwarding() {
    init_tracing();
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let stream_id = "test-dc-channel-whip";

    // Distinct from the other tests in this binary (8700-8703, 8720-8724) and
    // from datachannel_loadtest (8700-8711).
    let whipinto_ch_listen: u16 = 8730;
    let whipinto_ch_target: u16 = 8731;
    let whepfrom_ch_listen: u16 = 8732;
    let whepfrom_ch_target: u16 = 8733;

    // ── 1. liveion with a provisioned stream (no server-side UDP channel
    // needed: publisher and subscriber channels talk through the stream buses)
    let mut cfg = liveion::config::Config::default();
    cfg.stream.streams.insert(
        stream_id.to_string(),
        liveion::config::StreamEntry {
            sources: vec![],
            strategy: None,
            hooks: Default::default(),
            channel: None,
            ..Default::default()
        },
    );
    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    // ── 2. Publisher with a data channel (SDP-file input, no RTP is sent) ──
    let rtp_listen_port: u16 = 8734;
    let sdp_path = std::env::temp_dir().join(format!(
        "live777-test-{stream_id}-{}.sdp",
        std::process::id()
    ));
    std::fs::write(
        &sdp_path,
        format!(
            "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=No Name\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=video {rtp_listen_port} RTP/AVP 96\r\na=rtpmap:96 VP8/90000\r\n"
        ),
    )
    .unwrap();

    let ct_pub = CancellationToken::new();
    let whipinto_channel_url =
        format!("udp://0.0.0.0:{whipinto_ch_listen}?host=127.0.0.1&port={whipinto_ch_target}");
    let handle_whip = tokio::spawn(livetwo::whip::into(
        ct_pub.clone(),
        sdp_path.to_string_lossy().to_string(),
        format!("http://{addr}{}", api::path::whip(stream_id)),
        None,
        None,
        Some(whipinto_channel_url),
        Vec::new(),
    ));

    // Wait for the publish session to reach Connected.
    let mut publish_connected = false;
    for _ in 0..200 {
        let body = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap()
            .json::<Vec<api::response::Stream>>()
            .await
            .unwrap_or_default();
        if let Some(stream) = body.into_iter().find(|s| s.id == stream_id)
            && let Some(session) = stream.publish.sessions.first()
            && session.state == api::response::RTCPeerConnectionState::Connected
        {
            publish_connected = true;
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    assert!(
        publish_connected,
        "WHIP publish session did not reach Connected"
    );

    // ── 3. Subscriber with a data channel ───────────────────────────────────
    let ct_sub = CancellationToken::new();
    let whep_channel_url =
        format!("udp://0.0.0.0:{whepfrom_ch_listen}?host=127.0.0.1&port={whepfrom_ch_target}");
    let handle_whepfrom = tokio::spawn(livetwo::whep::from(
        ct_sub.clone(),
        format!("rtp://{ip}"),
        format!("http://{addr}{}", api::path::whep(stream_id)),
        None,
        None,
        None,
        Some(whep_channel_url),
        Vec::new(),
    ));
    assert!(
        wait_for_session_connected(&addr, stream_id).await,
        "WHEP subscriber (whepfrom) failed to connect"
    );

    // Bind receivers before sending so no packets are dropped
    let whipinto_target = UdpSocket::bind(format!("127.0.0.1:{whipinto_ch_target}"))
        .await
        .unwrap();
    let whepfrom_target = UdpSocket::bind(format!("127.0.0.1:{whepfrom_ch_target}"))
        .await
        .unwrap();

    // Give both DataChannels time to open
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    let udp_sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut buf = vec![0u8; 256];

    // ── 4. Test: UDP → whipinto listen → publisher DC → whepfrom target ──────
    let msg_whipinto_to_whepfrom = b"whipinto->dc->whepfrom";
    udp_sender
        .send_to(
            msg_whipinto_to_whepfrom,
            format!("127.0.0.1:{whipinto_ch_listen}"),
        )
        .await
        .unwrap();

    let (n, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        whepfrom_target.recv_from(&mut buf),
    )
    .await
    .expect("timeout waiting for message at whepfrom target")
    .unwrap();
    assert_eq!(
        &buf[..n],
        msg_whipinto_to_whepfrom,
        "unexpected data at whepfrom target"
    );

    // ── 5. Test: UDP → whepfrom listen → subscriber DC → whipinto target ─────
    let msg_whepfrom_to_whipinto = b"whepfrom->dc->whipinto";
    udp_sender
        .send_to(
            msg_whepfrom_to_whipinto,
            format!("127.0.0.1:{whepfrom_ch_listen}"),
        )
        .await
        .unwrap();

    let (n, _) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        whipinto_target.recv_from(&mut buf),
    )
    .await
    .expect("timeout waiting for message at whipinto target")
    .unwrap();
    assert_eq!(
        &buf[..n],
        msg_whepfrom_to_whipinto,
        "unexpected data at whipinto target"
    );

    // ── 6. Teardown ───────────────────────────────────────────────────────────
    ct_pub.cancel();
    ct_sub.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), handle_whip).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), handle_whepfrom).await;
    let _ = std::fs::remove_file(&sdp_path);
}
