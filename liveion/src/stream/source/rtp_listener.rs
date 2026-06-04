//! RTP Listener Source
//!
//! Binds to a UDP port and listens for incoming RTP packets.
//! Implements the `StreamSource` trait.
//!
//! URL format: `rtp://0.0.0.0:5004?codec=H264&profile=42001f`
//!
//! This is functionally similar to `SdpSource` but configured via URL
//! rather than an SDP file. It always listens on a single port for
//! a single video stream.

use super::stream_config_v2::parse_rtp_url;
use super::{InternalSourceConfig, MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
use anyhow::Result;
use async_trait::async_trait;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, error, info, trace};

#[cfg(feature = "source")]
use tokio::sync::mpsc;

#[cfg(feature = "source")]
use webrtc::rtp_transceiver::RTCPFeedback;

#[cfg(feature = "source")]
use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters};

/// Channel constants matching the SourceBridge convention
const CHANNEL_VIDEO_RTP: u8 = 0;

/// Configuration parsed from the rtp:// URL
#[derive(Debug, Clone)]
struct RtpConfig {
    bind_addr: SocketAddr,
    codec: String,
    profile: String,
    clock_rate: u32,
    payload_type: u8,
}

pub struct RtpListenerSource {
    config: InternalSourceConfig,
    rtp_config: RtpConfig,
    state: Arc<RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    task_handles: Vec<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<broadcast::Sender<()>>,
    #[cfg(feature = "source")]
    rtcp_tx: Arc<RwLock<Option<mpsc::UnboundedSender<(SocketAddr, Vec<u8>)>>>>,
}

impl RtpListenerSource {
    /// Create a new RTP listener source from a URL.
    ///
    /// URL format: `rtp://host:port?codec=H264&profile=42001f`
    pub fn from_url(url: &str, config: &crate::config::SourceConfig) -> Result<Self> {
        let params = parse_rtp_url(url)?;
        let internal_config = InternalSourceConfig::from_config(config);

        let rtp_config = RtpConfig {
            bind_addr: params.bind_addr,
            codec: params.codec,
            profile: params.profile,
            clock_rate: params.clock_rate,
            payload_type: params.payload_type,
        };

        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Ok(Self {
            config: internal_config,
            rtp_config,
            state: Arc::new(RwLock::new(StreamSourceState::Initializing)),
            rtp_tx,
            state_tx,
            task_handles: Vec::new(),
            shutdown_tx: None,
            #[cfg(feature = "source")]
            rtcp_tx: Arc::new(RwLock::new(None)),
        })
    }

    async fn set_state(&self, new_state: StreamSourceState, error: Option<String>) {
        let mut state = self.state.write().await;
        let old_state = *state;

        if old_state != new_state {
            *state = new_state;
            let _ = self.state_tx.send(StateChangeEvent {
                old_state,
                new_state,
                error: error.clone(),
            });
            info!(
                "[{}] State changed: {:?} -> {:?}{}",
                self.config.stream_id,
                old_state,
                new_state,
                error.map(|e| format!(" ({})", e)).unwrap_or_default()
            );
        }
    }

    /// Main UDP receive loop
    async fn udp_receive_loop(
        stream_id: String,
        bind_addr: SocketAddr,
        rtp_tx: broadcast::Sender<MediaPacket>,
        state: Arc<RwLock<StreamSourceState>>,
        state_tx: broadcast::Sender<StateChangeEvent>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        info!("[{}] Starting RTP listener on {}", stream_id, bind_addr);

        let socket = match UdpSocket::bind(bind_addr).await {
            Ok(s) => {
                info!("[{}] UDP socket bound to {}", stream_id, bind_addr);
                s
            }
            Err(e) => {
                error!(
                    "[{}] Failed to bind UDP socket on {}: {}",
                    stream_id, bind_addr, e
                );
                let mut s = state.write().await;
                *s = StreamSourceState::Error;
                let _ = state_tx.send(StateChangeEvent {
                    old_state: StreamSourceState::Initializing,
                    new_state: StreamSourceState::Error,
                    error: Some(format!("bind failed: {}", e)),
                });
                return;
            }
        };

        // Transition to Connected
        {
            let mut s = state.write().await;
            let old = *s;
            *s = StreamSourceState::Connected;
            let _ = state_tx.send(StateChangeEvent {
                old_state: old,
                new_state: StreamSourceState::Connected,
                error: None,
            });
        }

        let mut buf = vec![0u8; 2048];
        let mut packet_count: u64 = 0;

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!(
                        "[{}] RTP listener shutting down (received {} packets)",
                        stream_id, packet_count
                    );
                    break;
                }
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, _addr)) => {
                            packet_count += 1;

                            let packet = MediaPacket::Rtp {
                                channel: CHANNEL_VIDEO_RTP,
                                data: buf[..len].to_vec().into(),
                            };

                            if rtp_tx.send(packet).is_err() {
                                // No subscribers, that's ok
                            }

                            if packet_count % 1000 == 0 {
                                trace!(
                                    "[{}] RTP listener: received {} packets",
                                    stream_id, packet_count
                                );
                            }

                            if packet_count == 1 {
                                debug!(
                                    "[{}] First RTP packet received ({} bytes)",
                                    stream_id, len
                                );
                            }
                        }
                        Err(e) => {
                            error!("[{}] UDP receive error: {}", stream_id, e);
                        }
                    }
                }
            }
        }
    }

    #[cfg(feature = "source")]
    fn build_video_codec_params(&self) -> RTCRtpCodecParameters {
        let mime_type = format!("video/{}", self.rtp_config.codec.to_uppercase());

        let sdp_fmtp_line = if self.rtp_config.codec.to_uppercase() == "H264" {
            format!(
                "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={}",
                self.rtp_config.profile
            )
        } else {
            String::new()
        };

        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type,
                clock_rate: self.rtp_config.clock_rate,
                channels: 0,
                sdp_fmtp_line,
                rtcp_feedback: vec![
                    RTCPFeedback {
                        typ: "goog-remb".to_owned(),
                        parameter: "".to_owned(),
                    },
                    RTCPFeedback {
                        typ: "ccm".to_owned(),
                        parameter: "fir".to_owned(),
                    },
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
            payload_type: self.rtp_config.payload_type,
            stats_id: String::new(),
        }
    }
}

#[async_trait]
impl StreamSource for RtpListenerSource {
    fn stream_id(&self) -> &str {
        &self.config.stream_id
    }

    fn state(&self) -> StreamSourceState {
        *self.state.blocking_read()
    }

    async fn start(&mut self) -> Result<()> {
        if !self.task_handles.is_empty() {
            anyhow::bail!("Source already started");
        }

        let (shutdown_tx, _) = broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx.clone());

        // RTCP sender task (for forwarding RTCP from WebRTC subscribers back to RTP source)
        #[cfg(feature = "source")]
        {
            let (rtcp_internal_tx, rtcp_rx) = mpsc::unbounded_channel();
            let mut rtcp_store = self.rtcp_tx.write().await;
            *rtcp_store = Some(rtcp_internal_tx);

            let stream_id = self.config.stream_id.clone();
            let shutdown_rx = shutdown_tx.subscribe();

            let rtcp_task = tokio::spawn(async move {
                Self::rtcp_sender_task(stream_id, rtcp_rx, shutdown_rx).await;
            });
            self.task_handles.push(rtcp_task);
        }

        // Main UDP receive task
        let stream_id = self.config.stream_id.clone();
        let bind_addr = self.rtp_config.bind_addr;
        let rtp_tx = self.rtp_tx.clone();
        let state = self.state.clone();
        let state_tx = self.state_tx.clone();
        let shutdown_rx = shutdown_tx.subscribe();

        let recv_task = tokio::spawn(async move {
            Self::udp_receive_loop(stream_id, bind_addr, rtp_tx, state, state_tx, shutdown_rx)
                .await;
        });
        self.task_handles.push(recv_task);

        info!(
            "[{}] RTP listener source started on {}",
            self.config.stream_id, self.rtp_config.bind_addr
        );

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        for handle in self.task_handles.drain(..) {
            let _ = handle.await;
        }

        self.set_state(StreamSourceState::Disconnected, None).await;

        info!("[{}] RTP listener source stopped", self.config.stream_id);
        Ok(())
    }

    fn subscribe_rtp(&self) -> broadcast::Receiver<MediaPacket> {
        self.rtp_tx.subscribe()
    }

    fn subscribe_state(&self) -> broadcast::Receiver<StateChangeEvent> {
        self.state_tx.subscribe()
    }

    #[cfg(feature = "source")]
    async fn get_video_codec(&self) -> Option<RTCRtpCodecParameters> {
        // Codec is known from URL configuration, always available
        Some(self.build_video_codec_params())
    }

    #[cfg(feature = "source")]
    async fn get_audio_codec(&self) -> Option<RTCRtpCodecParameters> {
        // RTP listener currently only supports video
        None
    }

    #[cfg(feature = "source")]
    async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        // For RTP listener, we don't have a specific RTCP target address
        // (the source could be anywhere). Return None for now.
        // TODO: Track the source address from received packets and send RTCP back.
        None
    }
}

#[cfg(feature = "source")]
impl RtpListenerSource {
    async fn rtcp_sender_task(
        stream_id: String,
        mut rtcp_rx: mpsc::UnboundedReceiver<(SocketAddr, Vec<u8>)>,
        mut shutdown_rx: broadcast::Receiver<()>,
    ) {
        info!("[{}] RTCP sender task started", stream_id);

        let socket = match UdpSocket::bind("0.0.0.0:0").await {
            Ok(s) => s,
            Err(e) => {
                error!("[{}] Failed to create RTCP socket: {}", stream_id, e);
                return;
            }
        };

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("[{}] RTCP sender task shutting down", stream_id);
                    break;
                }
                Some((addr, data)) = rtcp_rx.recv() => {
                    debug!(
                        "[{}] Sending RTCP to {}, size: {} bytes",
                        stream_id, addr, data.len()
                    );
                    if let Err(e) = socket.send_to(&data, addr).await {
                        error!("[{}] Failed to send RTCP: {}", stream_id, e);
                    }
                }
            }
        }
    }
}
