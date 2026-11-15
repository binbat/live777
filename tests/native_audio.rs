//! End-to-end test for the native audio (Opus) source: a provisioned stream
//! whose `sine` capture + Opus encoder leg is WHEP-subscribed by a real
//! WebRTC peer, asserting the fixed wire contract on the received RTP —
//! payload type 111, 48 kHz Opus, every payload decodable by opus-rs — and
//! that the stream API reports the opus audio track.
//!
//! Gated on `native-source,encoder-opus`: under the default CI feature set
//! (which excludes native sources) this binary compiles to an empty harness.

#![cfg(all(feature = "native-source", feature = "encoder-opus"))]

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Mutex, Once,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use tokio::net::TcpListener;
use tokio::sync::watch;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCPeerConnectionState, RTCSessionDescription,
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

/// Counters for the WHEP subscriber's inbound audio RTP.
#[derive(Default)]
struct AudioStats {
    packets: AtomicU64,
    pt111: AtomicU64,
    decoded_samples: AtomicU64,
    decode_errors: AtomicU64,
}

/// WHEP client handler: report connection state; on the audio track count
/// packets and payload-type-111 packets, and Opus-decode every payload.
#[derive(Clone)]
struct SubscriberHandler {
    state_tx: watch::Sender<RTCPeerConnectionState>,
    stats: Arc<AudioStats>,
    first_error: Arc<Mutex<Option<String>>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for SubscriberHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.state_tx.send(state);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let stats = self.stats.clone();
        let first_error = self.first_error.clone();
        tokio::spawn(async move {
            // Stereo decoder matching the source config (channels = 2); a
            // stereo decoder also plays mono packets, as in libopus.
            let mut decoder = opus_rs::OpusDecoder::new(48_000, 2).expect("opus decoder");
            let mut pcm = vec![0f32; 5760 * 2];
            while let Some(event) = track.poll().await {
                if let TrackRemoteEvent::OnRtpPacket(packet) = event {
                    stats.packets.fetch_add(1, Ordering::Relaxed);
                    if packet.header.payload_type == 111 {
                        stats.pt111.fetch_add(1, Ordering::Relaxed);
                    }
                    match decoder.decode(&packet.payload, 5760, &mut pcm) {
                        Ok(n) => {
                            stats.decoded_samples.fetch_add(n as u64, Ordering::Relaxed);
                        }
                        Err(e) => {
                            stats.decode_errors.fetch_add(1, Ordering::Relaxed);
                            let mut guard = first_error.lock().unwrap();
                            if guard.is_none() {
                                *guard = Some(e.to_string());
                            }
                        }
                    }
                }
            }
        });
    }
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

/// A provisioned stream whose source is the `sine` audio capture + Opus
/// encoder must serve WHEP subscribers end to end: steady Opus RTP at the
/// fixed wire contract, every payload decodable, and the stream API
/// reporting the opus audio track.
#[tokio::test]
async fn native_audio_source_sine_whep_opus() {
    init_liveion_test_environment();

    // Provision the stream from the same TOML shape users write
    // (`[stream.mic.sources.audio_capture]` + `.audio_encoder`).
    let entry: liveion::config::StreamEntry = toml::from_str(
        r#"
        [[sources]]
        [sources.audio_capture]
        backend = "sine"
        channels = 2

        [sources.audio_encoder]
        codec = "opus"
        bitrate = 64000
        "#,
    )
    .expect("parse stream entry");
    entry
        .sources
        .first()
        .expect("one source")
        .validate()
        .expect("source config validates");

    let mut cfg = liveion::config::Config::default();
    cfg.stream.streams.insert("mic".to_string(), entry);

    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    // The source auto-starts at server startup; poll the streams API until
    // the media info reports the opus audio track (kind/codec/fmtp mirror
    // the fixed wire contract in `NativeEncodedSource::get_audio_codec`).
    let mut stream = None;
    for _ in 0..100 {
        let body = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap()
            .json::<Vec<api::response::Stream>>()
            .await
            .unwrap_or_default();
        if let Some(s) = body.into_iter().find(|s| s.id == "mic")
            && s.codecs
                .iter()
                .any(|c| c.kind == "audio" && c.codec == "opus")
        {
            stream = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let stream = stream.expect("stream 'mic' did not report an opus audio track within 10s");
    assert!(stream.provisioned);
    let audio = stream
        .codecs
        .iter()
        .find(|c| c.kind == "audio")
        .expect("audio codec entry");
    assert_eq!(audio.fmtp, "minptime=10;useinbandfec=1");
    assert!(
        stream
            .publish
            .sessions
            .iter()
            .any(|s| s.id == "virtual-source"),
        "source bridge must surface as the virtual-source publish session: {:?}",
        stream.publish.sessions
    );

    // WHEP subscriber: one recvonly audio transceiver, full-SDP offer (the
    // ICE gather sleep below puts the candidates into the offer — no
    // trickle needed).
    let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
    let stats = Arc::new(AudioStats::default());
    let first_error = Arc::new(Mutex::new(None));
    let handler: Arc<dyn PeerConnectionEventHandler> = Arc::new(SubscriberHandler {
        state_tx,
        stats: stats.clone(),
        first_error: first_error.clone(),
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
        RtpCodecKind::Audio,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            streams: vec![],
            send_encodings: vec![],
        }),
    )
    .await
    .unwrap();
    let offer = peer.create_offer(None).await.unwrap();
    peer.set_local_description(offer).await.unwrap();
    // Gather fully so the offer carries candidates.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let offer_sdp = peer.local_description().await.unwrap().sdp;

    let res = reqwest::Client::new()
        .post(format!("http://{addr}{}", api::path::whep("mic")))
        .header(http::header::CONTENT_TYPE, "application/sdp")
        .body(offer_sdp)
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::CREATED, res.status());
    let answer = res.text().await.unwrap();
    peer.set_remote_description(RTCSessionDescription::answer(answer).unwrap())
        .await
        .unwrap();

    assert!(
        wait_connected(state_rx.clone()).await,
        "WHEP subscriber did not reach Connected; last state: {:?}",
        state_rx.borrow()
    );

    // Collect 3 s of audio: at 20 ms Opus frames that is ~150 packets.
    tokio::time::sleep(Duration::from_secs(3)).await;

    let packets = stats.packets.load(Ordering::Relaxed);
    let pt111 = stats.pt111.load(Ordering::Relaxed);
    let decoded = stats.decoded_samples.load(Ordering::Relaxed);
    let errors = stats.decode_errors.load(Ordering::Relaxed);

    peer.close().await.unwrap();

    assert!(
        packets >= 50,
        "expected a steady Opus flow (>= 50 packets in 3 s), got {packets}"
    );
    assert_eq!(
        packets, pt111,
        "every audio packet must carry payload type 111"
    );
    assert_eq!(
        errors,
        0,
        "opus decode errors: {:?}",
        first_error.lock().unwrap()
    );
    assert!(decoded > 0, "no opus samples decoded");
}
