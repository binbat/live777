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

#[cfg(feature = "source")]
type RtcpSender = Arc<RwLock<Option<mpsc::UnboundedSender<(SocketAddr, Vec<u8>)>>>>;

#[cfg(feature = "source")]
type ParsedSdp = (Vec<(u8, u16)>, SdpMediaInfo, bool);

struct UdpReceiverContext {
    stream_id: String,
    channel: u8,
    port: u16,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state: Arc<RwLock<StreamSourceState>>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    is_ipv6: bool,
}

pub struct SdpSource {
    config: InternalSourceConfig,
    sdp_content: String,
    state: Arc<RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    task_handles: Vec<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<tokio::sync::broadcast::Sender<()>>,
    #[cfg(feature = "source")]
    media_info: Arc<RwLock<Option<SdpMediaInfo>>>,
    #[cfg(feature = "source")]
    rtcp_tx: RtcpSender,
}

#[cfg(feature = "source")]
#[derive(Clone, Debug)]
struct SdpMediaInfo {
    video_codec: Option<VideoCodecInfo>,
    audio_codec: Option<AudioCodecInfo>,
    video_rtcp_addr: Option<SocketAddr>,
    audio_rtcp_addr: Option<SocketAddr>,
}

#[cfg(feature = "source")]
#[derive(Clone, Debug)]
struct VideoCodecInfo {
    codec_name: String,
    clock_rate: u32,
    payload_type: u8,
}

#[cfg(feature = "source")]
#[derive(Clone, Debug)]
struct AudioCodecInfo {
    codec_name: String,
    clock_rate: u32,
    channels: u16,
    payload_type: u8,
}

impl SdpSource {
    pub fn new(config: InternalSourceConfig, sdp_content: String) -> Result<Self> {
        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Ok(Self {
            config,
            sdp_content,
            state: Arc::new(RwLock::new(StreamSourceState::Initializing)),
            rtp_tx,
            state_tx,
            task_handles: Vec::new(),
            shutdown_tx: None,
            #[cfg(feature = "source")]
            media_info: Arc::new(RwLock::new(None)),
            #[cfg(feature = "source")]
            rtcp_tx: Arc::new(RwLock::new(None)),
        })
    }

    async fn set_state(&self, new_state: StreamSourceState, error: Option<String>) {
        let mut state = self.state.write().await;
        let old_state = *state;

        if old_state != new_state {
            *state = new_state;

            let event = StateChangeEvent {
                old_state,
                new_state,
                error,
            };

            let _ = self.state_tx.send(event);

            info!(
                "[{}] State changed: {:?} -> {:?}",
                self.config.stream_id, old_state, new_state
            );
        }
    }

    #[cfg(feature = "source")]
    fn parse_connection_address(&self) -> Option<(String, bool)> {
        for line in self.sdp_content.lines() {
            let line = line.trim();
            if line.starts_with("c=IN IP4 ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    return Some((parts[2].to_string(), false));
                }
            } else if line.starts_with("c=IN IP6 ") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 {
                    return Some((parts[2].to_string(), true));
                }
            }
        }
        None
    }

    #[cfg(feature = "source")]
    fn parse_sdp(&self) -> Result<ParsedSdp> {
        let mut ports = Vec::new();
        let mut channel = 0u8;
        let mut video_codec: Option<VideoCodecInfo> = None;
        let mut audio_codec: Option<AudioCodecInfo> = None;
        let mut current_media_type: Option<String> = None;
        let mut current_payload_type: Option<u8> = None;

        let connection_info = self.parse_connection_address();
        let mut video_rtcp_addr: Option<SocketAddr> = None;
        let mut audio_rtcp_addr: Option<SocketAddr> = None;

        let is_ipv6 = connection_info
            .as_ref()
            .map(|(_, ipv6)| *ipv6)
            .unwrap_or(false);

        for line in self.sdp_content.lines() {
            let line = line.trim();

            if line.starts_with("m=video") || line.starts_with("m=audio") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    let media_type = parts[0].trim_start_matches("m=");
                    current_media_type = Some(media_type.to_string());

                    if let Ok(port) = parts[1].parse::<u16>()
                        && let Ok(pt) = parts[3].parse::<u8>()
                    {
                        current_payload_type = Some(pt);
                        ports.push((channel, port));

                        if let Some((ref addr, _)) = connection_info {
                            let rtcp_port = port + 1;
                            let rtcp_addr_str = if is_ipv6 {
                                format!("[{}]:{}", addr, rtcp_port)
                            } else {
                                format!("{}:{}", addr, rtcp_port)
                            };

                            if let Ok(rtcp_addr) = rtcp_addr_str.parse() {
                                match media_type {
                                    "video" => {
                                        video_rtcp_addr = Some(rtcp_addr);
                                        info!(
                                            "[{}] Video RTCP address: {}",
                                            self.config.stream_id, rtcp_addr
                                        );
                                    }
                                    "audio" => {
                                        audio_rtcp_addr = Some(rtcp_addr);
                                        info!(
                                            "[{}] Audio RTCP address: {}",
                                            self.config.stream_id, rtcp_addr
                                        );
                                    }
                                    _ => {}
                                }
                            }
                        }

                        channel += 2;

                        info!(
                            "[{}] Found media: {} on port {} (PT={})",
                            self.config.stream_id, media_type, port, pt
                        );
                    }
                }
            }

            if line.starts_with("a=rtpmap:") {
                let rtpmap = line.trim_start_matches("a=rtpmap:");
                let parts: Vec<&str> = rtpmap.split_whitespace().collect();

                if parts.len() >= 2
                    && let Ok(pt) = parts[0].parse::<u8>()
                    && Some(pt) == current_payload_type
                {
                    let codec_parts: Vec<&str> = parts[1].split('/').collect();

                    if codec_parts.len() >= 2 {
                        let codec_name = codec_parts[0].to_string();
                        let clock_rate = codec_parts[1].parse::<u32>().unwrap_or(90000);

                        match current_media_type.as_deref() {
                            Some("video") => {
                                video_codec = Some(VideoCodecInfo {
                                    codec_name: codec_name.clone(),
                                    clock_rate,
                                    payload_type: pt,
                                });
                                info!(
                                    "[{}] Parsed video codec: {} ({}Hz, PT={})",
                                    self.config.stream_id, codec_name, clock_rate, pt
                                );
                            }
                            Some("audio") => {
                                let channels = if codec_parts.len() >= 3 {
                                    codec_parts[2].parse::<u16>().unwrap_or(1)
                                } else {
                                    1
                                };

                                audio_codec = Some(AudioCodecInfo {
                                    codec_name: codec_name.clone(),
                                    clock_rate,
                                    channels,
                                    payload_type: pt,
                                });
                                info!(
                                    "[{}] Parsed audio codec: {} ({}Hz, {} channels, PT={})",
                                    self.config.stream_id, codec_name, clock_rate, channels, pt
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        if ports.is_empty() {
            anyhow::bail!("No valid media ports found in SDP");
        }

        let media_info = SdpMediaInfo {
            video_codec,
            audio_codec,
            video_rtcp_addr,
            audio_rtcp_addr,
        };

        Ok((ports, media_info, is_ipv6))
    }

    #[cfg(not(feature = "source"))]
    fn parse_sdp(&self) -> Result<Vec<(u8, u16)>> {
        let mut ports = Vec::new();
        let mut channel = 0u8;

        for line in self.sdp_content.lines() {
            let line = line.trim();

            if line.starts_with("m=video") || line.starts_with("m=audio") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(port) = parts[1].parse::<u16>() {
                        ports.push((channel, port));
                        channel += 2;

                        info!(
                            "[{}] Found media: {} on port {}",
                            self.config.stream_id, parts[0], port
                        );
                    }
                }
            }
        }

        if ports.is_empty() {
            anyhow::bail!("No valid media ports found in SDP");
        }

        Ok(ports)
    }

    #[cfg(feature = "source")]
    async fn rtcp_sender_task(
        stream_id: String,
        mut rtcp_rx: mpsc::UnboundedReceiver<(SocketAddr, Vec<u8>)>,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
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

                    match socket.send_to(&data, addr).await {
                        Ok(sent) => {
                            info!(
                                "[{}] RTCP sent successfully ({} bytes to {})",
                                stream_id, sent, addr
                            );
                        }
                        Err(e) => {
                            error!(
                                "[{}] Failed to send RTCP to {}: {}",
                                stream_id, addr, e
                            );
                        }
                    }
                }
            }
        }

        info!("[{}] RTCP sender task stopped", stream_id);
    }

    async fn run_udp_receiver(
        ctx: UdpReceiverContext,
        mut shutdown_rx: tokio::sync::broadcast::Receiver<()>,
    ) {
        let bind_addr: SocketAddr = if ctx.is_ipv6 {
            format!("[::]:{}", ctx.port).parse().unwrap()
        } else {
            format!("0.0.0.0:{}", ctx.port).parse().unwrap()
        };

        info!(
            "[{}] Starting UDP receiver on {} (channel {}, IPv{})",
            ctx.stream_id,
            bind_addr,
            ctx.channel,
            if ctx.is_ipv6 { 6 } else { 4 }
        );

        let socket = match UdpSocket::bind(bind_addr).await {
            Ok(s) => s,
            Err(e) => {
                error!("[{}] Failed to bind UDP socket: {}", ctx.stream_id, e);

                let mut s = ctx.state.write().await;
                *s = StreamSourceState::Error;

                let _ = ctx.state_tx.send(StateChangeEvent {
                    old_state: StreamSourceState::Initializing,
                    new_state: StreamSourceState::Error,
                    error: Some(format!("Failed to bind UDP socket: {}", e)),
                });

                return;
            }
        };

        let mut buf = vec![0u8; 2048];
        let mut packet_count = 0u64;

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!(
                        "[{}] UDP receiver shutting down (channel {})",
                        ctx.stream_id,
                        ctx.channel
                    );
                    break;
                }
                result = socket.recv_from(&mut buf) => {
                    match result {
                        Ok((len, _addr)) => {
                            packet_count += 1;

                            let packet = MediaPacket::Rtp {
                                channel: ctx.channel,
                                data: buf[..len].to_vec(),
                            };

                            if ctx.rtp_tx.send(packet).is_err() {
                                // Suppress warning
                            }

                            if packet_count.is_multiple_of(1000) {
                                trace!(
                                    "[{}] Received {} packets on channel {}",
                                    ctx.stream_id,
                                    packet_count,
                                    ctx.channel
                                );
                            }
                        }
                        Err(e) => {
                            error!(
                                "[{}] UDP receive error: {}",
                                ctx.stream_id,
                                e
                            );
                        }
                    }
                }
            }
        }
    }

    #[cfg(feature = "source")]
    fn video_codec_to_rtc(codec: &VideoCodecInfo) -> RTCRtpCodecParameters {
        let mime_type = format!("video/{}", codec.codec_name.to_uppercase());

        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type,
                clock_rate: codec.clock_rate,
                channels: 0,
                sdp_fmtp_line: if codec.codec_name.to_uppercase() == "H264" {
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                        .to_string()
                } else {
                    String::new()
                },
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
            payload_type: codec.payload_type,
            stats_id: String::new(),
        }
    }

    #[cfg(feature = "source")]
    fn audio_codec_to_rtc(codec: &AudioCodecInfo) -> RTCRtpCodecParameters {
        let mime_type = format!("audio/{}", codec.codec_name.to_uppercase());

        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type,
                clock_rate: codec.clock_rate,
                channels: codec.channels,
                sdp_fmtp_line: if codec.codec_name.to_lowercase() == "opus" {
                    "minptime=10;useinbandfec=1".to_string()
                } else {
                    String::new()
                },
                rtcp_feedback: vec![],
            },
            payload_type: codec.payload_type,
            stats_id: String::new(),
        }
    }

    #[cfg(feature = "source")]
    pub async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        let rtcp_tx = self.rtcp_tx.read().await;
        debug!(
            "[{}] get_rtcp_sender called, available: {}",
            self.config.stream_id,
            rtcp_tx.is_some()
        );

        if let Some(tx) = rtcp_tx.as_ref() {
            let media_info = self.media_info.read().await;

            if let Some(ref info) = *media_info {
                let rtcp_addr = info.video_rtcp_addr.or(info.audio_rtcp_addr);

                if let Some(addr) = rtcp_addr {
                    let (wrapper_tx, mut wrapper_rx) = mpsc::unbounded_channel::<Vec<u8>>();
                    let tx_clone = tx.clone();
                    let stream_id = self.config.stream_id.clone();

                    tokio::spawn(async move {
                        info!("[{}] RTCP wrapper task started for {}", stream_id, addr);

                        while let Some(data) = wrapper_rx.recv().await {
                            debug!(
                                "[{}] Forwarding RTCP to {}, size: {} bytes",
                                stream_id,
                                addr,
                                data.len()
                            );

                            if let Err(e) = tx_clone.send((addr, data)) {
                                error!("[{}] Failed to forward RTCP: {}", stream_id, e);
                                break;
                            }
                        }

                        info!("[{}] RTCP wrapper task stopped", stream_id);
                    });

                    info!(
                        "[{}] RTCP sender wrapper created for {}",
                        self.config.stream_id, addr
                    );

                    return Some(wrapper_tx);
                } else {
                    tracing::warn!(
                        "[{}] No RTCP address available in media info",
                        self.config.stream_id
                    );
                }
            }
        } else {
            tracing::warn!(
                "[{}] RTCP sender not available in get_rtcp_sender",
                self.config.stream_id
            );
        }

        None
    }
}

#[async_trait]
impl StreamSource for SdpSource {
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

        #[cfg(feature = "source")]
        let (ports, media_info, is_ipv6) = self.parse_sdp()?;

        #[cfg(not(feature = "source"))]
        let ports = self.parse_sdp()?;

        #[cfg(not(feature = "source"))]
        let is_ipv6 = false;

        let ports_len = ports.len();

        #[cfg(feature = "source")]
        {
            let mut store = self.media_info.write().await;
            *store = Some(media_info);
        }

        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx.clone());

        #[cfg(feature = "source")]
        {
            let (rtcp_tx, rtcp_rx) = mpsc::unbounded_channel();
            let mut rtcp_store = self.rtcp_tx.write().await;
            *rtcp_store = Some(rtcp_tx);

            let stream_id = self.config.stream_id.clone();
            let shutdown_rx = shutdown_tx.subscribe();

            let rtcp_task = tokio::spawn(async move {
                Self::rtcp_sender_task(stream_id, rtcp_rx, shutdown_rx).await;
            });

            self.task_handles.push(rtcp_task);

            info!("[{}] RTCP sender initialized", self.config.stream_id);
        }

        for (channel, port) in ports {
            let ctx = UdpReceiverContext {
                stream_id: self.config.stream_id.clone(),
                channel,
                port,
                rtp_tx: self.rtp_tx.clone(),
                state: self.state.clone(),
                state_tx: self.state_tx.clone(),
                is_ipv6,
            };

            let shutdown_rx = shutdown_tx.subscribe();

            let handle = tokio::spawn(async move {
                Self::run_udp_receiver(ctx, shutdown_rx).await;
            });

            self.task_handles.push(handle);
        }

        self.set_state(StreamSourceState::Connected, None).await;

        info!(
            "[{}] Started with {} receivers (IPv{})",
            self.config.stream_id,
            ports_len,
            if is_ipv6 { 6 } else { 4 }
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

        info!("[{}] Stopped", self.config.stream_id);
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
        if let Ok(media_info) = self.media_info.try_read()
            && let Some(ref info) = *media_info
            && let Some(ref video_codec) = info.video_codec
        {
            return Some(Self::video_codec_to_rtc(video_codec));
        }

        None
    }

    #[cfg(feature = "source")]
    async fn get_audio_codec(&self) -> Option<RTCRtpCodecParameters> {
        if let Ok(media_info) = self.media_info.try_read()
            && let Some(ref info) = *media_info
            && let Some(ref audio_codec) = info.audio_codec
        {
            return Some(Self::audio_codec_to_rtc(audio_codec));
        }

        None
    }

    #[cfg(feature = "source")]
    async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        self.get_rtcp_sender().await
    }
}
