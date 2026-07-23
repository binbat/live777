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
#[cfg(feature = "source")]
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[cfg(feature = "source")]
use tokio::net::{TcpListener, UdpSocket};
#[cfg(feature = "source")]
use tokio_util::sync::CancellationToken;

#[cfg(feature = "source")]
mod common;
#[cfg(feature = "source")]
use common::shutdown_signal;

#[cfg(feature = "source")]
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

#[cfg(feature = "source")]
#[tokio::test]
async fn test_whepfrom_datachannel_udp_forwarding() {
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

    // ── 7. Teardown ─────────────────────────────────────────────────────────────
    ct.cancel();
    let result_whepfrom = handle_whepfrom.await.unwrap();
    assert!(result_whepfrom.is_ok());
}
