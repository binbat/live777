//! Regression test for <https://github.com/binbat/live777/issues/406>
//!
//! A WHEP subscriber that attaches *before* the WHIP publisher is up gets its
//! sender track created with the fallback codec (VP8, see
//! `PeerForwardInternal::new_sender`). When the publisher then negotiates a
//! different codec (VP9 here), the bind path must switch the outbound codec
//! by reusing the already-bound sender track: webrtc-rs 0.20
//! `RtpSender::replace_track` never `bind()`s the replacement
//! `TrackLocalStaticRTP`, so every write to a replaced track fails with
//! "track is not binding yet" and the subscriber receives zero RTP forever —
//! in a browser, a permanently `muted` video track with zero decoded frames.
//!
//! This test attaches the subscriber first, publishes VP9 second, and asserts
//! the subscriber's peer actually receives RTP packets.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Once,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use rtc::media_stream::MediaStreamTrack;
use rtc::rtp::header::Header;
use rtc::rtp::packet::Packet;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
    RtpCodecKind,
};
use tokio::net::TcpListener;
use tokio::sync::watch;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
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

/// WHEP client handler: report connection state, count inbound RTP packets.
#[derive(Clone)]
struct SubscriberHandler {
    state_tx: watch::Sender<RTCPeerConnectionState>,
    packets: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for SubscriberHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.state_tx.send(state);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let packets = self.packets.clone();
        tokio::spawn(async move {
            while let Some(event) = track.poll().await {
                if let TrackRemoteEvent::OnRtpPacket(_) = event {
                    packets.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
    }
}

/// WHIP client handler: connection state only.
#[derive(Clone)]
struct PublisherHandler {
    state_tx: watch::Sender<RTCPeerConnectionState>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for PublisherHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.state_tx.send(state);
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

/// Issue #406 flow: the WHEP subscriber attaches while the stream has no
/// publisher yet (its sender track is created with the VP8 fallback codec),
/// then a VP9 publisher arrives. The codec mismatch must not strand the
/// subscriber: packets must flow.
#[tokio::test]
async fn whep_subscribe_before_publish_with_codec_switch_receives_media() {
    init_liveion_test_environment();

    let cfg = liveion::config::Config::default();
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let client = reqwest::Client::new();
    let stream = "early-subscribe";
    let res = client
        .post(format!("http://{addr}{}", api::path::streams(stream)))
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::NO_CONTENT, res.status());

    // --- 1. WHEP subscriber attaches first (no publisher yet) ---
    let (sub_state_tx, sub_state_rx) = watch::channel(RTCPeerConnectionState::New);
    let packets = Arc::new(AtomicU64::new(0));
    let sub_handler: Arc<dyn PeerConnectionEventHandler> = Arc::new(SubscriberHandler {
        state_tx: sub_state_tx,
        packets: packets.clone(),
    });
    let mut sub_media_engine = MediaEngine::default();
    sub_media_engine.register_default_codecs().unwrap();
    let sub_peer: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::<SocketAddr>::new()
            .with_media_engine(sub_media_engine)
            .with_handler(sub_handler)
            .with_udp_addrs(livetwo::utils::webrtc::ice_udp_addrs())
            .with_configuration(RTCConfigurationBuilder::new().build())
            .build()
            .await
            .unwrap(),
    );
    sub_peer
        .add_transceiver_from_kind(
            RtpCodecKind::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                streams: vec![],
                send_encodings: vec![],
            }),
        )
        .await
        .unwrap();
    let offer = sub_peer.create_offer(None).await.unwrap();
    sub_peer.set_local_description(offer).await.unwrap();
    // Gather fully so the offer carries candidates (no trickle needed).
    tokio::time::sleep(Duration::from_millis(500)).await;
    let offer_sdp = sub_peer.local_description().await.unwrap().sdp;

    let res = client
        .post(format!("http://{addr}{}", api::path::whep(stream)))
        .header(http::header::CONTENT_TYPE, "application/sdp")
        .body(offer_sdp)
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::CREATED, res.status());
    let answer = res.text().await.unwrap();
    sub_peer
        .set_remote_description(RTCSessionDescription::answer(answer).unwrap())
        .await
        .unwrap();

    assert!(
        wait_connected(sub_state_rx.clone()).await,
        "WHEP subscriber did not reach Connected; last state: {:?}",
        sub_state_rx.borrow()
    );

    // --- 2. VP9 publisher arrives (subscriber sender track is VP8) ---
    let (pub_state_tx, pub_state_rx) = watch::channel(RTCPeerConnectionState::New);
    let pub_handler: Arc<dyn PeerConnectionEventHandler> = Arc::new(PublisherHandler {
        state_tx: pub_state_tx,
    });
    // VP9-only media engine: the publish leg negotiates VP9 no matter what
    // the subscriber's sender track declared.
    let mut pub_media_engine = MediaEngine::default();
    pub_media_engine
        .register_codec(
            RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: "video/VP9".to_owned(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: "profile-id=0".to_owned(),
                    rtcp_feedback: vec![],
                },
                payload_type: 98,
            },
            RtpCodecKind::Video,
        )
        .unwrap();
    let pub_peer: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::<SocketAddr>::new()
            .with_media_engine(pub_media_engine)
            .with_handler(pub_handler)
            .with_udp_addrs(livetwo::utils::webrtc::ice_udp_addrs())
            .with_configuration(RTCConfigurationBuilder::new().build())
            .build()
            .await
            .unwrap(),
    );

    let publish_ssrc: u32 = 0x4060_0001;
    let track: Arc<dyn TrackLocal> = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
        "early-subscribe".to_owned(),
        "early-subscribe-video".to_owned(),
        "early-subscribe".to_owned(),
        RtpCodecKind::Video,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(publish_ssrc),
                ..Default::default()
            },
            codec: RTCRtpCodec {
                mime_type: "video/VP9".to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "profile-id=0".to_owned(),
                rtcp_feedback: vec![],
            },
            ..Default::default()
        }],
    )));
    pub_peer.add_track(track.clone()).await.unwrap();

    let offer = pub_peer.create_offer(None).await.unwrap();
    pub_peer.set_local_description(offer).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let offer_sdp = pub_peer.local_description().await.unwrap().sdp;

    let res = client
        .post(format!("http://{addr}{}", api::path::whip(stream)))
        .header(http::header::CONTENT_TYPE, "application/sdp")
        .body(offer_sdp)
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::CREATED, res.status());
    let answer = res.text().await.unwrap();
    pub_peer
        .set_remote_description(RTCSessionDescription::answer(answer).unwrap())
        .await
        .unwrap();

    assert!(
        wait_connected(pub_state_rx.clone()).await,
        "WHIP publisher did not reach Connected; last state: {:?}",
        pub_state_rx.borrow()
    );

    // --- 3. Publish VP9 packets (content is irrelevant for forwarding) ---
    let writer = tokio::spawn(async move {
        let mut sequence_number: u16 = 0;
        let mut timestamp: u32 = 0;
        for _ in 0..500 {
            let packet = Packet {
                header: Header {
                    version: 2,
                    payload_type: 98,
                    sequence_number,
                    timestamp,
                    ssrc: publish_ssrc,
                    ..Default::default()
                },
                // Minimal VP9 payload descriptor + dummy picture data; the
                // SFU forwards RTP without parsing the payload.
                payload: vec![0x90, 0x80, 0x01, 0x02, 0x03, 0x04].into(),
            };
            if track.write_rtp(packet).await.is_err() {
                break;
            }
            sequence_number = sequence_number.wrapping_add(1);
            timestamp = timestamp.wrapping_add(1800);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    // The subscriber must receive RTP within a few seconds; with the
    // replace_track regression this stayed 0 forever.
    let received = tokio::time::timeout(Duration::from_secs(10), async {
        while packets.load(Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    writer.abort();

    assert!(
        received.is_ok(),
        "subscriber received no RTP: early subscriber is stranded after a codec switch (issue #406)"
    );
}
