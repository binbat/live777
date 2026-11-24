//! Recorder API integration tests (`recorder` feature).

#![cfg(feature = "recorder")]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use tokio::sync::Notify;
use webrtc::media_stream::MediaStreamTrack;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
use webrtc::peer_connection::{PeerConnection, RTCSessionDescription};

mod common;

/// Publish a single video track with the given codec MIME and pump synthetic
/// RTP (never a valid bitstream — only the track metadata matters here),
/// returning once the peer is connected and media is flowing.
async fn publish_video(addr: &SocketAddr, codec_mime: &str) -> Arc<dyn PeerConnection> {
    let gather_complete = Arc::new(Notify::new());
    let publish = livetwo::whip::core::create_publish_peer(
        gather_complete.clone(),
        livetwo::whip::core::PublishPeerOptions {
            ice_servers: Vec::new(),
            extra_video_codecs: Vec::new(),
            cable: None,
        },
    )
    .await
    .expect("create publish peer");
    let peer = publish.peer.clone();

    let ssrc = 0x5a5a_5a5a_u32;
    let track: Arc<dyn TrackLocal> = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
        "recorder-test".to_owned(),
        "recorder-test-video".to_owned(),
        "recorder-test-video".to_owned(),
        RtpCodecKind::Video,
        vec![RTCRtpEncodingParameters {
            rtp_coding_parameters: RTCRtpCodingParameters {
                ssrc: Some(ssrc),
                ..Default::default()
            },
            codec: RTCRtpCodec {
                mime_type: codec_mime.to_owned(),
                clock_rate: 90_000,
                channels: 0,
                sdp_fmtp_line: String::new(),
                rtcp_feedback: Vec::new(),
            },
            ..Default::default()
        }],
    )));
    peer.add_track(track.clone()).await.expect("add track");

    let offer = peer.create_offer(None).await.expect("create offer");
    peer.set_local_description(offer)
        .await
        .expect("set local description");
    tokio::time::timeout(Duration::from_secs(5), gather_complete.notified())
        .await
        .expect("ICE gathering timed out");
    let local = peer
        .local_description()
        .await
        .expect("local description after gathering");

    let res = reqwest::Client::new()
        .post(format!("http://{addr}{}", api::path::whip("-")))
        .header("content-type", "application/sdp")
        .body(local.sdp)
        .send()
        .await
        .expect("WHIP POST");
    assert_eq!(http::StatusCode::CREATED, res.status());
    let answer = res.text().await.expect("WHIP answer body");
    peer.set_remote_description(RTCSessionDescription::answer(answer).expect("parse WHIP answer"))
        .await
        .expect("set remote description");

    livetwo::whip::core::wait_for_peer_connected(
        peer.clone(),
        publish.state_rx.clone(),
        publish.diagnostics.clone(),
    )
    .await
    .expect("publish peer did not connect");

    // The answer may have remapped the dynamic payload type; write packets
    // with the negotiated one.
    let mut payload_type = None;
    for sender in peer.get_senders().await {
        if let Ok(params) = sender.get_parameters().await
            && let Some(codec) = params.rtp_parameters.codecs.first()
        {
            payload_type = Some(codec.payload_type);
        }
    }
    let payload_type = payload_type.expect("negotiated payload type");

    // Pump synthetic RTP so the server-side track fires on_track; the
    // payload is never decoded by these tests.
    tokio::spawn(async move {
        let mut seq: u16 = 0;
        let mut timestamp: u32 = 0;
        let mut interval = tokio::time::interval(Duration::from_millis(20));
        loop {
            interval.tick().await;
            let packet = rtc::rtp::packet::Packet {
                header: rtc::rtp::header::Header {
                    version: 2,
                    marker: true,
                    payload_type,
                    sequence_number: seq,
                    timestamp,
                    ssrc,
                    ..Default::default()
                },
                payload: vec![0u8; 1000].into(),
            };
            if track.write_rtp(packet).await.is_err() {
                // The peer is closing; the test drives teardown.
                break;
            }
            seq = seq.wrapping_add(1);
            timestamp = timestamp.wrapping_add(3000);
        }
    });

    peer
}

/// Start liveion with the recorder writing into a temporary directory.
async fn start_recorder_liveion() -> (SocketAddr, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_str().expect("utf8 tempdir").to_string();
    let index = dir
        .path()
        .join("index.json")
        .to_str()
        .expect("utf8 tempdir")
        .to_string();
    let addr = common::start_liveion_with(None, |cfg| {
        cfg.recorder.storage = storage::StorageConfig::Fs { root };
        cfg.recorder.index_path = Some(index);
    })
    .await;
    (addr, dir)
}

/// Issue #524: a stream whose video codec the recorder cannot write must fail
/// fast with 400 instead of being accepted and silently recorded audio-only.
#[tokio::test]
async fn test_record_start_rejects_unsupported_video_codec() {
    let (addr, _dir) = start_recorder_liveion().await;
    let peer = publish_video(&addr, "video/VP8").await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let res = client
        .post(format!("http://{addr}{}", api::path::record("-")))
        .send()
        .await
        .expect("record POST");
    assert_eq!(http::StatusCode::BAD_REQUEST, res.status());
    let body = res.text().await.unwrap();
    assert!(body.contains("VP8"), "error should name the codec: {body}");

    peer.close().await.expect("close publish peer");
}

/// The same start must keep accepting a supported codec.
#[tokio::test]
async fn test_record_start_accepts_supported_video_codec() {
    let (addr, _dir) = start_recorder_liveion().await;
    let peer = publish_video(&addr, "video/H264").await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    let url = format!("http://{addr}{}", api::path::record("-"));
    let res = client.post(&url).send().await.expect("record POST");
    assert_eq!(http::StatusCode::OK, res.status());

    let res = client.delete(&url).send().await.expect("record DELETE");
    assert_eq!(http::StatusCode::OK, res.status());

    peer.close().await.expect("close publish peer");
}

/// Issue #526: recording objects must be served streamed (not buffered whole
/// in memory) and honor single Range requests with 206/416.
#[tokio::test]
async fn test_record_object_streams_and_honors_range() {
    let (addr, dir) = start_recorder_liveion().await;

    let object = b"0123456789abcdef";
    let rel = "stream1/1/manifest.mpd";
    let abs = dir.path().join(rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, object).unwrap();

    let client = reqwest::Client::new();
    let url = format!("http://{addr}/api/record/object/{rel}");
    let header_str = |res: &reqwest::Response, name: http::header::HeaderName| {
        res.headers()
            .get(name)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    };

    // Full body, with range support advertised.
    let res = client.get(&url).send().await.unwrap();
    assert_eq!(http::StatusCode::OK, res.status());
    assert_eq!(header_str(&res, http::header::ACCEPT_RANGES), "bytes");
    assert_eq!(header_str(&res, http::header::CONTENT_LENGTH), "16");
    assert_eq!(
        header_str(&res, http::header::CONTENT_TYPE),
        "application/dash+xml"
    );
    assert_eq!(res.bytes().await.unwrap().as_ref(), object);

    // Explicit range -> 206.
    let res = client
        .get(&url)
        .header("range", "bytes=4-7")
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::PARTIAL_CONTENT, res.status());
    assert_eq!(
        header_str(&res, http::header::CONTENT_RANGE),
        "bytes 4-7/16"
    );
    assert_eq!(header_str(&res, http::header::CONTENT_LENGTH), "4");
    assert_eq!(res.bytes().await.unwrap().as_ref(), &object[4..8]);

    // Suffix range.
    let res = client
        .get(&url)
        .header("range", "bytes=-4")
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::PARTIAL_CONTENT, res.status());
    assert_eq!(res.bytes().await.unwrap().as_ref(), &object[12..16]);

    // End beyond the object is clamped.
    let res = client
        .get(&url)
        .header("range", "bytes=14-100")
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::PARTIAL_CONTENT, res.status());
    assert_eq!(
        header_str(&res, http::header::CONTENT_RANGE),
        "bytes 14-15/16"
    );
    assert_eq!(res.bytes().await.unwrap().as_ref(), &object[14..16]);

    // Unsatisfiable -> 416.
    let res = client
        .get(&url)
        .header("range", "bytes=100-200")
        .send()
        .await
        .unwrap();
    assert_eq!(http::StatusCode::RANGE_NOT_SATISFIABLE, res.status());
    assert_eq!(header_str(&res, http::header::CONTENT_RANGE), "bytes */16");

    // Malformed and multi-range headers are ignored -> 200 full body.
    for range in ["items=1-2", "bytes=9-5", "bytes=0-1,3-4"] {
        let res = client
            .get(&url)
            .header("range", range)
            .send()
            .await
            .unwrap();
        assert_eq!(http::StatusCode::OK, res.status(), "range: {range}");
        assert_eq!(res.bytes().await.unwrap().as_ref(), object);
    }
}
