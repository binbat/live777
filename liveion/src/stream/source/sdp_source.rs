use super::{InternalSourceConfig, MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
use anyhow::Result;
use async_trait::async_trait;
use std::net::SocketAddr;
#[cfg(feature = "source")]
use std::net::{Ipv4Addr, Ipv6Addr};
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

/// Multicast group membership for the receiver sockets, derived from the
/// SDP connection address when it names a multicast group (e.g.
/// `c=IN IP4 230.1.1.1`); unicast and unspecified addresses yield `None`.
#[cfg(feature = "source")]
#[derive(Debug, Clone, Copy)]
enum MulticastJoin {
    /// IPv4 group joined on the interface owning `interface`
    /// (`0.0.0.0` lets the kernel choose).
    V4 {
        group: Ipv4Addr,
        interface: Ipv4Addr,
    },
    /// IPv6 group joined on the interface with index `interface`
    /// (`0` lets the kernel choose).
    V6 { group: Ipv6Addr, interface: u32 },
}

#[cfg(feature = "source")]
type ParsedSdp = (Vec<(u8, u16)>, SdpMediaInfo, bool, Option<MulticastJoin>);

struct UdpReceiverContext {
    stream_id: String,
    channel: u8,
    port: u16,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state: Arc<std::sync::RwLock<StreamSourceState>>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    is_ipv6: bool,
    #[cfg(feature = "source")]
    multicast: Option<MulticastJoin>,
}

pub struct SdpSource {
    config: InternalSourceConfig,
    sdp_content: String,
    state: Arc<std::sync::RwLock<StreamSourceState>>,
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

/// Whether the SDP connection address is missing a usable RTCP destination.
/// Senders commonly write `c=IN IP4 0.0.0.0` (e.g. FFmpeg), which is not a
/// usable RTCP destination.  A multicast group is not one either: the
/// source only receives from the group, and hosts without a multicast
/// sender route would spam ENETUNREACH errors on every feedback packet.
#[cfg(feature = "source")]
fn addr_lacks_rtcp_destination(connection_info: &Option<(String, bool)>) -> bool {
    connection_info.as_ref().is_some_and(|(addr, _)| {
        addr.parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_unspecified() || ip.is_multicast())
    })
}

/// Resolve the multicast join for the SDP connection address: `None` for
/// unicast/unspecified addresses, an error when the address is a multicast
/// group but `multicast_interface` does not fit its family (IPv4 groups
/// take an interface address, IPv6 groups an interface index).
#[cfg(feature = "source")]
fn multicast_join(
    connection_info: &Option<(String, bool)>,
    multicast_interface: Option<&str>,
) -> Result<Option<MulticastJoin>> {
    let Some((addr, _)) = connection_info else {
        return Ok(None);
    };
    let Ok(ip) = addr.parse::<std::net::IpAddr>() else {
        // Hostnames and malformed addresses stay on the unicast path.
        return Ok(None);
    };
    if !ip.is_multicast() {
        return Ok(None);
    }

    let configured = multicast_interface.map(str::trim).filter(|s| !s.is_empty());

    match ip {
        std::net::IpAddr::V4(group) => {
            let interface = match configured {
                Some(v) => v.parse::<Ipv4Addr>().map_err(|_| {
                    anyhow::anyhow!(
                        "multicast_interface '{v}' must be an IPv4 address for IPv4 group {group}"
                    )
                })?,
                None => Ipv4Addr::UNSPECIFIED,
            };
            Ok(Some(MulticastJoin::V4 { group, interface }))
        }
        std::net::IpAddr::V6(group) => {
            let interface = match configured {
                Some(v) => v.parse::<u32>().map_err(|_| {
                    anyhow::anyhow!(
                        "multicast_interface '{v}' must be an interface index for IPv6 group {group}"
                    )
                })?,
                None => 0,
            };
            Ok(Some(MulticastJoin::V6 { group, interface }))
        }
    }
}

impl SdpSource {
    pub fn new(config: InternalSourceConfig, sdp_content: String) -> Result<Self> {
        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Ok(Self {
            config,
            sdp_content,
            state: Arc::new(std::sync::RwLock::new(StreamSourceState::Initializing)),
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
        let changed = {
            let mut state = self.state.write().unwrap();
            let old_state = *state;

            if old_state != new_state {
                *state = new_state;
                Some(old_state)
            } else {
                None
            }
        };

        if let Some(old_state) = changed {
            let _ = self.state_tx.send(StateChangeEvent {
                old_state,
                new_state,
                error,
            });

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

        let multicast =
            multicast_join(&connection_info, self.config.multicast_interface.as_deref())?;

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

                        // A connection address without a usable RTCP
                        // destination (unspecified / multicast) gets none;
                        // skip it instead of sending RTCP into the void
                        // (os error 65 / 101).
                        let no_rtcp = addr_lacks_rtcp_destination(&connection_info);

                        if let Some((ref addr, _)) = connection_info
                            && !no_rtcp
                        {
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

        Ok((ports, media_info, is_ipv6, multicast))
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

                let mut s = ctx.state.write().unwrap();
                *s = StreamSourceState::Error;

                let _ = ctx.state_tx.send(StateChangeEvent {
                    old_state: StreamSourceState::Initializing,
                    new_state: StreamSourceState::Error,
                    error: Some(format!("Failed to bind UDP socket: {}", e)),
                });

                return;
            }
        };

        #[cfg(feature = "source")]
        if let Some(join) = ctx.multicast {
            let joined = match join {
                MulticastJoin::V4 { group, interface } => socket
                    .join_multicast_v4(group, interface)
                    .map(|_| format!("{group} (interface {interface})")),
                MulticastJoin::V6 { group, interface } => socket
                    .join_multicast_v6(&group, interface)
                    .map(|_| format!("{group} (interface index {interface})")),
            };

            match joined {
                Ok(what) => {
                    info!(
                        "[{}] Joined multicast group {} on channel {}",
                        ctx.stream_id, what, ctx.channel
                    );
                }
                Err(e) => {
                    error!("[{}] Failed to join multicast group: {}", ctx.stream_id, e);

                    let mut s = ctx.state.write().unwrap();
                    *s = StreamSourceState::Error;

                    let _ = ctx.state_tx.send(StateChangeEvent {
                        old_state: StreamSourceState::Initializing,
                        new_state: StreamSourceState::Error,
                        error: Some(format!("Failed to join multicast group: {}", e)),
                    });

                    return;
                }
            }
        }

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
        // can negotiate the codec.  profile-id 1 = Main (RFC 7798); browsers
        // offering H265 (Safari, Chrome 136+) use profile-id=1, and 0 is not
        // a valid HEVC profile-id, so it never matches a real offer.
        if matches!(codec, rtsp::VideoCodecParams::H265 { .. })
            && params.rtp_codec.sdp_fmtp_line.is_empty()
        {
            params.rtp_codec.sdp_fmtp_line = "profile-id=1;tier-flag=0;tx-mode=SRST".to_string();
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
                    // Polled by the source manager until its wait deadline;
                    // it reports the timeout, so stay quiet here.
                    debug!(
                        "[{}] No RTCP address available in media info",
                        self.config.stream_id
                    );
                }
            }
        } else {
            debug!(
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
        *self.state.read().unwrap()
    }

    async fn start(&mut self) -> Result<()> {
        if !self.task_handles.is_empty() {
            anyhow::bail!("Source already started");
        }

        #[cfg(feature = "source")]
        let (ports, media_info, is_ipv6, multicast) = self.parse_sdp()?;

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
                #[cfg(feature = "source")]
                multicast,
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
        test_source_with(sdp, None)
    }

    fn test_source_with(sdp: &str, multicast_interface: Option<&str>) -> SdpSource {
        SdpSource::new(
            InternalSourceConfig {
                stream_id: "test".to_string(),
                #[cfg(any(feature = "source-rtsp", feature = "source-whep"))]
                url: "test.sdp".to_string(),
                multicast_interface: multicast_interface.map(str::to_string),
            },
            sdp.to_string(),
        )
        .unwrap()
    }

    const MULTICAST_SDP: &str = "v=0\r\n\
                                 o=- 0 0 IN IP4 192.168.123.13\r\n\
                                 s=unitree\r\n\
                                 c=IN IP4 230.1.1.1\r\n\
                                 t=0 0\r\n\
                                 m=video 1720 RTP/AVP 96\r\n\
                                 a=rtpmap:96 H264/90000\r\n";

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
            params.rtp_codec.sdp_fmtp_line.contains("profile-id=1"),
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
        let (ports, media_info, is_ipv6, multicast) = source.parse_sdp().unwrap();

        assert_eq!(ports, vec![(0u8, 5004u16), (2u8, 5006u16)]);
        assert!(!is_ipv6);
        assert!(multicast.is_none(), "unicast address must not join");
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

    #[test]
    fn multicast_group_joins_with_kernel_default_interface() {
        let source = test_source(MULTICAST_SDP);
        let (ports, media_info, is_ipv6, multicast) = source.parse_sdp().unwrap();

        assert_eq!(ports, vec![(0u8, 1720u16)]);
        assert!(!is_ipv6);
        // A multicast group is not a usable RTCP destination: the source
        // only receives, and hosts without a multicast sender route would
        // spam ENETUNREACH on every feedback packet.
        assert!(media_info.video_rtcp_addr.is_none());
        match multicast {
            Some(MulticastJoin::V4 { group, interface }) => {
                assert_eq!(group, Ipv4Addr::new(230, 1, 1, 1));
                assert_eq!(interface, Ipv4Addr::UNSPECIFIED);
            }
            other => panic!("expected IPv4 multicast join, got {other:?}"),
        }
    }

    #[test]
    fn multicast_interface_selects_join_interface() {
        let source = test_source_with(MULTICAST_SDP, Some("192.168.123.11"));
        let (_, _, _, multicast) = source.parse_sdp().unwrap();

        match multicast {
            Some(MulticastJoin::V4 { interface, .. }) => {
                assert_eq!(interface, Ipv4Addr::new(192, 168, 123, 11));
            }
            other => panic!("expected IPv4 multicast join, got {other:?}"),
        }
    }

    #[test]
    fn multicast_interface_family_mismatch_errors() {
        let source = test_source_with(MULTICAST_SDP, Some("not-an-ip"));
        assert!(source.parse_sdp().is_err());

        // An interface index is only meaningful for IPv6 groups.
        let source = test_source_with(MULTICAST_SDP, Some("2"));
        assert!(source.parse_sdp().is_err());
    }

    #[test]
    fn ipv6_multicast_group_takes_interface_index() {
        let sdp = MULTICAST_SDP.replace("c=IN IP4 230.1.1.1", "c=IN IP6 ff15::1");
        let source = test_source_with(&sdp, Some("2"));
        let (_, _, is_ipv6, multicast) = source.parse_sdp().unwrap();

        assert!(is_ipv6);
        match multicast {
            Some(MulticastJoin::V6 { group, interface }) => {
                assert_eq!(group, "ff15::1".parse::<Ipv6Addr>().unwrap());
                assert_eq!(interface, 2);
            }
            other => panic!("expected IPv6 multicast join, got {other:?}"),
        }
    }
}
