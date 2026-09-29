use super::{InternalSourceConfig, MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
use anyhow::Result;
use async_trait::async_trait;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, error, info, trace, warn};

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
impl std::fmt::Display for MulticastJoin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MulticastJoin::V4 { group, interface } => {
                write!(f, "{group} (interface {interface})")
            }
            MulticastJoin::V6 { group, interface } => {
                write!(f, "{group} (interface index {interface})")
            }
        }
    }
}

/// The session-level SDP connection address (`c=IN IP4/IP6 <addr>`), keyed
/// by the address family declared on the line.  The bare address is kept
/// as written (it may be a hostname); RFC 4566 suffixes (`/ttl`, `/count`)
/// are stripped, and an IPv6 address may carry a `%zone` (interface index
/// or name) that defaults the multicast join interface.
#[cfg(feature = "source")]
enum ConnectionAddress {
    V4 { addr: String },
    V6 { addr: String, zone: Option<String> },
}

#[cfg(feature = "source")]
impl ConnectionAddress {
    fn addr(&self) -> &str {
        match self {
            ConnectionAddress::V4 { addr } | ConnectionAddress::V6 { addr, .. } => addr,
        }
    }
}

/// Split a `c=` address token into the bare address and an optional IPv6
/// zone, dropping RFC 4566 suffixes (`/ttl`, `/count`).
#[cfg(feature = "source")]
fn split_connection_token(token: &str) -> (String, Option<String>) {
    let (addr, zone) = match token.split_once('%') {
        Some((addr, zone)) => (addr, Some(zone)),
        None => (token, None),
    };
    let addr = addr.split('/').next().unwrap_or(addr).to_string();
    let zone = zone
        .map(|z| z.split('/').next().unwrap_or(z))
        .filter(|z| !z.is_empty())
        .map(str::to_string);
    (addr, zone)
}

/// The wildcard bind address for the media-port sockets: IPv6 when the
/// SDP connection address is an IP6 line, IPv4 otherwise.
#[cfg(feature = "source")]
type ParsedSdp = (Vec<(u8, u16)>, SdpMediaInfo, IpAddr, Option<MulticastJoin>);

struct UdpReceiverContext {
    stream_id: String,
    channel: u8,
    socket: UdpSocket,
    rtp_tx: broadcast::Sender<MediaPacket>,
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
fn addr_lacks_rtcp_destination(connection_info: &Option<ConnectionAddress>) -> bool {
    connection_info.as_ref().is_some_and(|ca| {
        ca.addr()
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_unspecified() || ip.is_multicast())
    })
}

/// Resolve an interface name to its index for IPv6 multicast joins.
#[cfg(all(feature = "source", unix))]
fn if_name_to_index(name: &str) -> Result<u32> {
    let name_c = std::ffi::CString::new(name)
        .map_err(|_| anyhow::anyhow!("interface name contains a NUL byte"))?;
    // SAFETY: name_c is a valid NUL-terminated C string; the returned index
    // is 0 when no interface has that name.
    let index = unsafe { libc::if_nametoindex(name_c.as_ptr()) };
    if index == 0 {
        anyhow::bail!("interface '{name}' not found");
    }
    Ok(index)
}

#[cfg(all(feature = "source", not(unix)))]
fn if_name_to_index(name: &str) -> Result<u32> {
    anyhow::bail!(
        "interface names are not supported on this platform; use an interface index instead of '{name}'"
    )
}

/// Resolve an IPv6 multicast join interface given as an interface index or
/// an interface name (interface indexes are not stable across reboots;
/// names are the durable identifier).
#[cfg(feature = "source")]
fn resolve_v6_interface(value: &str, group: &Ipv6Addr) -> Result<u32> {
    if let Ok(index) = value.parse::<u32>() {
        return Ok(index);
    }
    if_name_to_index(value).map_err(|e| {
        anyhow::anyhow!(
            "multicast_interface '{value}' must be an interface index or name for IPv6 group {group}: {e:#}"
        )
    })
}

/// Resolve the multicast join for the SDP connection address: `None` for
/// unicast/unspecified addresses, an error when the address is a multicast
/// group but `multicast_interface` does not fit its family (IPv4 groups
/// take an interface address, IPv6 groups an interface index or name).
#[cfg(feature = "source")]
fn multicast_join(
    connection_info: &Option<ConnectionAddress>,
    multicast_interface: Option<&str>,
) -> Result<Option<MulticastJoin>> {
    let Some(ca) = connection_info else {
        return Ok(None);
    };
    let Ok(ip) = ca.addr().parse::<std::net::IpAddr>() else {
        // Hostnames and malformed addresses stay on the unicast path.
        warn!(
            "SDP connection address '{}' is not an IP literal; treated as unicast (no multicast join)",
            ca.addr()
        );
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
            // An explicit config wins; otherwise the address's own zone
            // (`c=IN IP6 ff12::1%eth0`) picks the interface; otherwise the
            // kernel — which cannot choose for link-local groups.
            let zone = match ca {
                ConnectionAddress::V6 { zone, .. } => zone.as_deref(),
                ConnectionAddress::V4 { .. } => None,
            };
            let interface = match configured.or(zone) {
                Some(v) => resolve_v6_interface(v, &group)?,
                None => 0,
            };
            Ok(Some(MulticastJoin::V6 { group, interface }))
        }
    }
}

/// Multicast membership depends on interface state that may still be
/// settling when the source starts (boot-time link-up, DHCP), so a failed
/// bind/join is retried with backoff before the start is failed.  Longer
/// interface delays must be covered by service ordering instead (systemd:
/// `After=network-online.target`).
#[cfg(feature = "source")]
const BIND_JOIN_MAX_ATTEMPTS: u32 = 5;
#[cfg(feature = "source")]
const BIND_JOIN_INITIAL_DELAY: std::time::Duration = std::time::Duration::from_millis(500);
#[cfg(feature = "source")]
const BIND_JOIN_MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(4);

/// Bind a media-port UDP socket on the wildcard address of `bind_ip`'s
/// family, joining the multicast group when the SDP connection address
/// named one.  Multicast receivers set SO_REUSEADDR so several receivers
/// of the same group:port can coexist on one host (a second stream, a
/// monitoring tool); SO_REUSEPORT would load-balance the datagrams instead
/// of duplicating them.
#[cfg(feature = "source")]
async fn bind_receiver_socket(
    bind_ip: IpAddr,
    port: u16,
    multicast: Option<MulticastJoin>,
) -> Result<UdpSocket> {
    let bind_addr = SocketAddr::new(bind_ip, port);

    let Some(join) = multicast else {
        return UdpSocket::bind(bind_addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind UDP socket {bind_addr}: {e}"));
    };

    let domain = match bind_ip {
        IpAddr::V4(_) => socket2::Domain::IPV4,
        IpAddr::V6(_) => socket2::Domain::IPV6,
    };
    let socket = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket
        .bind(&bind_addr.into())
        .map_err(|e| anyhow::anyhow!("Failed to bind UDP socket {bind_addr}: {e}"))?;
    let socket = UdpSocket::from_std(socket.into())?;

    match join {
        MulticastJoin::V4 { group, interface } => {
            socket.join_multicast_v4(group, interface).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to join multicast group {group} (interface {interface}): {e}"
                )
            })?;
        }
        MulticastJoin::V6 { group, interface } => {
            socket.join_multicast_v6(&group, interface).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to join multicast group {group} (interface index {interface}): {e}"
                )
            })?;
        }
    }
    Ok(socket)
}

#[cfg(feature = "source")]
async fn bind_receiver_with_retry(
    stream_id: &str,
    channel: u8,
    port: u16,
    bind_ip: IpAddr,
    multicast: Option<MulticastJoin>,
) -> Result<UdpSocket> {
    let mut delay = BIND_JOIN_INITIAL_DELAY;
    for attempt in 1..=BIND_JOIN_MAX_ATTEMPTS {
        match bind_receiver_socket(bind_ip, port, multicast).await {
            Ok(socket) => {
                match multicast {
                    Some(join) => info!(
                        "[{}] Joined multicast group {} on channel {} (port {})",
                        stream_id, join, channel, port
                    ),
                    None => info!(
                        "[{}] UDP receiver bound on port {} (channel {}, IPv{})",
                        stream_id,
                        port,
                        channel,
                        if bind_ip.is_ipv6() { 6 } else { 4 }
                    ),
                }
                return Ok(socket);
            }
            Err(e) => {
                // Only multicast receivers retry: a unicast wildcard bind
                // fails only on local conflicts that will not clear.
                if multicast.is_none() || attempt == BIND_JOIN_MAX_ATTEMPTS {
                    return Err(e);
                }
                warn!(
                    "[{}] channel {} bind/join failed (attempt {}/{}): {:#}; retrying in {:?}",
                    stream_id, channel, attempt, BIND_JOIN_MAX_ATTEMPTS, e, delay
                );
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(BIND_JOIN_MAX_DELAY);
            }
        }
    }
    unreachable!()
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
    fn parse_connection_address(&self) -> Option<ConnectionAddress> {
        for line in self.sdp_content.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("c=IN IP4 ")
                && let Some(token) = rest.split_whitespace().next()
            {
                let (addr, _) = split_connection_token(token);
                return Some(ConnectionAddress::V4 { addr });
            } else if let Some(rest) = line.strip_prefix("c=IN IP6 ")
                && let Some(token) = rest.split_whitespace().next()
            {
                let (addr, zone) = split_connection_token(token);
                return Some(ConnectionAddress::V6 { addr, zone });
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

        let bind_ip: IpAddr = match connection_info {
            Some(ConnectionAddress::V6 { .. }) => Ipv6Addr::UNSPECIFIED.into(),
            _ => Ipv4Addr::UNSPECIFIED.into(),
        };

        let multicast =
            multicast_join(&connection_info, self.config.multicast_interface.as_deref())?;

        // A connection address without a usable RTCP destination
        // (unspecified / multicast) gets none; skip it instead of sending
        // RTCP into the void (os error 65 / 101).
        let no_rtcp = addr_lacks_rtcp_destination(&connection_info);

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

                        if let Some(ref ca) = connection_info
                            && !no_rtcp
                        {
                            let rtcp_port = port + 1;
                            let rtcp_addr_str = if bind_ip.is_ipv6() {
                                format!("[{}]:{}", ca.addr(), rtcp_port)
                            } else {
                                format!("{}:{}", ca.addr(), rtcp_port)
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

        Ok((ports, media_info, bind_ip, multicast))
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
                result = ctx.socket.recv_from(&mut buf) => {
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
        let (ports, media_info, bind_ip, multicast) = self.parse_sdp()?;

        #[cfg(not(feature = "source"))]
        let ports = self.parse_sdp()?;

        #[cfg(not(feature = "source"))]
        let bind_ip: IpAddr = Ipv4Addr::UNSPECIFIED.into();

        let ports_len = ports.len();

        #[cfg(feature = "source")]
        {
            let mut store = self.media_info.write().await;
            *store = Some(media_info);
        }

        // Bind every media port (joining the multicast group, if any)
        // before spawning anything: a bind/join failure must fail the start
        // here, where it surfaces through the caller's error path, instead
        // of later from a receiver task — by then the bridge is up and the
        // stream looks published but stays silent forever.
        let mut sockets = Vec::with_capacity(ports_len);
        for (channel, port) in ports {
            #[cfg(feature = "source")]
            let socket =
                bind_receiver_with_retry(&self.config.stream_id, channel, port, bind_ip, multicast)
                    .await?;

            #[cfg(not(feature = "source"))]
            let socket = UdpSocket::bind(SocketAddr::new(bind_ip, port)).await?;

            sockets.push((channel, socket));
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

        for (channel, socket) in sockets {
            let ctx = UdpReceiverContext {
                stream_id: self.config.stream_id.clone(),
                channel,
                socket,
                rtp_tx: self.rtp_tx.clone(),
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
            if bind_ip.is_ipv6() { 6 } else { 4 }
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
        let (ports, media_info, bind_ip, multicast) = source.parse_sdp().unwrap();

        assert_eq!(ports, vec![(0u8, 5004u16), (2u8, 5006u16)]);
        assert!(!bind_ip.is_ipv6());
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
        let (ports, media_info, bind_ip, multicast) = source.parse_sdp().unwrap();

        assert_eq!(ports, vec![(0u8, 1720u16)]);
        assert!(!bind_ip.is_ipv6());
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
        let (_, _, bind_ip, multicast) = source.parse_sdp().unwrap();

        assert!(bind_ip.is_ipv6());
        match multicast {
            Some(MulticastJoin::V6 { group, interface }) => {
                assert_eq!(group, "ff15::1".parse::<Ipv6Addr>().unwrap());
                assert_eq!(interface, 2);
            }
            other => panic!("expected IPv6 multicast join, got {other:?}"),
        }
    }

    #[test]
    fn multicast_group_with_rfc4566_ttl_suffix_still_joins() {
        // Senders may write the TTL after the group (RFC 4566):
        // `c=IN IP4 230.1.1.1/64` (optionally `/ttl/count`).
        let sdp = MULTICAST_SDP.replace("c=IN IP4 230.1.1.1", "c=IN IP4 230.1.1.1/64");
        let source = test_source(&sdp);
        let (_, _, _, multicast) = source.parse_sdp().unwrap();

        match multicast {
            Some(MulticastJoin::V4 { group, interface }) => {
                assert_eq!(group, Ipv4Addr::new(230, 1, 1, 1));
                assert_eq!(interface, Ipv4Addr::UNSPECIFIED);
            }
            other => panic!("expected IPv4 multicast join, got {other:?}"),
        }
    }

    #[test]
    fn ipv6_multicast_zone_defaults_the_interface() {
        let sdp = MULTICAST_SDP.replace("c=IN IP4 230.1.1.1", "c=IN IP6 ff12::1%3");
        let source = test_source(&sdp);
        let (_, _, bind_ip, multicast) = source.parse_sdp().unwrap();

        assert!(bind_ip.is_ipv6());
        match multicast {
            Some(MulticastJoin::V6 { group, interface }) => {
                assert_eq!(group, "ff12::1".parse::<Ipv6Addr>().unwrap());
                assert_eq!(interface, 3);
            }
            other => panic!("expected IPv6 multicast join, got {other:?}"),
        }
    }

    #[test]
    fn ipv6_multicast_configured_interface_overrides_zone() {
        let sdp = MULTICAST_SDP.replace("c=IN IP4 230.1.1.1", "c=IN IP6 ff12::1%3");
        let source = test_source_with(&sdp, Some("2"));
        let (_, _, _, multicast) = source.parse_sdp().unwrap();

        match multicast {
            Some(MulticastJoin::V6 { interface, .. }) => assert_eq!(interface, 2),
            other => panic!("expected IPv6 multicast join, got {other:?}"),
        }
    }

    #[test]
    fn ipv6_multicast_unknown_interface_name_errors() {
        let sdp = MULTICAST_SDP.replace("c=IN IP4 230.1.1.1", "c=IN IP6 ff15::1");
        let source = test_source_with(&sdp, Some("live777-no-such-iface0"));
        assert!(source.parse_sdp().is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ipv6_multicast_interface_name_resolves_to_index() {
        let sdp = MULTICAST_SDP.replace("c=IN IP4 230.1.1.1", "c=IN IP6 ff15::1");
        let source = test_source_with(&sdp, Some("lo"));
        let (_, _, _, multicast) = source.parse_sdp().unwrap();

        let lo_index = std::ffi::CString::new("lo")
            .map(|name| unsafe { libc::if_nametoindex(name.as_ptr()) })
            .unwrap();
        assert!(lo_index > 0);
        match multicast {
            Some(MulticastJoin::V6 { interface, .. }) => assert_eq!(interface, lo_index),
            other => panic!("expected IPv6 multicast join, got {other:?}"),
        }
    }

    #[test]
    fn non_literal_connection_address_stays_unicast() {
        // A hostname is legal in SDP but cannot be checked for multicast;
        // it keeps the historical unicast behavior (no join, RTCP skipped
        // only because the address string does not parse into a socket).
        let sdp = MULTICAST_SDP.replace("c=IN IP4 230.1.1.1", "c=IN IP4 camera.local");
        let source = test_source_with(&sdp, Some("192.168.123.11"));
        let (_, media_info, _, multicast) = source.parse_sdp().unwrap();

        assert!(multicast.is_none());
        assert!(media_info.video_rtcp_addr.is_none());
    }

    #[test]
    fn split_connection_token_strips_suffixes() {
        assert_eq!(
            split_connection_token("230.1.1.1"),
            ("230.1.1.1".into(), None)
        );
        assert_eq!(
            split_connection_token("230.1.1.1/64"),
            ("230.1.1.1".into(), None)
        );
        assert_eq!(
            split_connection_token("230.1.1.1/64/3"),
            ("230.1.1.1".into(), None)
        );
        assert_eq!(
            split_connection_token("ff12::1%eth0"),
            ("ff12::1".into(), Some("eth0".into()))
        );
        assert_eq!(
            split_connection_token("ff12::1%2/3"),
            ("ff12::1".into(), Some("2".into()))
        );
    }

    #[tokio::test]
    async fn start_fails_when_multicast_interface_is_wrong_family() {
        // Fail-fast: the error must come out of start() itself, not later
        // from a receiver task after the bridge is up.
        let mut source = test_source_with(MULTICAST_SDP, Some("2"));
        let err = source.start().await.unwrap_err();
        assert!(
            err.to_string().contains("multicast_interface"),
            "unexpected error: {err:#}"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn start_joins_multicast_group_on_loopback() {
        // Joining 232.x on the loopback interface works without any
        // multicast-capable network; the membership is dropped on stop().
        let mut source = test_source_with(MULTICAST_SDP, Some("127.0.0.1"));
        source.start().await.unwrap();
        assert_eq!(source.state(), StreamSourceState::Connected);
        source.stop().await.unwrap();
        assert_eq!(source.state(), StreamSourceState::Disconnected);
    }
}
