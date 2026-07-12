use super::PeerForward;
#[cfg(any(
    feature = "source-rtsp",
    feature = "source-sdp",
    feature = "native-source"
))]
use crate::forward::av1_repacketizer::Av1Repacketizer;
use crate::forward::rtcp::RtcpMessage;
use crate::stream::source::{MediaPacket, StateChangeEvent};
use anyhow::Result;
#[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
use anyhow::anyhow;
use rtc::shared::marshal::Marshal;
#[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
use rtc::shared::marshal::Unmarshal;
#[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
use rtc_rtp::packet::Packet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, error, info, trace, warn};

#[cfg(any(
    feature = "source-rtsp",
    feature = "source-sdp",
    feature = "native-source"
))]
const LOG_PACKET_INTERVAL: u64 = 100;

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct ChannelMapping {
    video_rtp: Option<u8>,
    video_rtcp: Option<u8>,
    audio_rtp: Option<u8>,
    audio_rtcp: Option<u8>,
}

#[allow(dead_code)]
impl ChannelMapping {
    fn new(has_video: bool, has_audio: bool) -> Self {
        match (has_video, has_audio) {
            (true, false) => Self {
                video_rtp: Some(0),
                video_rtcp: Some(1),
                audio_rtp: None,
                audio_rtcp: None,
            },
            (false, true) => Self {
                video_rtp: None,
                video_rtcp: None,
                audio_rtp: Some(0),
                audio_rtcp: Some(1),
            },
            (true, true) => Self {
                video_rtp: Some(0),
                video_rtcp: Some(1),
                audio_rtp: Some(2),
                audio_rtcp: Some(3),
            },
            (false, false) => Self {
                video_rtp: None,
                video_rtcp: None,
                audio_rtp: None,
                audio_rtcp: None,
            },
        }
    }

    fn is_video_rtp(&self, channel: u8) -> bool {
        self.video_rtp == Some(channel)
    }

    fn is_video_rtcp(&self, channel: u8) -> bool {
        self.video_rtcp == Some(channel)
    }

    fn is_audio_rtp(&self, channel: u8) -> bool {
        self.audio_rtp == Some(channel)
    }

    fn is_audio_rtcp(&self, channel: u8) -> bool {
        self.audio_rtcp == Some(channel)
    }
}
pub struct SourceBridge {
    source_id: String,
    forward: PeerForward,
    tasks: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,

    channel_mapping: ChannelMapping,
    #[cfg(any(
        feature = "source-rtsp",
        feature = "source-sdp",
        feature = "native-source"
    ))]
    av1_repacketizer: Option<Av1Repacketizer>,

    #[cfg(feature = "source")]
    rtcp_to_source_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    #[cfg(feature = "source")]
    rtcp_ready: Arc<tokio::sync::Notify>,
}

impl SourceBridge {
    pub fn new(
        source_id: String,
        forward: PeerForward,
        has_video: bool,
        has_audio: bool,
        #[cfg(any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "native-source"
        ))]
        video_codec_name: Option<String>,
    ) -> Self {
        let channel_mapping = ChannelMapping::new(has_video, has_audio);
        #[cfg(any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "native-source"
        ))]
        let av1_repacketizer = video_codec_name
            .as_deref()
            .map(|name| name.eq_ignore_ascii_case("AV1"))
            .unwrap_or(false)
            .then(Av1Repacketizer::new);

        Self {
            source_id,
            forward,
            tasks: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            shutdown_tx: None,
            channel_mapping,
            #[cfg(any(
                feature = "source-rtsp",
                feature = "source-sdp",
                feature = "native-source"
            ))]
            av1_repacketizer,
            #[cfg(feature = "source")]
            rtcp_to_source_tx: None,
            #[cfg(feature = "source")]
            rtcp_ready: Arc::new(tokio::sync::Notify::new()),
        }
    }

    #[cfg(feature = "source")]
    pub fn set_rtcp_sender(&mut self, tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>) {
        self.rtcp_to_source_tx = Some(tx);
        self.rtcp_ready.notify_one();
        info!("[{}] RTCP sender set and notified", self.source_id);
    }

    pub async fn start_bridging(
        &mut self,
        mut rtp_rx: broadcast::Receiver<MediaPacket>,
        mut state_rx: broadcast::Receiver<StateChangeEvent>,
    ) -> Result<()> {
        let (shutdown_tx, _shutdown_rx) = tokio::sync::broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx.clone());

        #[cfg(feature = "source")]
        {
            tokio::select! {
                _ = self.rtcp_ready.notified() => {
                    debug!("[{}] RTCP sender is ready", self.source_id);
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    warn!(
                        "[{}] RTCP sender timeout, keyframe requests may not work",
                        self.source_id
                    );
                }
            }
        }

        #[cfg(any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "native-source"
        ))]
        let forward_clone = self.forward.clone();
        let source_id_clone = self.source_id.clone();
        let mut shutdown_rx1 = shutdown_tx.subscribe();
        let channel_mapping = self.channel_mapping;
        #[cfg(any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "native-source"
        ))]
        let mut av1_repacketizer = self.av1_repacketizer.take();

        let rtp_task = tokio::spawn(async move {
            info!(
                "[{}] RTP bridging task started with mapping: {:?}",
                source_id_clone, channel_mapping
            );
            let mut packet_count = 0u64;
            #[cfg(any(
                feature = "source-rtsp",
                feature = "source-sdp",
                feature = "native-source"
            ))]
            let mut video_count = 0u64;
            #[cfg(not(any(
                feature = "source-rtsp",
                feature = "source-sdp",
                feature = "native-source"
            )))]
            let video_count = 0u64;
            #[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
            let mut audio_count = 0u64;
            #[cfg(not(any(feature = "source-rtsp", feature = "source-sdp")))]
            let audio_count = 0u64;
            #[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
            let mut video_dropped = 0u64;
            #[cfg(not(any(feature = "source-rtsp", feature = "source-sdp")))]
            let video_dropped = 0u64;

            loop {
                tokio::select! {
                    _ = shutdown_rx1.recv() => {
                        info!(
                            "[{}] RTP task shutting down, forwarded {} packets (video: {}, audio: {}, dropped: {})",
                            source_id_clone, packet_count, video_count, audio_count, video_dropped
                        );
                        break;
                    }
                    result = rtp_rx.recv() => {
                        match result {
                            Ok(packet) => {
                                packet_count += 1;

                                let inject_result: anyhow::Result<()> = match packet {
                                    #[cfg(feature = "native-source")]
                                    MediaPacket::RtpPacket(packet) => {
                                        video_count += 1;
                                        if video_count % LOG_PACKET_INTERVAL == 1 {
                                            debug!(
                                                "[{}] Forwarding video packet #{}, size: {}",
                                                source_id_clone, video_count, packet.payload.len()
                                            );
                                        }
                                        forward_clone.inject_video_rtp_packet(packet).await.map_err(|e| anyhow::anyhow!("{:?}", e))
                                    }
                                    #[cfg(any(feature = "source-rtsp", feature = "source-sdp"))]
                                    MediaPacket::Rtp { channel, data, .. } => {
                                        if channel_mapping.is_video_rtp(channel) {
                                            video_count += 1;
                                            if video_count % LOG_PACKET_INTERVAL == 1 {
                                                debug!(
                                                    "[{}] Forwarding video packet #{}, size: {}",
                                                    source_id_clone, video_count, data.len()
                                                );
                                            }
                                            if let Some(ref mut repacketizer) = av1_repacketizer {
                                                match Packet::unmarshal(&mut &data[..]) {
                                                    Ok(packet) => {
                                                        match repacketizer.process(&packet) {
                                                            Ok(packets) => {
                                                                for packet in packets {
                                                                    if let Err(e) = forward_clone.inject_video_rtp_packet(std::sync::Arc::new(packet)).await {
                                                                        error!("[{}] Failed to inject repacketized AV1 RTP packet: {:?}", source_id_clone, e);
                                                                    }
                                                                }
                                                                Ok(())
                                                            }
                                                            Err(e) => {
                                                                video_dropped += 1;
                                                                warn!("[{}] AV1 repacketization failed, dropping packet: {}", source_id_clone, e);
                                                                Ok(())
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        video_dropped += 1;
                                                        warn!("[{}] Failed to unmarshal AV1 RTP packet, dropping: {}", source_id_clone, e);
                                                        Ok(())
                                                    }
                                                }
                                            } else {
                                                forward_clone.inject_video_rtp(&data).await.map_err(|e| anyhow!("{:?}", e))
                                            }
                                        } else if channel_mapping.is_audio_rtp(channel) {
                                            audio_count += 1;
                                            if audio_count % LOG_PACKET_INTERVAL == 1 {
                                                debug!(
                                                    "[{}] Forwarding audio packet #{}, size: {}",
                                                    source_id_clone, audio_count, data.len()
                                                );
                                            }
                                            forward_clone.inject_audio_rtp(&data).await.map_err(|e| anyhow!("{:?}", e))
                                        } else if channel_mapping.is_video_rtcp(channel) || channel_mapping.is_audio_rtcp(channel) {
                                            trace!(
                                                "[{}] Received RTCP packet on channel {}",
                                                source_id_clone, channel
                                            );
                                            Ok(())
                                        } else {
                                            warn!(
                                                "[{}] Unknown channel: {}",
                                                source_id_clone, channel
                                            );
                                            Ok(())
                                        }
                                    }
                                    // The `source` feature alone enables no
                                    // concrete source implementation; the enum
                                    // carries a placeholder variant in that
                                    // configuration, so we just ignore it.
                                    #[cfg(not(any(
                                        feature = "source-rtsp",
                                        feature = "source-sdp",
                                        feature = "native-source"
                                    )))]
                                    _ => Ok(()),
                                };

                                if let Err(e) = inject_result {
                                    error!(
                                        "[{}] Failed to inject RTP packet #{}: {:?}",
                                        source_id_clone, packet_count, e
                                    );
                                }

                                if packet_count.is_multiple_of(1000) {
                                    debug!(
                                        "[{}] Forwarded {} packets (video: {}, audio: {})",
                                        source_id_clone, packet_count, video_count, audio_count
                                    );
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                warn!(
                                    "[{}] Lagged, skipped {} packets",
                                    source_id_clone, skipped
                                );
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                info!("[{}] Source channel closed", source_id_clone);
                                break;
                            }
                        }
                    }
                }
            }
        });

        let source_id_clone = self.source_id.clone();
        let mut shutdown_rx2 = shutdown_tx.subscribe();

        let state_task = tokio::spawn(async move {
            info!("[{}] State monitoring task started", source_id_clone);

            loop {
                tokio::select! {
                    _ = shutdown_rx2.recv() => {
                        info!("[{}] State task shutting down", source_id_clone);
                        break;
                    }
                    result = state_rx.recv() => {
                        match result {
                            Ok(event) => {
                                info!(
                                    "[{}] State changed: {:?} -> {:?}",
                                    source_id_clone, event.old_state, event.new_state
                                );

                                if let Some(error) = event.error {
                                    error!(
                                        "[{}] State change error: {}",
                                        source_id_clone, error
                                    );
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {
                                warn!("[{}] State events lagged", source_id_clone);
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                info!("[{}] State channel closed", source_id_clone);
                                break;
                            }
                        }
                    }
                }
            }
        });

        #[cfg(feature = "source")]
        let rtcp_task = {
            let forward_clone = self.forward.clone();
            let source_id_clone = self.source_id.clone();
            let rtcp_tx = self.rtcp_to_source_tx.clone();
            let shutdown_rx3 = shutdown_tx.subscribe();

            tokio::spawn(async move {
                Self::rtcp_handler(source_id_clone, forward_clone, rtcp_tx, shutdown_rx3).await;
            })
        };

        #[cfg(feature = "source")]
        let sender_report_task = {
            let forward_clone = self.forward.clone();
            let source_id_clone = self.source_id.clone();
            let shutdown_rx4 = shutdown_tx.subscribe();

            tokio::spawn(async move {
                Self::sender_report_loop(source_id_clone, forward_clone, shutdown_rx4).await;
            })
        };

        let mut tasks = self.tasks.lock().await;
        tasks.push(rtp_task);
        tasks.push(state_task);

        #[cfg(feature = "source")]
        {
            tasks.push(rtcp_task);
            tasks.push(sender_report_task);
        }

        info!(
            "[{}] Bridge started with {} tasks",
            self.source_id,
            tasks.len()
        );
        Ok(())
    }

    #[cfg(feature = "source")]
    async fn rtcp_handler(
        source_id: String,
        forward: PeerForward,
        rtcp_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) {
        info!("[{}] RTCP handler started", source_id);
        let mut rtcp_rx = forward.internal.publish_rtcp_channel.subscribe();

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("[{}] RTCP handler shutting down", source_id);
                    break;
                }

                result = rtcp_rx.recv() => {
                    let (rtcp_msg, ssrc) = match result {
                        Ok(pair) => pair,
                        Err(e) => {
                            error!("[{}] RTCP receiver error: {}", source_id, e);
                            break;
                        }
                    };

                    debug!(
                        "[{}] Received RTCP {:?} for SSRC {}",
                        source_id, rtcp_msg, ssrc
                    );

                    match rtcp_msg {
                        RtcpMessage::PictureLossIndication => {
                            debug!(
                                "[{}] Received PLI for SSRC {}, requesting keyframe",
                                source_id, ssrc
                            );

                            let Some(tx) = rtcp_tx.as_ref() else {
                                warn!(
                                    "[{}] RTCP sender is None, cannot forward PLI for SSRC {}",
                                    source_id, ssrc
                                );
                                continue;
                            };

                            let pli = rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication {
                                sender_ssrc: 0,
                                media_ssrc: ssrc,
                            };

                            match pli.marshal() {
                                Ok(buf) => {
                                    if let Err(e) = tx.send(buf.to_vec()) {
                                        error!(
                                            "[{}] Failed to send PLI to source: {}",
                                            source_id, e
                                        );
                                    } else {
                                        debug!("[{}] PLI sent to source successfully", source_id);
                                    }
                                }
                                Err(e) => {
                                    error!(
                                        "[{}] Failed to marshal PLI: {}",
                                        source_id, e
                                    );
                                }
                            }
                        }

                        RtcpMessage::_FullIntraRequest => {
                            info!(
                                "[{}] Received FIR for SSRC {}, requesting keyframe",
                                source_id, ssrc
                            );

                            let Some(tx) = rtcp_tx.as_ref() else {
                                warn!(
                                    "[{}] RTCP sender is None, cannot forward FIR for SSRC {}",
                                    source_id, ssrc
                                );
                                continue;
                            };

                            let fir = rtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest {
                                sender_ssrc: 0,
                                media_ssrc: ssrc,
                                fir: vec![],
                            };

                            match fir.marshal() {
                                Ok(buf) => {
                                    if let Err(e) = tx.send(buf.to_vec()) {
                                        error!(
                                            "[{}] Failed to send FIR to source: {}",
                                            source_id, e
                                        );
                                    } else {
                                        debug!("[{}] FIR sent to source successfully", source_id);
                                    }
                                }
                                Err(e) => {
                                    error!(
                                        "[{}] Failed to marshal FIR: {}",
                                        source_id, e
                                    );
                                }
                            }
                        }

                        RtcpMessage::_SliceLossIndication => {
                            debug!(
                                "[{}] Received SLI for SSRC {} (not forwarded)",
                                source_id, ssrc
                            );
                        }
                    }
                }
            }
        }

        info!("[{}] RTCP handler stopped", source_id);
    }

    #[cfg(feature = "source")]
    async fn sender_report_loop(
        source_id: String,
        forward: PeerForward,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) {
        info!("[{}] Sender Report task started", source_id);
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("[{}] Sender Report task shutting down", source_id);
                    break;
                }
                _ = interval.tick() => {
                    let tracks = forward.internal.publish_tracks.read().await;

                    for track in tracks.iter() {
                        let forward_info = forward.info().await;

                        for subscribe_info in &forward_info.subscribe_session_infos {
                            if let Some(peer) = forward.get_subscribe_peer(&subscribe_info.id).await {
                                for transceiver in peer.get_transceivers().await {
                                    if let Ok(Some(sender)) = transceiver.sender().await {
                                        let track_local = sender.track();
                                        if let Some(sr) = track.generate_sender_report()
                                            && track_local.write_rtcp(vec![sr]).await.is_err()
                                        {
                                            debug!(
                                                "[{}] Failed to send SR to {}",
                                                source_id, subscribe_info.id
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        trace!(
                            "[{}] Sent SR to {} subscribers",
                            source_id,
                            forward_info.subscribe_session_infos.len()
                        );
                    }
                }
            }
        }

        info!("[{}] Sender Report task stopped", source_id);
    }

    pub async fn stop(&mut self) -> Result<()> {
        info!("[{}] Stopping bridge", self.source_id);

        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        let mut tasks = self.tasks.lock().await;
        for task in tasks.drain(..) {
            if let Err(e) = task.await {
                warn!("[{}] Task join error: {:?}", self.source_id, e);
            }
        }

        info!("[{}] Bridge stopped", self.source_id);
        Ok(())
    }
}

impl Drop for SourceBridge {
    fn drop(&mut self) {
        debug!("[{}] Dropping bridge", self.source_id);

        if let Some(tx) = &self.shutdown_tx {
            let _ = tx.send(());
        }
    }
}

#[cfg(all(test, any(feature = "source-rtsp", feature = "source-sdp")))]
mod integration_tests {
    use super::*;
    use crate::forward::PeerForward;
    use bytes::BytesMut;
    use rtc::rtp::header::Header;
    use rtc::rtp::packet::Packet;
    use rtc::rtp_transceiver::rtp_sender::{
        RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RtpCodecKind,
    };
    use rtc::shared::marshal::MarshalSize;
    use std::time::Duration;
    use tokio::sync::broadcast;

    fn build_av1_obu(payload: &[u8]) -> bytes::Bytes {
        let mut obu = BytesMut::with_capacity(1 + payload.len());
        obu.extend_from_slice(&[0x30]); // Frame OBU, no size field
        obu.extend_from_slice(payload);
        obu.freeze()
    }

    fn build_av1_rtp_packet(seq: u16, timestamp: u32, marker: bool, obu: &[u8]) -> Packet {
        let mut payload = BytesMut::with_capacity(1 + obu.len());
        payload.extend_from_slice(&[0x10]); // aggregation header: W=1
        payload.extend_from_slice(obu);

        Packet {
            header: Header {
                version: 2,
                marker,
                payload_type: 96,
                sequence_number: seq,
                timestamp,
                ssrc: 0xDEADBEEF,
                ..Default::default()
            },
            payload: payload.freeze(),
        }
    }

    #[tokio::test]
    async fn bridge_repacketizes_oversized_av1_rtp() {
        let forward = PeerForward::new(
            "av1-repacketizer-test",
            vec![],
            vec![],
            #[cfg(feature = "source")]
            None,
            api::strategy::Strategy::default(),
        );

        let codec = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/AV1".to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: vec![
                    RTCPFeedback {
                        typ: "nack".to_owned(),
                        parameter: "".to_owned(),
                    },
                    RTCPFeedback {
                        typ: "nack".to_owned(),
                        parameter: "pli".to_owned(),
                    },
                ],
            },
            payload_type: 96,
        };

        forward
            .add_virtual_track(RtpCodecKind::Video, codec)
            .await
            .expect("add virtual track");

        let mut track_subscribe = {
            let tracks = forward.internal.publish_tracks.read().await;
            let track = tracks
                .iter()
                .find(|t| t.kind() == RtpCodecKind::Video)
                .expect("video track exists");
            track.subscribe()
        };

        let mut bridge = SourceBridge::new(
            "av1-test".to_owned(),
            forward,
            true,
            false,
            Some("AV1".to_owned()),
        );

        let (rtcp_tx, _rtcp_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        bridge.set_rtcp_sender(rtcp_tx);

        let (rtp_tx, rtp_rx) = broadcast::channel(16);
        let (_state_tx, state_rx) = broadcast::channel(4);

        bridge
            .start_bridging(rtp_rx, state_rx)
            .await
            .expect("start bridging");

        // Send a single AV1 temporal unit larger than 1200 bytes.
        let obu = build_av1_obu(&[0xAB; 3000]);
        let packet = build_av1_rtp_packet(1, 1000, true, &obu);
        let raw_packet = {
            let mut buf = vec![0u8; packet.marshal_size()];
            rtc::shared::marshal::Marshal::marshal_to(&packet, &mut buf).expect("marshal packet");
            buf
        };

        rtp_tx
            .send(MediaPacket::Rtp {
                channel: 0,
                data: raw_packet.into(),
            })
            .expect("send rtp");

        // Wait for repacketized packets to arrive on the virtual track.
        let mut received = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let timeout = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(timeout, track_subscribe.recv()).await {
                Ok(Ok(pkt)) => {
                    let is_marker = pkt.header.marker;
                    received.push(pkt);
                    if is_marker {
                        break;
                    }
                }
                Ok(Err(_)) => break,
                Err(_) => panic!("timeout waiting for repacketized AV1 packets"),
            }
        }

        assert!(
            !received.is_empty(),
            "should receive repacketized AV1 packets"
        );
        assert!(
            received.len() > 1,
            "oversized temporal unit should be split into multiple packets"
        );

        for (i, pkt) in received.iter().enumerate() {
            assert!(
                pkt.payload.len() <= 1200,
                "packet {} payload {} exceeds 1200 bytes",
                i,
                pkt.payload.len()
            );
        }

        assert!(received.last().unwrap().header.marker);

        bridge.stop().await.expect("stop bridge");
    }
}
