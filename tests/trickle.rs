//! Regression test for <https://github.com/binbat/live777/issues/417>
//!
//! A WHEP client that POSTs its offer before ICE gathering finishes (zero
//! candidates in the offer SDP, e.g. `@binbat/whip-whep`) relies on trickle
//! ICE: candidates are sent afterwards via `PATCH /session/{stream}/{session}`
//! with `application/trickle-ice-sdpfrag`. These tests assert the session
//! still reaches `Connected`.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Once},
    time::Duration,
};

use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use tokio::net::TcpListener;
use tokio::sync::{Notify, watch};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceGatheringState, RTCPeerConnectionState, RTCSessionDescription,
};
use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};

mod common;
use common::shutdown_signal;

static TRACING_INIT: Once = Once::new();

fn init_liveion_test_environment() {
    TRACING_INIT.call_once(|| {
        // Both WebRTC peers run in this process; pin ICE candidates to
        // loopback so CI runners cannot choose an unroutable host interface.
        unsafe {
            std::env::set_var("LIVE777_WEBRTC_ICE_UDP_ADDRS", "127.0.0.1:0");
        }

        let filter = std::env::var("RUST_LOG")
            .unwrap_or_else(|_| "live777=info,liveion=info,livetwo=info,libwish=info".to_string());
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

#[derive(Clone)]
struct TrickleHandler {
    gather_complete: Arc<Notify>,
    state_tx: watch::Sender<RTCPeerConnectionState>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for TrickleHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.state_tx.send(state);
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gather_complete.notify_one();
        }
    }
}

/// Remove `a=candidate:` / `a=end-of-candidates` lines so an SDP looks like
/// one produced/consumed before ICE gathering finished.
fn strip_ice_candidates(sdp: &str) -> String {
    sdp.lines()
        .filter(|line| {
            !line.starts_with("a=candidate:") && !line.starts_with("a=end-of-candidates")
        })
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n"
}

/// Build a `trickle-ice-sdpfrag` body from the gathered local description:
/// the m= line and mid of every media section, followed by its candidates.
fn build_trickle_frag(sdp: &str) -> String {
    let mut frag = String::new();
    let mut current: Vec<String> = Vec::new();
    let flush = |section: &mut Vec<String>, frag: &mut String| {
        if section.is_empty() {
            return;
        }
        let m_line = section[0].clone();
        let mid = section
            .iter()
            .find_map(|line| line.strip_prefix("a=mid:"))
            .unwrap_or_default()
            .to_string();
        let candidates: Vec<&String> = section
            .iter()
            .filter(|line| line.starts_with("a=candidate:"))
            .collect();
        if !candidates.is_empty() {
            frag.push_str(&m_line);
            frag.push_str("\r\n");
            frag.push_str(&format!("a=mid:{mid}\r\n"));
            for candidate in candidates {
                frag.push_str(candidate);
                frag.push_str("\r\n");
            }
            frag.push_str("a=end-of-candidates\r\n");
        }
        section.clear();
    };

    for line in sdp.lines() {
        if line.starts_with("m=") {
            flush(&mut current, &mut frag);
        }
        current.push(line.to_string());
    }
    flush(&mut current, &mut frag);
    frag
}

/// Run one WHEP trickle session against an in-process liveion and return the
/// client's connection-state receiver.
///
/// With `strip_answer_candidates` the server candidates are removed from the
/// answer before it is applied, so the client has no remote candidates at
/// all: the session can then only connect through *server-initiated* checks
/// towards the trickled client candidates — the exact server-side path
/// issue #417 is about. That path only exists with a full-ICE server
/// (`ice_lite = false`); an ICE Lite server never initiates checks by
/// design (RFC 8445 section 2.7).
async fn whep_trickle_session(
    stream_id: &str,
    strip_answer_candidates: bool,
    ice_lite: bool,
) -> watch::Receiver<RTCPeerConnectionState> {
    let mut cfg = liveion::config::Config::default();
    cfg.webrtc.ice_lite = ice_lite;
    if ice_lite {
        // The shipped sample config sets a STUN server, and ICE Lite agents
        // gather host candidates only — liveion must drop the configured
        // servers before they reach the agent, which rejects URLs it cannot
        // use. Exercise exactly that path.
        cfg.ice_servers = iceserver::default_ice_servers();
    }
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);

    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let client = reqwest::Client::new();
    let res = client
        .post(format!("http://{addr}{}", api::path::streams(stream_id)))
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    // WHEP client peer with a recvonly video transceiver.
    let gather_complete = Arc::new(Notify::new());
    let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
    let handler: Arc<dyn PeerConnectionEventHandler> = Arc::new(TrickleHandler {
        gather_complete: gather_complete.clone(),
        state_tx,
    });

    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs().unwrap();

    let peer: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::<SocketAddr>::new()
            .with_media_engine(media_engine)
            .with_handler(handler)
            .with_udp_addrs(livetwo::utils::webrtc::ice_udp_addrs())
            .with_configuration(RTCConfigurationBuilder::new().build())
            .build()
            .await
            .unwrap(),
    );

    peer.add_transceiver_from_kind(
        RtpCodecKind::Video,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            streams: vec![],
            send_encodings: vec![],
        }),
    )
    .await
    .unwrap();

    // POST the offer BEFORE ICE gathering finishes: the offer carries zero
    // candidates, like `@binbat/whip-whep` does (issue #417).
    let offer = peer.create_offer(None).await.unwrap();
    peer.set_local_description(offer).await.unwrap();
    let local_desc = peer.local_description().await.unwrap();
    let offer_sdp = strip_ice_candidates(&local_desc.sdp);
    assert!(
        !offer_sdp.lines().any(|l| l.starts_with("a=candidate:")),
        "offer SDP must not contain ICE candidates"
    );

    let res = client
        .post(format!("http://{addr}{}", api::path::whep(stream_id)))
        .header(http::header::CONTENT_TYPE, "application/sdp")
        .body(offer_sdp)
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::CREATED, res.status());
    let location = res
        .headers()
        .get(http::header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let answer = res.text().await.unwrap();

    // ICE Lite endpoints must mark the answer with `a=ice-lite` (RFC 8445
    // section 2.7, RFC 9725); a full-ICE server must not.
    assert_eq!(
        ice_lite,
        answer.lines().any(|line| line == "a=ice-lite"),
        "answer SDP ice-lite marker does not match ice_lite={ice_lite}:\n{answer}"
    );

    let answer = if strip_answer_candidates {
        strip_ice_candidates(&answer)
    } else {
        answer
    };
    peer.set_remote_description(RTCSessionDescription::answer(answer).unwrap())
        .await
        .unwrap();

    // Gather, then trickle the candidates through the WHEP resource. The
    // delay mirrors the issue #417 repro (browser PATCHes ~1.5 s after the
    // answer), so the server has long settled with an empty remote
    // candidate list when the trickle arrives.
    tokio::time::timeout(Duration::from_secs(5), gather_complete.notified())
        .await
        .expect("ICE gathering did not complete");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let local_desc = peer.local_description().await.unwrap();
    let frag = build_trickle_frag(&local_desc.sdp);
    assert!(
        frag.contains("a=candidate:"),
        "no gathered candidates to trickle:\n{}",
        local_desc.sdp
    );

    let res = client
        .patch(format!("http://{addr}{location}"))
        .header(
            http::header::CONTENT_TYPE,
            "application/trickle-ice-sdpfrag",
        )
        .body(frag)
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    state_rx
}

async fn wait_connected(mut state_rx: watch::Receiver<RTCPeerConnectionState>) -> bool {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            state_rx.changed().await.unwrap();
            match *state_rx.borrow() {
                RTCPeerConnectionState::Connected => return true,
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => return false,
                _ => {}
            }
        }
    })
    .await
    .unwrap_or(false)
}

/// Issue #417 flow: zero-candidate offer, candidates trickled via PATCH.
/// Runs against the default ICE Lite server: the client checks the server's
/// answer candidates and nominates; the server only responds.
#[tokio::test]
async fn test_whep_trickle_ice_connects() {
    init_liveion_test_environment();

    let state_rx = whep_trickle_session("trickle", false, true).await;
    assert!(
        wait_connected(state_rx.clone()).await,
        "WHEP trickle-ICE session did not reach Connected (issue #417); last state: {:?}",
        state_rx.borrow()
    );
}

/// The pure server-side trickle path: the client never learns any server
/// candidates, so only server-initiated checks towards the trickled client
/// candidate can establish connectivity. ICE Lite servers (the default)
/// never initiate checks by design, so this scenario is exercised against a
/// full-ICE server (`ice_lite = false`) to keep the path covered.
#[tokio::test]
async fn test_whep_trickle_ice_server_initiated_checks() {
    init_liveion_test_environment();

    let state_rx = whep_trickle_session("trickle-server-checks", true, false).await;
    assert!(
        wait_connected(state_rx.clone()).await,
        "server never used the trickled candidate for connectivity checks (issue #417); \
         last state: {:?}",
        state_rx.borrow()
    );
}
