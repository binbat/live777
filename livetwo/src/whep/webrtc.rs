use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, anyhow};
use libwish::Client;
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::shared::marshal::{Marshal, MarshalSize};
use tokio::sync::{Mutex, Notify, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceGatheringState, RTCIceServer, RTCPeerConnectionState,
    RTCSessionDescription,
};
use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};

use crate::utils;
use crate::utils::stats::RtcpStats;

/// DataChannel label used to join liveion's WHEP group for bidirectional control messaging.
const DATA_CHANNEL_LABEL: &str = "control";

/// Connection-level options for an outgoing WHEP peer.
#[derive(Clone)]
pub struct WhepPeerOptions {
    /// ICE servers used for gathering the offer; an empty list means host
    /// candidates only. The upstream's own ice-servers from the WHEP
    /// response Link headers replace this list via `set_configuration` once
    /// the answer arrives.
    pub ice_servers: Vec<RTCIceServer>,
    /// UDP sockets ICE binds to. Empty falls back to the
    /// `LIVE777_WEBRTC_ICE_UDP_ADDRS` resolution (env override or OS
    /// default allocation).
    pub ice_udp_addrs: Vec<std::net::SocketAddr>,
    /// Create the "control" DataChannel joining liveion's WHEP group for
    /// bidirectional control messaging. Pull-only consumers (e.g. the
    /// liveion WHEP source) should disable it.
    pub control_channel: bool,
}

impl Default for WhepPeerOptions {
    fn default() -> Self {
        Self {
            ice_servers: iceserver::default_rtc_ice_servers(),
            ice_udp_addrs: Vec::new(),
            control_channel: true,
        }
    }
}

/// Set up a WHEP client peer with `options`, POST the offer and apply the
/// answer; see [`WhepPeerOptions`] for the connection-level knobs.
#[allow(clippy::too_many_arguments)]
pub async fn setup_whep_peer(
    ct: CancellationToken,
    client: &mut Client,
    video_send: mpsc::Sender<Vec<u8>>,
    audio_send: mpsc::Sender<Vec<u8>>,
    codec_info: Arc<Mutex<rtsp::CodecInfo>>,
    options: WhepPeerOptions,
    state_tx: Option<watch::Sender<RTCPeerConnectionState>>,
    video_mime_tx: Option<watch::Sender<Option<String>>>,
) -> Result<(
    Arc<dyn PeerConnection>,
    RTCSessionDescription,
    Arc<RtcpStats>,
    mpsc::UnboundedReceiver<Vec<u8>>,
    mpsc::UnboundedSender<Vec<u8>>,
)> {
    let gather_complete = Arc::new(Notify::new());
    let (dc_recv_tx, dc_recv_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (dc_send_tx, dc_send_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let peer = create_peer(
        ct,
        video_send,
        audio_send,
        codec_info.clone(),
        options,
        gather_complete.clone(),
        dc_recv_tx,
        dc_send_rx,
        state_tx,
        video_mime_tx,
    )
    .await?;

    utils::webrtc::setup_connection(peer.clone(), client, gather_complete).await?;

    let answer = peer
        .remote_description()
        .await
        .ok_or_else(|| anyhow!("No remote description"))?;

    let stats = Arc::new(RtcpStats::new());

    Ok((peer, answer, stats, dc_recv_rx, dc_send_tx))
}

/// Forward RTCP feedback (e.g. PLI from downstream subscribers) to the
/// remote track of a WHEP peer, so it reaches the upstream publisher.
///
/// `data` is a marshalled RTCP packet buffer as received from the RTP side;
/// packets without a destination SSRC, or whose destination matches no
/// remote track, are dropped.
pub async fn forward_rtcp_to_peer(data: &[u8], peer: &Arc<dyn PeerConnection>) {
    let mut cursor = Cursor::new(data);
    match rtc_rtcp::packet::unmarshal(&mut cursor) {
        Ok(packets) => {
            for packet in packets {
                crate::whip::log_rtcp_feedback_packet("WHEP peer RTCP", packet.as_ref());
                forward_rtcp_packet_to_peer(packet, peer).await;
            }
        }
        Err(e) => {
            warn!("Failed to parse RTCP: {}", e);
        }
    }
}

async fn forward_rtcp_packet_to_peer(
    packet: Box<dyn rtc_rtcp::packet::Packet + Send + Sync>,
    peer: &Arc<dyn PeerConnection>,
) {
    let destination_ssrcs = packet.destination_ssrc();
    if destination_ssrcs.is_empty() {
        debug!("Dropping RTCP packet without destination SSRC");
        return;
    }

    let receivers = peer.get_receivers().await;
    let mut target_track = None;
    for receiver in receivers {
        let track = receiver.track().clone();
        let track_ssrcs = track.ssrcs().await;
        if destination_ssrcs
            .iter()
            .any(|destination| track_ssrcs.contains(destination))
        {
            target_track = Some(track);
            break;
        }
    }

    let Some(track) = target_track else {
        warn!(
            "Dropping RTCP packet for unknown WHEP destination SSRC(s): {:?}",
            destination_ssrcs
        );
        return;
    };

    if let Err(error) = track.write_rtcp(vec![packet]).await {
        warn!("Failed to forward RTCP packet to WHEP track: {}", error);
    }
}

#[derive(Clone)]
struct WhepTrackHandler {
    ct: CancellationToken,
    gather_complete: Arc<Notify>,
    video_send: Option<mpsc::Sender<Vec<u8>>>,
    audio_send: Option<mpsc::Sender<Vec<u8>>>,
    codec_info: Arc<Mutex<rtsp::CodecInfo>>,
    state_tx: Option<watch::Sender<RTCPeerConnectionState>>,
    video_mime_tx: Option<watch::Sender<Option<String>>>,
    /// Cumulative count of dropped video RTP packets due to a full channel.
    video_drop_count: Arc<AtomicU64>,
    /// Cumulative count of dropped audio RTP packets due to a full channel.
    audio_drop_count: Arc<AtomicU64>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for WhepTrackHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        info!("WHEP connection state changed: {}", state);
        if let Some(tx) = &self.state_tx {
            let _ = tx.send(state);
        }
        match state {
            RTCPeerConnectionState::Failed => {
                self.ct.cancel();
                warn!("WHEP connection closed due to failure");
            }
            RTCPeerConnectionState::Closed => {
                self.ct.cancel();
                info!("WHEP connection closed normally");
            }
            _ => debug!("WHEP connection state: {}", state),
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let kind = track.kind().await;
        let ssrcs = track.ssrcs().await;
        let track_id = track.track_id().await;
        info!(
            "WHEP on_track: kind={}, ssrcs={:?}, id={}",
            kind, ssrcs, track_id
        );

        // Report the negotiated mime type as soon as the track is known so
        // callers (e.g. the rsmpeg probe) can select a decoder without waiting
        // for the first RTP packet. Do NOT populate codec_info here: the
        // payload type is not known until the first RTP packet arrives, and
        // using 0 would cause SDP filtering to discard all payload formats and
        // produce an invalid "m=video 9 RTP/AVP" line.
        let first_ssrc = ssrcs.first().copied().unwrap_or(0);
        let on_track_codec = track.codec(first_ssrc).await;
        if let Some(codec) = &on_track_codec {
            debug!(
                "WHEP on_track codec: kind={}, mime={}",
                kind, codec.mime_type
            );
            if kind == RtpCodecKind::Video
                && let Some(tx) = &self.video_mime_tx
            {
                let _ = tx.send(Some(codec.mime_type.clone()));
            }
        }

        // Select the appropriate sender channel
        let sender = match kind {
            RtpCodecKind::Video => self.video_send.clone(),
            RtpCodecKind::Audio => self.audio_send.clone(),
            _ => None,
        };

        if let Some(sender) = sender {
            let track_clone = track.clone();
            let codec_info = self.codec_info.clone();
            let video_mime_tx = self.video_mime_tx.clone();
            let drop_count = match kind {
                RtpCodecKind::Video => self.video_drop_count.clone(),
                RtpCodecKind::Audio => self.audio_drop_count.clone(),
                _ => unreachable!("sender is only Some for video/audio"),
            };
            let on_track_codec = on_track_codec.clone();
            let ct = self.ct.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 1500];
                let mut first_packet = true;
                loop {
                    tokio::select! {
                        event = track_clone.poll() => {
                            match event {
                                Some(TrackRemoteEvent::OnRtpPacket(rtp_packet)) => {
                                    if first_packet {
                                        let first_packet_codec = track_clone
                                            .codec(rtp_packet.header.ssrc)
                                            .await
                                            .or(on_track_codec.clone());
                                        if let Some(codec) = first_packet_codec {
                                            let codec_params =
                                                rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters {
                                                    rtp_codec: codec.clone(),
                                                    payload_type: rtp_packet.header.payload_type,
                                                };
                                            let mut info = codec_info.lock().await;
                                            match kind {
                                                RtpCodecKind::Video => {
                                                    info.video_codec = Some(codec_params.clone());
                                                    // Only report the MIME type from the first
                                                    // packet if on_track did not already provide
                                                    // it, avoiding duplicate watch-channel updates.
                                                    if on_track_codec.is_none() && let Some(tx) = &video_mime_tx {
                                                        let _ = tx.send(Some(codec.mime_type.clone()));
                                                    }
                                                }
                                                RtpCodecKind::Audio => {
                                                    info.audio_codec = Some(codec_params);
                                                }
                                                _ => {}
                                            }
                                        }
                                        first_packet = false;
                                    }
                                    let size = rtp_packet.marshal_size();
                                    if size > buf.len() {
                                        warn!("WHEP: RTP packet too large ({} bytes)", size);
                                        continue;
                                    }
                                    if let Err(e) = rtp_packet.marshal_to(&mut buf[..size]) {
                                        warn!("WHEP: Failed to marshal RTP packet: {}", e);
                                        continue;
                                    }
                                    match sender.try_send(buf[..size].to_vec()) {
                                        Ok(()) => {}
                                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                            // Decoder cannot keep up; drop the packet to
                                            // avoid unbounded memory growth.
                                            let dropped = drop_count.fetch_add(1, Ordering::Relaxed) + 1;
                                            if dropped <= 10 || dropped % 100 == 0 {
                                                warn!(
                                                    "WHEP: {} channel full, dropping packet (total dropped {})",
                                                    kind, dropped
                                                );
                                            } else {
                                                debug!(
                                                    "WHEP: {} channel full, dropping packet (total dropped {})",
                                                    kind, dropped
                                                );
                                            }
                                        }
                                        Err(_) => {
                                            debug!("WHEP: {} channel receiver dropped, stopping", kind);
                                            break;
                                        }
                                    }
                                }
                                Some(TrackRemoteEvent::OnEnded) => {
                                    info!("WHEP: {} track ended", kind);
                                    break;
                                }
                                Some(TrackRemoteEvent::OnRtcpPacket(packets)) => {
                                    debug!("WHEP: Received {} RTCP packets for {}", packets.len(), kind);
                                }
                                None => {
                                    debug!("WHEP: {} track poll returned None", kind);
                                    break;
                                }
                                _ => {}
                            }
                        }
                        _ = ct.cancelled() => {
                            info!("WHEP: {} RTP reader cancelled", kind);
                            break;
                        }
                    }
                }
                info!("WHEP: {} RTP reader stopped", kind);
            });
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            info!("WHEP ICE gathering complete");
            self.gather_complete.notify_one();
        }
    }
}

fn setup_data_channel_loop(
    dc: Arc<dyn DataChannel>,
    dc_recv_tx: mpsc::UnboundedSender<Vec<u8>>,
    mut dc_send_rx: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    tokio::spawn(async move {
        // Wait for OnOpen
        loop {
            match dc.poll().await {
                Some(DataChannelEvent::OnOpen) => {
                    info!("whepfrom: DataChannel opened");
                    break;
                }
                Some(DataChannelEvent::OnClose) => {
                    info!("whepfrom: DataChannel closed before open");
                    return;
                }
                None => {
                    info!("whepfrom: DataChannel poll ended before open");
                    return;
                }
                _ => {}
            }
        }

        loop {
            tokio::select! {
                event = dc.poll() => match event {
                    Some(DataChannelEvent::OnMessage(msg))
                        if dc_recv_tx.send(msg.data.to_vec()).is_err() =>
                    {
                        debug!("whepfrom: DataChannel recv channel closed");
                        break;
                    }
                    Some(DataChannelEvent::OnClose) => {
                        info!("whepfrom: DataChannel closed");
                        break;
                    }
                    None => {
                        info!("whepfrom: DataChannel poll ended");
                        break;
                    }
                    _ => {}
                },
                msg = dc_send_rx.recv() => match msg {
                    Some(data) => {
                        if let Err(e) = dc.send(bytes::BytesMut::from(&data[..])).await {
                            warn!("whepfrom: DataChannel send failed: {}", e);
                            break;
                        }
                    }
                    None => {
                        info!("whepfrom: DataChannel send channel closed");
                        break;
                    }
                },
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
async fn create_peer(
    ct: CancellationToken,
    video_send: mpsc::Sender<Vec<u8>>,
    audio_send: mpsc::Sender<Vec<u8>>,
    codec_info: Arc<Mutex<rtsp::CodecInfo>>,
    options: WhepPeerOptions,
    gather_complete: Arc<Notify>,
    dc_recv_tx: mpsc::UnboundedSender<Vec<u8>>,
    dc_send_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    state_tx: Option<watch::Sender<RTCPeerConnectionState>>,
    video_mime_tx: Option<watch::Sender<Option<String>>>,
) -> Result<Arc<dyn PeerConnection>> {
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs()?;

    let handler: Arc<dyn PeerConnectionEventHandler> = Arc::new(WhepTrackHandler {
        ct,
        gather_complete,
        video_send: Some(video_send),
        audio_send: Some(audio_send),
        codec_info,
        state_tx,
        video_mime_tx,
        video_drop_count: Arc::new(AtomicU64::new(0)),
        audio_drop_count: Arc::new(AtomicU64::new(0)),
    });

    let config = RTCConfigurationBuilder::new()
        .with_ice_servers(options.ice_servers.clone())
        .build();

    // Caller-provided UDP addrs win; otherwise fall back to the
    // LIVE777_WEBRTC_ICE_UDP_ADDRS resolution (env override or OS default).
    let udp_addrs = if options.ice_udp_addrs.is_empty() {
        utils::webrtc::ice_udp_addrs()
    } else {
        options.ice_udp_addrs
    };

    let peer: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::<std::net::SocketAddr>::new()
            .with_media_engine(media_engine)
            .with_handler(handler)
            .with_udp_addrs(udp_addrs)
            .with_configuration(config)
            .build()
            .await
            .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?,
    );

    if options.control_channel {
        // Create DataChannel to participate in liveion's WHEP group
        let dc = peer
            .create_data_channel(DATA_CHANNEL_LABEL, None)
            .await
            .map_err(|e| anyhow!("create_data_channel failed: {:?}", e))?;

        // Start the data channel polling loop
        setup_data_channel_loop(dc, dc_recv_tx, dc_send_rx);
    }

    peer.add_transceiver_from_kind(
        RtpCodecKind::Video,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            streams: vec![],
            send_encodings: vec![],
        }),
    )
    .await
    .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    peer.add_transceiver_from_kind(
        RtpCodecKind::Audio,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            streams: vec![],
            send_encodings: vec![],
        }),
    )
    .await
    .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    Ok(peer)
}
