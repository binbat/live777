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
use rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters;

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
    video_codec: Option<rtsp::VideoCodecParams>,
    audio_codec: Option<rtsp::AudioCodecParams>,
    video_rtcp_addr: Option<SocketAddr>,
    audio_rtcp_addr: Option<SocketAddr>,
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

        let connection_info = self.parse_connection_address();
        let mut video_rtcp_addr: Option<SocketAddr> = None;
        let mut audio_rtcp_addr: Option<SocketAddr> = None;

        let is_ipv6 = connection_info
            .as_ref()
            .map(|(_, ipv6)| *ipv6)
            .unwrap_or(false);

        // Scan `m=` lines for media ports and derive RTCP addresses
        // (port + 1) from the session connection address. Codec parsing is
        // delegated to the shared libs/rtsp SDP parser below.
        for line in self.sdp_content.lines() {
            let line = line.trim();

            if line.starts_with("m=video") || line.starts_with("m=audio") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    let media_type = parts[0].trim_start_matches("m=");

                    if let Ok(port) = parts[1].parse::<u16>() {
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
                            "[{}] Found media: {} on port {}",
                            self.config.stream_id, media_type, port
                        );
                    }
                }
            }
        }

        if ports.is_empty() {
            anyhow::bail!("No valid media ports found in SDP");
        }

        // Shared SDP codec parsing (libs/rtsp): keeps codec parameters such
        // as H264 sprop-parameter-sets and H265 sprop-vps/sps/pps.
        let parsed = rtsp::parse_media_info_from_sdp(self.sdp_content.as_bytes())?;

        if let Some(ref codec) = parsed.video_codec {
            info!(
                "[{}] Parsed video codec: {:?}",
                self.config.stream_id, codec
            );
        }
        if let Some(ref codec) = parsed.audio_codec {
            info!(
                "[{}] Parsed audio codec: {:?}",
                self.config.stream_id, codec
            );
        }

        let media_info = SdpMediaInfo {
            video_codec: parsed.video_codec,
            audio_codec: parsed.audio_codec,
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
                                data: buf[..len].to_vec().into(),
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
    fn video_codec_to_rtc(codec: &rtsp::VideoCodecParams) -> RTCRtpCodecParameters {
        let mut params = crate::rtsp_codec::video_codec_to_rtc(codec);
        // Keep the historical H265 browser fallback: when the source SDP
        // carries no sprop parameters, offer a default H265 fmtp so browsers
        // can negotiate the codec.
        if matches!(codec, rtsp::VideoCodecParams::H265 { .. })
            && params.rtp_codec.sdp_fmtp_line.is_empty()
        {
            params.rtp_codec.sdp_fmtp_line = "profile-id=0;tier-flag=0;tx-mode=SRST".to_string();
        }
        params
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
            return Some(crate::rtsp_codec::audio_codec_to_rtc(audio_codec));
        }

        None
    }

    #[cfg(feature = "source")]
    async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        self.get_rtcp_sender().await
    }
}

#[cfg(all(test, feature = "source"))]
mod tests {
    use super::*;

    fn test_source(sdp: &str) -> SdpSource {
        SdpSource::new(
            InternalSourceConfig {
                stream_id: "test".to_string(),
                url: "test.sdp".to_string(),
            },
            sdp.to_string(),
        )
        .unwrap()
    }

    fn h265(vps: Vec<u8>, sps: Vec<u8>, pps: Vec<u8>) -> rtsp::VideoCodecParams {
        rtsp::VideoCodecParams::H265 {
            payload_type: 97,
            clock_rate: 90000,
            vps,
            sps,
            pps,
        }
    }

    #[test]
    fn h265_without_sprop_gets_default_profile() {
        let params = SdpSource::video_codec_to_rtc(&h265(vec![], vec![], vec![]));

        assert_eq!(params.rtp_codec.mime_type, "video/H265");
        assert!(
            params.rtp_codec.sdp_fmtp_line.contains("profile-id=0"),
            "expected default profile-id, got {}",
            params.rtp_codec.sdp_fmtp_line
        );
        assert!(
            params.rtp_codec.sdp_fmtp_line.contains("tx-mode=SRST"),
            "expected tx-mode=SRST, got {}",
            params.rtp_codec.sdp_fmtp_line
        );
    }

    #[test]
    fn h265_with_sprop_uses_sprop_fmtp() {
        let params = SdpSource::video_codec_to_rtc(&h265(vec![1], vec![2], vec![3]));

        let fmtp = &params.rtp_codec.sdp_fmtp_line;
        assert!(
            fmtp.contains("sprop-vps=AQ=="),
            "expected sprop-vps in fmtp, got {fmtp}"
        );
        assert!(
            fmtp.contains("sprop-sps=Ag=="),
            "expected sprop-sps in fmtp, got {fmtp}"
        );
        assert!(
            fmtp.contains("sprop-pps=Aw=="),
            "expected sprop-pps in fmtp, got {fmtp}"
        );
        assert!(
            !fmtp.contains("tx-mode=SRST"),
            "sprop present, default fallback must not apply: {fmtp}"
        );
    }

    #[test]
    fn h264_keeps_default_fmtp() {
        let codec = rtsp::VideoCodecParams::H264 {
            payload_type: 96,
            clock_rate: 90000,
            profile_level_id: None,
            packetization_mode: None,
            sps: vec![],
            pps: vec![],
        };

        let params = SdpSource::video_codec_to_rtc(&codec);

        assert!(
            params
                .rtp_codec
                .sdp_fmtp_line
                .contains("profile-level-id=42001f")
        );
        assert!(
            params
                .rtp_codec
                .sdp_fmtp_line
                .contains("packetization-mode=1")
        );
    }

    #[test]
    fn parses_ports_codecs_and_rtcp_with_shared_parser() {
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=test\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=video 5004 RTP/AVP 96\r\n\
                   a=rtpmap:96 H264/90000\r\n\
                   a=fmtp:96 profile-level-id=42001f;sprop-parameter-sets=Z0IAH5WoFAFuQA==,aM4yyA==\r\n\
                   m=audio 5006 RTP/AVP 111\r\n\
                   a=rtpmap:111 opus/48000/2\r\n";

        let source = test_source(sdp);
        let (ports, media_info, is_ipv6) = source.parse_sdp().unwrap();

        assert_eq!(ports, vec![(0u8, 5004u16), (2u8, 5006u16)]);
        assert!(!is_ipv6);
        assert_eq!(media_info.video_rtcp_addr.unwrap().port(), 5005);
        assert_eq!(media_info.audio_rtcp_addr.unwrap().port(), 5007);

        match media_info.video_codec.unwrap() {
            rtsp::VideoCodecParams::H264 { sps, pps, .. } => {
                assert!(!sps.is_empty(), "H264 sprop SPS must be retained");
                assert!(!pps.is_empty(), "H264 sprop PPS must be retained");
            }
            other => panic!("expected H264, got {other:?}"),
        }

        let audio = media_info.audio_codec.unwrap();
        assert_eq!(audio.codec.to_lowercase(), "opus");
        assert_eq!(audio.clock_rate, 48000);
        assert_eq!(audio.channels, 2);
    }
}
