use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, broadcast};

use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters};
use rtc::shared::marshal::{Marshal, MarshalSize};

use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::config::{RtspConfig, RtspListen};
use crate::forward::rtcp::RtcpMessage;
use crate::forward::track::PublishTrackRemote;
use crate::stream::manager::Manager;
use rtc_rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use rtc_rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use rtsp::{ServerConfig, udp_route};

const DEFAULT_STREAM: &str = "rtsp";

pub async fn start_rtsp_server(
    manager: Arc<Manager>,
    config: RtspConfig,
    cancel: CancellationToken,
) {
    let listen = RtspListen::parse(&config.listen)
        .unwrap_or_else(|e| panic!("invalid RTSP listen URL '{}': {e}", config.listen));
    info!(
        "Starting RTSP server on {} (auth: {})",
        listen.addr,
        listen.enable_auth()
    );
    let listen_addr = format_bind_addr(listen.addr);
    let handler = RtspHandler {
        manager,
        stream_ready: Arc::new(RwLock::new(HashMap::new())),
    };
    let server_config = ServerConfig {
        listen_addr: listen.addr,
        max_connections: config.max_connections,
        session_timeout: config.session_timeout,
        enable_auth: listen.enable_auth(),
        username: listen.username.clone().unwrap_or_default(),
        password: listen.password.clone().unwrap_or_default(),
        realm: config.realm.clone(),
    };

    tokio::spawn(async move {
        if let Err(e) = rtsp::setup_rtsp_server_with_handler(
            &listen_addr,
            rtsp::SessionMode::Mixed,
            handler,
            server_config,
            cancel,
        )
        .await
        {
            error!("RTSP server error: {}", e);
        }
    });
}

#[derive(Clone)]
struct RtspHandler {
    manager: Arc<Manager>,
    stream_ready: Arc<RwLock<HashMap<String, broadcast::Sender<()>>>>,
}

#[async_trait::async_trait]
impl rtsp::server::SessionHandler for RtspHandler {
    async fn on_announce(&self, path: String, sdp: Vec<u8>) -> Result<()> {
        let stream_id = stream_id_from_path(path);
        let media_info = rtsp::parse_media_info_from_sdp(&sdp)?;

        // Recreate the stream so a new publisher always starts from a clean state.
        let _ = self.manager.stream_delete(stream_id.clone()).await;
        self.manager.stream_create(stream_id.clone()).await?;
        // stream_create already inserted the forward; get_forward is sufficient
        // and avoids an unnecessary second insertion path.
        let forward = self
            .manager
            .get_forward(&stream_id)
            .await
            .ok_or_else(|| anyhow!("Forward {} disappeared after create", stream_id))?;

        if let Some(video) = &media_info.video_codec {
            let codec = video_codec_to_rtc(video);
            if let Err(e) = forward.add_virtual_track(RtpCodecKind::Video, codec).await {
                warn!(
                    "Failed to add virtual video track for {}: {:?}",
                    stream_id, e
                );
            }
        }
        if let Some(audio) = &media_info.audio_codec {
            let codec = audio_codec_to_rtc(audio);
            if let Err(e) = forward.add_virtual_track(RtpCodecKind::Audio, codec).await {
                warn!(
                    "Failed to add virtual audio track for {}: {:?}",
                    stream_id, e
                );
            }
        }

        self.notify_stream_ready(&stream_id).await;

        info!("RTSP push accepted for stream {}", stream_id);
        Ok(())
    }

    async fn on_describe(&self, path: String) -> Result<Vec<u8>> {
        let stream_id = stream_id_from_path(path);
        let forward = wait_for_forward(&self.manager, &self.stream_ready, &stream_id)
            .await
            .map_err(|e| anyhow!("Stream {} not available: {}", stream_id, e))?;
        let tracks = wait_for_tracks(&forward)
            .await
            .map_err(|e| anyhow!("Stream {} has no tracks: {}", stream_id, e))?;
        Ok(build_sdp_from_tracks(&tracks)?.into_bytes())
    }

    async fn on_session(
        &self,
        path: String,
        mode: rtsp::SessionMode,
        media_info: rtsp::MediaInfo,
        endpoint: rtsp::SessionEndpoint,
        cancel: CancellationToken,
    ) -> Result<()> {
        let stream_id = stream_id_from_path(path);

        match mode {
            rtsp::SessionMode::Push => {
                let (mut rx, tx) = match endpoint {
                    rtsp::SessionEndpoint::Push(rx, tx) => (rx, tx),
                    _ => return Err(anyhow!("Expected push endpoint")),
                };

                let forward = self.manager.get_or_create_forward(&stream_id).await;
                let channel_to_kind = build_channel_kind_map(&media_info);

                // Forward RTCP keyframe requests from pull subscribers back to the
                // push client on its RTCP channel.
                let rtcp_channel = media_info
                    .video_transport
                    .as_ref()
                    .and_then(|t| t.tcp_channels())
                    .map(|(_, rtcp)| rtcp)
                    .unwrap_or(1);
                let mut rtcp_rx = forward.internal.publish_rtcp_channel.subscribe();
                let stream_id_for_rtcp = stream_id.clone();
                let push_cancel = cancel.child_token();
                let rtcp_cancel = push_cancel.clone();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = rtcp_cancel.cancelled() => break,
                            result = rtcp_rx.recv() => {
                                let (msg, ssrc) = match result {
                                    Ok(v) => v,
                                    Err(_) => break,
                                };
                                let packet = match msg {
                                    RtcpMessage::PictureLossIndication => {
                                        let pli = PictureLossIndication {
                                            sender_ssrc: 0,
                                            media_ssrc: ssrc,
                                        };
                                        match pli.marshal() {
                                            Ok(buf) => buf.to_vec(),
                                            Err(e) => {
                                                debug!(
                                                    "Failed to marshal PLI for {}: {}",
                                                    stream_id_for_rtcp, e
                                                );
                                                continue;
                                            }
                                        }
                                    }
                                    RtcpMessage::_FullIntraRequest => {
                                        let fir = FullIntraRequest {
                                            sender_ssrc: 0,
                                            media_ssrc: ssrc,
                                            fir: vec![],
                                        };
                                        match fir.marshal() {
                                            Ok(buf) => buf.to_vec(),
                                            Err(e) => {
                                                debug!(
                                                    "Failed to marshal FIR for {}: {}",
                                                    stream_id_for_rtcp, e
                                                );
                                                continue;
                                            }
                                        }
                                    }
                                    _ => continue,
                                };
                                if tx.send((rtcp_channel, packet)).await.is_err() {
                                    debug!("RTCP feedback channel closed for {}", stream_id_for_rtcp);
                                    break;
                                }
                            }
                        }
                    }
                });

                let manager = self.manager.clone();
                let stream_ready = self.stream_ready.clone();
                tokio::spawn(async move {
                    while let Some((channel, data)) = rx.recv().await {
                        let Some(&kind) = channel_to_kind.get(&channel) else {
                            continue;
                        };

                        if channel % 2 != 0 {
                            // RTCP from the push client. The push client is the
                            // publisher, so do not echo its own keyframe requests
                            // back to itself.
                            continue;
                        }

                        let result = match kind {
                            RtpCodecKind::Video => forward.inject_video_rtp(&data).await,
                            RtpCodecKind::Audio => forward.inject_audio_rtp(&data).await,
                            _ => continue,
                        };
                        if let Err(e) = result {
                            trace!(
                                "Failed to inject {:?} RTP for stream {}: {:?}",
                                kind, stream_id, e
                            );
                        }
                    }
                    info!("RTSP push forward stopped for {}", stream_id);
                    push_cancel.cancel();
                    if let Err(e) = manager.stream_delete(stream_id.clone()).await {
                        debug!(
                            "Failed to delete stream {} on push disconnect: {}",
                            stream_id, e
                        );
                    }
                    let mut map = stream_ready.write().await;
                    map.remove(&stream_id);
                });
            }
            rtsp::SessionMode::Pull => {
                let (tx, mut rtcp_rx) = match endpoint {
                    rtsp::SessionEndpoint::Pull(tx, rtcp_rx) => (tx, rtcp_rx),
                    _ => return Err(anyhow!("Expected pull endpoint")),
                };

                let pull_cancel = cancel.child_token();
                let manager = self.manager.clone();
                let stream_ready = self.stream_ready.clone();
                let rtcp_manager = self.manager.clone();
                let rtcp_stream_id = stream_id.clone();
                let rtcp_cancel = pull_cancel.child_token();
                tokio::spawn(async move {
                    loop {
                        tokio::select! {
                            _ = rtcp_cancel.cancelled() => break,
                            maybe_rtcp = rtcp_rx.recv() => {
                                let Some((_, data)) = maybe_rtcp else {
                                    break;
                                };
                                forward_rtcp_to_publish(&rtcp_manager, &rtcp_stream_id, &data).await;
                            }
                        }
                    }
                });

                tokio::spawn(async move {
                    let forward = match wait_for_forward(&manager, &stream_ready, &stream_id).await
                    {
                        Ok(f) => f,
                        Err(e) => {
                            error!("RTSP pull forward failed for {}: {}", stream_id, e);
                            return;
                        }
                    };
                    let tracks = match wait_for_tracks(&forward).await {
                        Ok(t) => t,
                        Err(e) => {
                            error!("RTSP pull no tracks for {}: {}", stream_id, e);
                            return;
                        }
                    };

                    // Use the negotiated TCP interleaved channels when available;
                    // fall back to the fixed UDP channel map (video=0/1, audio=2/3).
                    let channel_for = |kind| match kind {
                        RtpCodecKind::Video => media_info
                            .video_transport
                            .as_ref()
                            .and_then(|t| t.tcp_channels())
                            .unwrap_or((udp_route::VIDEO_RTP, udp_route::VIDEO_RTCP)),
                        RtpCodecKind::Audio => media_info
                            .audio_transport
                            .as_ref()
                            .and_then(|t| t.tcp_channels())
                            .unwrap_or((udp_route::AUDIO_RTP, udp_route::AUDIO_RTCP)),
                        _ => (udp_route::VIDEO_RTP, udp_route::VIDEO_RTCP),
                    };

                    for track in tracks {
                        let (rtp_channel, rtcp_channel) = channel_for(track.kind());

                        // RTP forward.
                        let tx_clone = tx.clone();
                        let mut packet_rx = track.subscribe();
                        let task_cancel = pull_cancel.child_token();
                        tokio::spawn(async move {
                            loop {
                                tokio::select! {
                                    _ = task_cancel.cancelled() => break,
                                    result = packet_rx.recv() => {
                                        let packet = match result {
                                            Ok(p) => p,
                                            Err(_) => break,
                                        };
                                        let mut buf = vec![0u8; packet.marshal_size()];
                                        if Marshal::marshal_to(&*packet, &mut buf).is_err() {
                                            continue;
                                        }
                                        if tx_clone.send((rtp_channel, buf)).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            }
                        });

                        // RTCP sender reports.
                        let tx_clone = tx.clone();
                        let track = track.clone();
                        let task_cancel = pull_cancel.child_token();
                        tokio::spawn(async move {
                            let mut interval = tokio::time::interval(Duration::from_secs(5));
                            loop {
                                tokio::select! {
                                    _ = task_cancel.cancelled() => break,
                                    _ = interval.tick() => {
                                        if let Some(packet) = track.generate_sender_report() {
                                            match packet.marshal() {
                                                Ok(buf) => {
                                                    if tx_clone
                                                        .send((rtcp_channel, buf.to_vec()))
                                                        .await
                                                        .is_err()
                                                    {
                                                        break;
                                                    }
                                                }
                                                Err(e) => {
                                                    debug!("Failed to marshal sender report: {}", e);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        });
                    }
                });
            }
            rtsp::SessionMode::Mixed => return Err(anyhow!("session mode must be resolved")),
        }

        Ok(())
    }

    async fn on_rtcp(&self, path: String, data: Vec<u8>) -> Result<()> {
        let stream_id = stream_id_from_path(path);
        forward_rtcp_to_publish(&self.manager, &stream_id, &data).await;
        Ok(())
    }
}

fn stream_id_from_path(path: String) -> String {
    if path.is_empty() {
        DEFAULT_STREAM.to_string()
    } else {
        path
    }
}

/// Build a map from interleaved channel number to codec kind using the
/// negotiated transport. TCP channels come from the SETUP response; UDP falls
/// back to the fixed channel map (video=0/1, audio=2/3).
fn build_channel_kind_map(media_info: &rtsp::MediaInfo) -> HashMap<u8, RtpCodecKind> {
    let mut map = HashMap::new();

    if let Some(ref transport) = media_info.video_transport {
        let (rtp, rtcp) = transport
            .tcp_channels()
            .unwrap_or((udp_route::VIDEO_RTP, udp_route::VIDEO_RTCP));
        map.insert(rtp, RtpCodecKind::Video);
        map.insert(rtcp, RtpCodecKind::Video);
    }

    if let Some(ref transport) = media_info.audio_transport {
        let (rtp, rtcp) = transport
            .tcp_channels()
            .unwrap_or((udp_route::AUDIO_RTP, udp_route::AUDIO_RTCP));
        map.insert(rtp, RtpCodecKind::Audio);
        map.insert(rtcp, RtpCodecKind::Audio);
    }

    map
}

/// Parse incoming RTCP from a pull client and forward PLI/FIR to the publisher
/// when the SSRC matches the current video track.
async fn forward_rtcp_to_publish(manager: &Manager, stream_id: &str, data: &[u8]) {
    let mut reader = data;
    let packets = match rtc_rtcp::packet::unmarshal(&mut reader) {
        Ok(packets) => packets,
        Err(e) => {
            debug!("Failed to parse RTCP for stream {}: {}", stream_id, e);
            return;
        }
    };

    let Some(forward) = manager.get_forward(stream_id).await else {
        return;
    };

    for packet in packets {
        let any = packet.as_any();
        let (is_pli, media_ssrc) = if let Some(pli) = any.downcast_ref::<PictureLossIndication>() {
            (true, pli.media_ssrc)
        } else if let Some(fir) = any.downcast_ref::<FullIntraRequest>() {
            (false, fir.media_ssrc)
        } else {
            continue;
        };

        // Collect video track references while holding the read lock, then
        // release before awaiting source_ssrc() so writers on publish_tracks
        // (e.g. codec renegotiation) are not starved.
        let video_tracks: Vec<_> = {
            let tracks = forward.internal.publish_tracks.read().await;
            tracks
                .iter()
                .filter(|t| t.kind() == RtpCodecKind::Video)
                .cloned()
                .collect()
        };
        let mut matches = false;
        for t in &video_tracks {
            if t.source_ssrc().await == media_ssrc {
                matches = true;
                break;
            }
        }

        if matches {
            let msg = if is_pli {
                RtcpMessage::PictureLossIndication
            } else {
                RtcpMessage::_FullIntraRequest
            };
            let _ = forward
                .internal
                .publish_rtcp_channel
                .send((msg, media_ssrc));
            debug!(
                "Forwarded RTCP keyframe request {:?} for stream {} ssrc {}",
                msg, stream_id, media_ssrc
            );
        }
    }
}

impl RtspHandler {
    async fn notify_stream_ready(&self, stream_id: &str) {
        let map = self.stream_ready.read().await;
        if let Some(tx) = map.get(stream_id) {
            let _ = tx.send(());
        }
    }
}

async fn wait_for_forward(
    manager: &Manager,
    stream_ready: &Arc<RwLock<HashMap<String, broadcast::Sender<()>>>>,
    stream_id: &str,
) -> Result<crate::forward::PeerForward> {
    if let Some(forward) = manager.get_forward(stream_id).await {
        return Ok(forward);
    }

    let (tx, mut rx) = {
        let mut map = stream_ready.write().await;
        if let Some(forward) = manager.get_forward(stream_id).await {
            return Ok(forward);
        }
        let tx = map
            .entry(stream_id.to_string())
            .or_insert_with(|| broadcast::channel(1).0)
            .clone();
        let rx = tx.subscribe();
        (tx, rx)
    };

    if let Some(forward) = manager.get_forward(stream_id).await {
        return Ok(forward);
    }

    let wait_result = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await;

    match wait_result {
        Err(_elapsed) => {
            // Timed out waiting for a publisher. Clean up the coordination
            // entry if no other pull clients are still waiting for this stream.
            let mut map = stream_ready.write().await;
            if tx.receiver_count() == 0 && manager.get_forward(stream_id).await.is_none() {
                map.remove(stream_id);
            }
            return Err(anyhow!("Timeout waiting for forward {}", stream_id));
        }
        Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
            // The broadcast channel cycled past unread messages (e.g. a
            // re-announce overwrote the notification). Clean up the stale
            // coordination entry and report the lag error.
            let mut map = stream_ready.write().await;
            if tx.receiver_count() == 0 && manager.get_forward(stream_id).await.is_none() {
                map.remove(stream_id);
            }
            return Err(anyhow!(
                "Stream ready notification lagged for {}",
                stream_id
            ));
        }
        Ok(Err(_)) => {
            return Err(anyhow!("Stream ready channel closed for {}", stream_id));
        }
        Ok(Ok(())) => {}
    }

    // The forward is now available. Remove the coordination entry so it does
    // not linger for the lifetime of the stream; new pull clients will find
    // the forward directly via manager.get_forward().
    {
        let mut map = stream_ready.write().await;
        map.remove(stream_id);
    }

    manager
        .get_forward(stream_id)
        .await
        .ok_or_else(|| anyhow!("Forward {} disappeared", stream_id))
}

async fn wait_for_tracks(forward: &crate::forward::PeerForward) -> Result<Vec<PublishTrackRemote>> {
    let mut rx = forward.subscribe_tracks_change();

    loop {
        {
            let tracks = forward.internal.publish_tracks.read().await;
            if !tracks.is_empty() {
                return Ok(tracks.clone());
            }
        }

        tokio::select! {
            _ = rx.recv() => continue,
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                return Err(anyhow!("Timeout waiting for publish tracks"));
            }
        }
    }
}

fn build_sdp_from_tracks(tracks: &[PublishTrackRemote]) -> Result<String> {
    let mut lines = vec![
        "v=0".to_string(),
        "o=- 0 0 IN IP4 127.0.0.1".to_string(),
        "s=liveion".to_string(),
        "t=0 0".to_string(),
    ];

    for track in tracks {
        let codec = track.codec();
        // When a WHIP-published stream is described before the first RTP
        // packet arrives, the negotiated payload type is still 0 (not yet
        // detected).  Default to 96 (dynamic PT range) for video and use the
        // static PT defined in RFC 3551 for well-known audio codecs so the
        // SDP remains valid.
        let pt = if codec.payload_type != 0 {
            codec.payload_type
        } else {
            match codec.codec.as_str() {
                "pcma" => 8,
                "pcmu" => 0,
                "g722" => 9,
                _ => 96,
            }
        };
        let (media, clock_rate, channels) = match codec.codec.as_str() {
            "h264" | "h265" | "hevc" | "vp8" | "vp9" | "av1" => ("video", codec.clock_rate, None),
            "opus" | "g722" | "pcma" | "pcmu" => {
                ("audio", codec.clock_rate, Some(codec.channels as u8))
            }
            _ => continue,
        };

        lines.push(format!("m={} 0 RTP/AVP {}", media, pt));
        if let Some(ch) = channels {
            lines.push(format!(
                "a=rtpmap:{} {}/{}/{}",
                pt,
                codec.codec.to_uppercase(),
                clock_rate,
                ch
            ));
        } else {
            lines.push(format!(
                "a=rtpmap:{} {}/{}",
                pt,
                codec.codec.to_uppercase(),
                clock_rate
            ));
        }
        if !codec.fmtp.is_empty() {
            lines.push(format!("a=fmtp:{} {}", pt, codec.fmtp));
        }
        let control_id = match track.kind() {
            RtpCodecKind::Video => "video",
            RtpCodecKind::Audio => "audio",
            _ => continue,
        };
        lines.push(format!("a=control:{control_id}"));
    }

    Ok(lines.join("\r\n") + "\r\n")
}

fn format_bind_addr(addr: SocketAddr) -> String {
    match addr {
        SocketAddr::V4(v4) => format!("{}:{}", v4.ip(), v4.port()),
        SocketAddr::V6(v6) => format!("[{}]:{}", v6.ip(), v6.port()),
    }
}

pub(crate) fn video_codec_to_rtc(codec: &rtsp::VideoCodecParams) -> RTCRtpCodecParameters {
    use rtsp::VideoCodecParams;

    let (mime, pt, clock_rate, fmtp) = match codec {
        VideoCodecParams::H264 {
            payload_type,
            clock_rate,
            profile_level_id,
            packetization_mode,
            sps,
            pps,
        } => {
            let profile = profile_level_id.as_deref().unwrap_or("42001f");
            let mode = packetization_mode.unwrap_or(1);
            let mut fmtp = format!(
                "level-asymmetry-allowed=1;packetization-mode={};profile-level-id={}",
                mode, profile
            );
            if !sps.is_empty() && !pps.is_empty() {
                fmtp.push_str(&format!(
                    ";sprop-parameter-sets={},{}",
                    BASE64.encode(sps),
                    BASE64.encode(pps)
                ));
            }
            ("video/H264", *payload_type, *clock_rate, fmtp)
        }
        VideoCodecParams::H265 {
            payload_type,
            clock_rate,
            vps,
            sps,
            pps,
            ..
        } => {
            let mut parts = Vec::new();
            if !vps.is_empty() {
                parts.push(format!("sprop-vps={}", BASE64.encode(vps)));
            }
            if !sps.is_empty() {
                parts.push(format!("sprop-sps={}", BASE64.encode(sps)));
            }
            if !pps.is_empty() {
                parts.push(format!("sprop-pps={}", BASE64.encode(pps)));
            }
            let fmtp = if parts.is_empty() {
                String::new()
            } else {
                parts.join(";")
            };
            ("video/H265", *payload_type, *clock_rate, fmtp)
        }
        VideoCodecParams::VP8 {
            payload_type,
            clock_rate,
        } => ("video/VP8", *payload_type, *clock_rate, String::new()),
        VideoCodecParams::VP9 {
            payload_type,
            clock_rate,
        } => (
            "video/VP9",
            *payload_type,
            *clock_rate,
            "profile-id=0".to_string(),
        ),
        VideoCodecParams::AV1 {
            payload_type,
            clock_rate,
            profile_id,
        } => (
            "video/AV1",
            *payload_type,
            *clock_rate,
            format!("profile-id={}", profile_id.as_deref().unwrap_or("0")),
        ),
    };

    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: mime.to_string(),
            clock_rate,
            channels: 0,
            sdp_fmtp_line: fmtp,
            rtcp_feedback: rtsp::video_rtcp_feedback(),
        },
        payload_type: pt,
    }
}

fn audio_codec_to_rtc(codec: &rtsp::AudioCodecParams) -> RTCRtpCodecParameters {
    let mime_type = format!("audio/{}", codec.codec.to_uppercase());

    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type,
            clock_rate: codec.clock_rate,
            channels: codec.channels,
            sdp_fmtp_line: if codec.codec.to_lowercase() == "opus" {
                "minptime=10;useinbandfec=1".to_string()
            } else {
                String::new()
            },
            rtcp_feedback: vec![],
        },
        payload_type: codec.payload_type,
    }
}
