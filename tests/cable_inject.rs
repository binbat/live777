//! Integration tests for the HTTP cable message inject/receive API
//! (`/api/streams/{stream}/cables/{label}/messages`).
//!
//! Covered here: the error matrix (404 stream, 404 session, 400 body, 401
//! auth) for both methods, a broadcast inject with no listeners
//! (`receivers: 0`), and three end-to-end paths — broadcast inject onto a
//! cable whose DC target bridge exports to UDP, a targeted direct send to a
//! whepfrom subscriber's channel bridged to UDP, and SSE receive of a
//! DC target's inbound UDP inject (`to_publisher` direction).

use std::net::SocketAddr;

use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

mod common;
use common::{start_liveion, start_liveion_with};

/// Poll the stream list until the stream has a Connected subscribe session,
/// returning its id (the targeted injection needs it as `?session=`).
async fn wait_for_subscribe_session(addr: &SocketAddr, stream_id: &str) -> Option<String> {
    for _ in 0..200 {
        let body = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap()
            .json::<Vec<api::response::Stream>>()
            .await
            .unwrap_or_default();
        if let Some(stream) = body.into_iter().find(|s| s.id == stream_id)
            && let Some(session) = stream.subscribe.sessions.first()
            && session.state == api::response::RTCPeerConnectionState::Connected
        {
            return Some(session.id.clone());
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    None
}

async fn create_stream(client: &reqwest::Client, addr: &SocketAddr, stream_id: &str) {
    let resp = client
        .post(format!("http://{addr}{}", api::path::streams(stream_id)))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "stream create failed: {}",
        resp.status()
    );
}

async fn inject(
    client: &reqwest::Client,
    addr: &SocketAddr,
    stream_id: &str,
    label: &str,
    query: &str,
    data: &str,
) -> reqwest::Response {
    client
        .post(format!(
            "http://{addr}{}{query}",
            api::path::stream_cable_messages(stream_id, label)
        ))
        .json(&api::request::CableMessageInject {
            data: data.to_string(),
        })
        .send()
        .await
        .unwrap()
}

/// Accumulate body chunks and pop complete `data:` events ("\n\n"
/// terminated); keep-alive comment lines are ignored.
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
async fn next_sse_data(resp: &mut reqwest::Response, buffer: &mut String) -> String {
    loop {
        if let Some((event, rest)) = buffer.split_once("\n\n") {
            let event = event.to_string();
            *buffer = rest.to_string();
            let data: String = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|line| line.trim_start())
                .collect();
            if !data.is_empty() {
                return data;
            }
            continue;
        }
        let chunk = resp.chunk().await.unwrap().expect("SSE stream ended");
        buffer.push_str(&String::from_utf8_lossy(&chunk));
    }
}

#[tokio::test]
async fn test_cable_inject_stream_not_found() {
    let addr = start_liveion(None).await;
    let client = reqwest::Client::new();
    let resp = inject(&client, &addr, "ghost", "control", "", "x").await;
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_cable_inject_broadcast_no_listeners() {
    let addr = start_liveion(None).await;
    let client = reqwest::Client::new();
    create_stream(&client, &addr, "inject-empty").await;

    let resp = inject(&client, &addr, "inject-empty", "control", "", "hello").await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp
        .json::<api::response::CableMessageInjected>()
        .await
        .unwrap();
    // Only the bus's permanent drain receiver is attached.
    assert_eq!(body.receivers, 0);
}

#[tokio::test]
async fn test_cable_inject_requires_auth() {
    let addr = start_liveion(Some("cable-secret")).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "http://{addr}{}",
            api::path::streams("inject-auth")
        ))
        .bearer_auth("cable-secret")
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let resp = inject(&client, &addr, "inject-auth", "control", "", "hello").await;
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_cable_inject_target_session_not_found() {
    let addr = start_liveion(None).await;
    let client = reqwest::Client::new();
    create_stream(&client, &addr, "inject-bogus-session").await;

    let resp = inject(
        &client,
        &addr,
        "inject-bogus-session",
        "control",
        "?session=bogus",
        "hello",
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_cable_inject_invalid_body() {
    let addr = start_liveion(None).await;
    let client = reqwest::Client::new();
    create_stream(&client, &addr, "inject-bad-body").await;
    let url = format!(
        "http://{addr}{}",
        api::path::stream_cable_messages("inject-bad-body", "control")
    );

    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

    let resp = client
        .post(&url)
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

/// A DC target endpoint (subscriber seat) keyed to a cable label, export
/// only (no listen authority): the cable's `to_subscribers` bus is exported
/// to the `?host`/`?port` target.
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
fn dc_target_export(target: u16, label: &str) -> liveion::config::TargetConfig {
    liveion::config::TargetConfig {
        url: String::new(),
        cables: vec![liveion::config::CableEndpoint {
            label: label.to_string(),
            url: format!("udp://?host=127.0.0.1&port={target}"),
        }],
        ..Default::default()
    }
}

/// A DC target endpoint (subscriber seat) keyed to a cable label, inject
/// only (no `?host`/`?port` target): inbound UDP packets go onto the
/// cable's `to_publisher` bus.
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
fn dc_target_listen(listen: u16, label: &str) -> liveion::config::TargetConfig {
    liveion::config::TargetConfig {
        url: String::new(),
        cables: vec![liveion::config::CableEndpoint {
            label: label.to_string(),
            url: format!("udp://127.0.0.1:{listen}"),
        }],
        ..Default::default()
    }
}

/// Broadcast injection reaches the cable's DC target bridge: the injected
/// bytes land on the bridge's UDP target verbatim.
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
#[tokio::test]
async fn test_cable_inject_broadcast_reaches_dc_target() {
    let stream_id = "inject-dc-target";
    let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let target_port = receiver.local_addr().unwrap().port();

    let addr = start_liveion_with(None, |cfg| {
        cfg.stream.streams.insert(
            stream_id.to_string(),
            liveion::config::StreamEntry {
                cables: vec![liveion::config::CableConfig::new("control")],
                targets: vec![dc_target_export(target_port, "control")],
                ..Default::default()
            },
        );
    })
    .await;

    // Provisioning (and with it the bridge's bus subscription) completed
    // before the server started accepting requests, so the very first
    // inject is already delivered.
    let msg = "broadcast-inject";
    let client = reqwest::Client::new();
    let resp = inject(&client, &addr, stream_id, "control", "", msg).await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp
        .json::<api::response::CableMessageInjected>()
        .await
        .unwrap();
    // The DC target bridge is the only live receiver besides the drain.
    assert_eq!(body.receivers, 1);

    let mut buf = vec![0u8; 256];
    let (n, _) = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        receiver.recv_from(&mut buf),
    )
    .await
    .expect("timeout waiting for injected message at the DC target")
    .unwrap();
    assert_eq!(&buf[..n], msg.as_bytes());
}

/// Targeted injection bypasses the bus and sends directly onto one
/// session's channel: a whepfrom subscriber bridged to UDP receives the
/// message, and the response reports exactly one receiver.
#[tokio::test]
async fn test_cable_inject_targeted_delivery() {
    let stream_id = "inject-targeted";
    let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let target_port = receiver.local_addr().unwrap().port();

    let addr = start_liveion_with(None, |cfg| {
        cfg.stream.streams.insert(
            stream_id.to_string(),
            liveion::config::StreamEntry {
                cables: vec![liveion::config::CableConfig {
                    create: false,
                    ..liveion::config::CableConfig::new("control")
                }],
                ..Default::default()
            },
        );
    })
    .await;

    // whepfrom joins with its "control" channel bridged to UDP (listen side
    // ephemeral; only the export direction is exercised here).
    let ct = CancellationToken::new();
    let whep_channel_url = format!("udp://0.0.0.0:0?host=127.0.0.1&port={target_port}");
    let handle_whepfrom = tokio::spawn(livetwo::whep::from(
        ct.clone(),
        "rtp://127.0.0.1".to_string(),
        format!("http://{addr}{}", api::path::whep(stream_id)),
        None,
        None,
        None,
        Some(whep_channel_url),
        "control".to_string(),
        Vec::new(),
    ));

    let session_id = wait_for_subscribe_session(&addr, stream_id)
        .await
        .expect("WHEP subscriber (whepfrom) failed to connect");
    // Give the DataChannel time to open before the direct send.
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    let msg = "targeted-inject";
    let client = reqwest::Client::new();
    let resp = inject(
        &client,
        &addr,
        stream_id,
        "control",
        &format!("?session={session_id}"),
        msg,
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp
        .json::<api::response::CableMessageInjected>()
        .await
        .unwrap();
    assert_eq!(body.receivers, 1);

    let mut buf = vec![0u8; 256];
    let (n, _) = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        receiver.recv_from(&mut buf),
    )
    .await
    .expect("timeout waiting for injected message at the whepfrom target")
    .unwrap();
    assert_eq!(&buf[..n], msg.as_bytes());

    ct.cancel();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), handle_whepfrom).await;
}

#[tokio::test]
async fn test_cable_receive_stream_not_found() {
    let addr = start_liveion(None).await;
    let resp = reqwest::Client::new()
        .get(format!(
            "http://{addr}{}",
            api::path::stream_cable_messages("ghost", "control")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_cable_receive_requires_auth() {
    let addr = start_liveion(Some("cable-secret")).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "http://{addr}{}",
            api::path::streams("receive-auth")
        ))
        .bearer_auth("cable-secret")
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let resp = client
        .get(format!(
            "http://{addr}{}",
            api::path::stream_cable_messages("receive-auth", "control")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

/// The receive direction: a DC target's inbound UDP inject lands on the
/// cable's `to_publisher` bus and is delivered as an SSE event carrying the
/// exact bytes, base64-encoded.
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
#[tokio::test]
async fn test_cable_receive_sse_from_dc_target() {
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

    let stream_id = "receive-dc-target";
    // Probe a free port for the bridge's listen socket, then release it.
    let probe = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listen = probe.local_addr().unwrap();
    drop(probe);

    let addr = start_liveion_with(None, |cfg| {
        cfg.stream.streams.insert(
            stream_id.to_string(),
            liveion::config::StreamEntry {
                cables: vec![liveion::config::CableConfig::new("control")],
                targets: vec![dc_target_listen(listen.port(), "control")],
                ..Default::default()
            },
        );
    })
    .await;

    // Open the SSE stream first: the handler subscribes to the bus before
    // the response headers arrive, so nothing sent afterwards is missed.
    let client = reqwest::Client::new();
    let mut resp = client
        .get(format!(
            "http://{addr}{}",
            api::path::stream_cable_messages(stream_id, "control")
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let msg = b"receive-inject\x00\x01\xff";
    let udp_sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    udp_sender.send_to(msg, listen).await.unwrap();

    let mut buffer = String::new();
    let data = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        next_sse_data(&mut resp, &mut buffer),
    )
    .await
    .expect("timeout waiting for SSE message from the cable bus");
    let event: api::response::CableMessage =
        serde_json::from_str(&data).expect("SSE event is not a cable message");
    let data = event.data.expect("expected a data event, got lagged");
    assert_eq!(BASE64.decode(data).unwrap(), msg);
}
