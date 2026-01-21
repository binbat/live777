use super::PeerForward;
use crate::forward::rtcp::RtcpMessage;
use crate::stream::source::{MediaPacket, StateChangeEvent};
use anyhow::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, error, info, trace, warn};
use webrtc::util::Marshal;

const LOG_PACKET_INTERVAL: u64 = 100;
const CHANNEL_VIDEO_RTP: u8 = 0;
const CHANNEL_VIDEO_RTCP: u8 = 1;
const CHANNEL_AUDIO_RTP: u8 = 2;
const CHANNEL_AUDIO_RTCP: u8 = 3;

pub struct SourceBridge {
    source_id: String,
    forward: Arc<PeerForward>,
    tasks: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,

    #[cfg(feature = "source")]
    rtcp_to_source_tx: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    #[cfg(feature = "source")]
    rtcp_ready: Arc<tokio::sync::Notify>,
}

impl SourceBridge {
    pub fn new(source_id: String, forward: Arc<PeerForward>) -> Self {
        Self {
            source_id,
            forward,
            tasks: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            shutdown_tx: None,
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

        let forward_clone = self.forward.clone();
        let source_id_clone = self.source_id.clone();
        let mut shutdown_rx1 = shutdown_tx.subscribe();

        let rtp_task = tokio::spawn(async move {
            info!("[{}] RTP bridging task started", source_id_clone);
            let mut packet_count = 0u64;
            let mut video_count = 0u64;
            let mut audio_count = 0u64;

            loop {
                tokio::select! {
                    _ = shutdown_rx1.recv() => {
                        info!(
                            "[{}] RTP task shutting down, forwarded {} packets (video: {}, audio: {})",
                            source_id_clone, packet_count, video_count, audio_count
                        );
                        break;
                    }
                    result = rtp_rx.recv() => {
                        match result {
                            Ok(packet) => {
                                packet_count += 1;

                                let inject_result = match packet {
                                    MediaPacket::Rtp { channel, data, .. } => {
                                        match channel {
                                            CHANNEL_VIDEO_RTP => {
                                                video_count += 1;
                                                if video_count % LOG_PACKET_INTERVAL == 1 {
                                                    debug!(
                                                        "[{}] Forwarding video packet #{}, size: {}",
                                                        source_id_clone, video_count, data.len()
                                                    );
                                                }
                                                forward_clone.inject_video_rtp(&data).await
                                            }
                                            CHANNEL_AUDIO_RTP => {
                                                audio_count += 1;
                                                if audio_count % LOG_PACKET_INTERVAL == 1 {
                                                    debug!(
                                                        "[{}] Forwarding audio packet #{}, size: {}",
                                                        source_id_clone, audio_count, data.len()
                                                    );
                                                }
                                                forward_clone.inject_audio_rtp(&data).await
                                            }
                                            CHANNEL_VIDEO_RTCP | CHANNEL_AUDIO_RTCP => {
                                                trace!(
                                                    "[{}] Received RTCP packet on channel {}",
                                                    source_id_clone, channel
                                                );
                                                Ok(())
                                            }
                                            _ => {
                                                warn!(
                                                    "[{}] Unknown channel: {}",
                                                    source_id_clone, channel
                                                );
                                                Ok(())
                                            }
                                        }
                                    }
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
        forward: Arc<PeerForward>,
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
                            info!(
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

                            let pli = webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication {
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

                        RtcpMessage::FullIntraRequest => {
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

                            let fir = webrtc::rtcp::payload_feedbacks::full_intra_request::FullIntraRequest {
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

                        RtcpMessage::SliceLossIndication => {
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
        forward: Arc<PeerForward>,
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
                        if let Some(sr) = track.generate_sender_report() {
                            let forward_info = forward.info().await;

                            for subscribe_info in &forward_info.subscribe_session_infos {
                                if let Some(peer) = forward.get_subscribe_peer(&subscribe_info.id).await
                                    && let Err(e) = peer.write_rtcp(std::slice::from_ref(&sr)).await {
                                        debug!(
                                            "[{}] Failed to send SR to {}: {}",
                                            source_id, subscribe_info.id, e
                                        );
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
