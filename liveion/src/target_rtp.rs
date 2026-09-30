//! Static RTP/UDP output targets (multicast sender).
//!
//! A `[[stream.<name>.targets]]` config entry with an `rtp://host:port` URL
//! sends the stream's media out as plain RTP over UDP — to a multicast group
//! (e.g. a Unitree video receiver, or any `udpsrc`-based pipeline) when the
//! host is a multicast address, or to a unicast address otherwise. live777
//! acts as the multicast sender, symmetric to how an SDP file source joins a
//! group as the receiver (live777#465).
//!
//! The send is media-driven, mirroring the WHIP target supervisor: the
//! sockets open when the stream gains a publisher (`PublishStarted`, real
//! WHIP or a source's virtual one) and close when the publisher goes away
//! (`PublishStopped`). Failures retry with the same exponential backoff, and
//! a target on an `on_demand` stream acts as standing demand for its sources.
//!
//! Media handling per track (only the first video and the first audio track
//! are sent; other tracks are ignored):
//!
//! - Video is re-assembled and re-packetized through [`RePayloadCodec`]:
//!   SPS/PPS are inlined ahead of every IDR (idempotent for streams that
//!   already carry them) and oversized frames become clean FU-A fragments,
//!   so simple receivers joining mid-GOP decode from the next IDR.
//! - Audio (and anything else) is forwarded untouched.
//!
//! Ports follow the RTP/AVP convention: video goes to the URL's port, audio
//! to port + 2 (port + 1 stays reserved for RTCP).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use livetwo::payload::{Forward, RePayload, RePayloadCodec};
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::shared::marshal::{Marshal, MarshalSize};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::TargetConfig;
use crate::event::{Event, StreamDeleteReason};
use crate::forward::track::PublishTrackRemote;
use crate::forward::{PeerForward, rtcp::RtcpMessage};
use crate::reconnect::reconnect_delay;
use crate::stream::manager::Manager;

/// Spawn one supervisor for a configured static RTP target. Parse failures
/// are reported once and dropped, mirroring `target::init`.
pub(crate) fn spawn(manager: Arc<Manager>, stream: String, target: TargetConfig) {
    match RtpTargetContext::new(manager, stream, target) {
        Ok(ctx) => {
            tokio::spawn(ctx.run());
        }
        Err(e) => error!("[target] {}", e),
    }
}

/// Parse an `rtp://host:port` target URL into a socket address.
/// The host must be an IP literal (v4 or v6 in brackets); no userinfo,
/// path, query or fragment is allowed.
pub(crate) fn parse_rtp_url(raw: &str) -> anyhow::Result<SocketAddr> {
    let url = raw.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        anyhow::bail!("invalid rtp:// target url '{url}': missing scheme");
    };
    if !scheme.eq_ignore_ascii_case("rtp") {
        anyhow::bail!("invalid rtp:// target url '{url}': scheme must be rtp");
    }
    rest.parse::<SocketAddr>().map_err(|e| {
        anyhow::anyhow!(
            "invalid rtp:// target url '{url}': {e} \
             (the host must be an IP literal, IPv6 in brackets; \
             userinfo, path, query and fragment are not allowed)"
        )
    })
}

/// Validate a configured `rtp://` target: URL shape, port, multicast-only
/// options and their address-family rules. Called from `Config::validate`
/// (via `TargetConfig::validate`) so misconfiguration fails at startup
/// instead of surfacing once in a supervisor log line.
pub(crate) fn validate_rtp_target(target: &TargetConfig) -> anyhow::Result<()> {
    let dest = parse_rtp_url(&target.url)?;
    if dest.port() == 0 {
        anyhow::bail!(
            "invalid rtp:// target url '{}': port must be non-zero",
            target.url.trim()
        );
    }
    if let Some(ttl) = target.ttl
        && ttl > 255
    {
        anyhow::bail!("rtp:// target ttl must be <= 255, got {ttl}");
    }
    if !dest.ip().is_multicast() && (target.multicast_interface.is_some() || target.ttl.is_some()) {
        anyhow::bail!("multicast_interface and ttl are only valid with a multicast rtp:// target");
    }
    let interface = target
        .multicast_interface
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(interface) = interface {
        match dest.ip() {
            IpAddr::V4(group) => {
                interface.parse::<Ipv4Addr>().map_err(|_| {
                    anyhow::anyhow!(
                        "multicast_interface '{interface}' must be an IPv4 address \
                         for IPv4 group {group}"
                    )
                })?;
            }
            IpAddr::V6(group) => {
                resolve_v6_interface(interface, &group)?;
            }
        }
    }
    Ok(())
}

/// Resolve an interface name to its index for IPv6 multicast.
#[cfg(unix)]
fn if_name_to_index(name: &str) -> anyhow::Result<u32> {
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

#[cfg(not(unix))]
fn if_name_to_index(name: &str) -> anyhow::Result<u32> {
    anyhow::bail!(
        "interface names are not supported on this platform; \
         use an interface index instead of '{name}'"
    )
}

/// Resolve the outbound interface for an IPv6 multicast group given as an
/// interface index or name (interface indexes are not stable across reboots;
/// names are the durable identifier).
fn resolve_v6_interface(value: &str, group: &Ipv6Addr) -> anyhow::Result<u32> {
    if let Ok(index) = value.parse::<u32>() {
        return Ok(index);
    }
    if_name_to_index(value).map_err(|e| {
        anyhow::anyhow!(
            "multicast_interface '{value}' must be an interface index or name \
             for IPv6 group {group}: {e}"
        )
    })
}

/// Build the UDP socket sending towards `dest`: multicast TTL / outbound
/// interface options when `dest` is a group, a plain bound socket otherwise.
fn build_sender_socket(
    dest: SocketAddr,
    multicast_interface: Option<&str>,
    ttl: Option<u32>,
) -> anyhow::Result<UdpSocket> {
    let domain = match dest.ip() {
        IpAddr::V4(_) => socket2::Domain::IPV4,
        IpAddr::V6(_) => socket2::Domain::IPV6,
    };
    let socket = socket2::Socket::new(domain, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    socket.set_nonblocking(true)?;

    let bind_ip = match dest.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    socket
        .bind(&SocketAddr::new(bind_ip, 0).into())
        .map_err(|e| anyhow::anyhow!("Failed to bind UDP socket {bind_ip}:0: {e}"))?;

    let interface = multicast_interface.map(str::trim).filter(|s| !s.is_empty());
    match dest.ip() {
        IpAddr::V4(group) if group.is_multicast() => {
            socket.set_multicast_ttl_v4(ttl.unwrap_or(1))?;
            if let Some(interface) = interface {
                let addr: Ipv4Addr = interface.parse().map_err(|_| {
                    anyhow::anyhow!(
                        "multicast_interface '{interface}' must be an IPv4 address \
                         for IPv4 group {group}"
                    )
                })?;
                socket.set_multicast_if_v4(&addr)?;
            }
        }
        IpAddr::V6(group) if group.is_multicast() => {
            socket.set_multicast_hops_v6(ttl.unwrap_or(1))?;
            if let Some(interface) = interface {
                socket.set_multicast_if_v6(resolve_v6_interface(interface, &group)?)?;
            }
        }
        _ => {}
    }

    Ok(UdpSocket::from_std(socket.into())?)
}

/// Whether the stream currently has a live publisher session (a real WHIP
/// publisher or a source bridge's virtual one).
async fn has_publisher(manager: &Manager, stream: &str) -> bool {
    manager
        .info(vec![stream.to_string()])
        .await
        .first()
        .is_some_and(|s| s.publish.sessions.iter().any(|x| x.leave_at == 0))
}

/// Wait (bounded) for the publisher's tracks to appear. The change
/// notification is subscribed before the first snapshot so a track added in
/// between is still observed. The bus does not replay: media that became
/// available before the supervisor started is only visible through the
/// snapshot.
async fn wait_for_tracks(forward: &PeerForward) -> anyhow::Result<Vec<PublishTrackRemote>> {
    let mut rx = forward.subscribe_tracks_change();

    loop {
        let tracks = forward.publish_tracks().await;
        if !tracks.is_empty() {
            return Ok(tracks);
        }

        tokio::select! {
            _ = rx.recv() => continue,
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                anyhow::bail!("Timeout waiting for publish tracks");
            }
        }
    }
}

struct RtpTargetContext {
    manager: Arc<Manager>,
    stream: String,
    dest: SocketAddr,
    multicast_interface: Option<String>,
    ttl: Option<u32>,
    cancel: CancellationToken,
}

impl RtpTargetContext {
    fn new(manager: Arc<Manager>, stream: String, target: TargetConfig) -> anyhow::Result<Self> {
        let dest = parse_rtp_url(&target.url)
            .map_err(|e| anyhow::anyhow!("[{}] invalid RTP target: {}", stream, e))?;
        if dest.port() == 0 {
            anyhow::bail!("[{}] invalid RTP target: port must be non-zero", stream);
        }
        // A multicast-only option on a unicast destination would silently do
        // nothing; reject it here (config validation already reports it, this
        // covers programmatic targets).
        if !dest.ip().is_multicast()
            && (target.multicast_interface.is_some() || target.ttl.is_some())
        {
            anyhow::bail!(
                "[{}] multicast_interface and ttl are only valid with a multicast rtp:// target",
                stream
            );
        }
        let cancel = manager.cancel_token();
        Ok(Self {
            manager,
            stream,
            dest,
            multicast_interface: target.multicast_interface,
            ttl: target.ttl,
            cancel,
        })
    }

    async fn run(self) {
        // Subscribe before the initial snapshot/kick so media transitions
        // happening in between are still observed.
        let mut events = self.manager.subscribe_event();
        // Send epoch: while `Some`, per-track send tasks are running and
        // cancelled on teardown. A task exiting on its own also signals via
        // `exit_rx` (track set changed or a bus closed).
        let mut senders: Option<CancellationToken> = None;
        let (exit_tx, mut exit_rx) = mpsc::unbounded_channel::<()>();
        // Consecutive failed kick/sender attempts, reset once sending works.
        let mut failures: u32 = 0;
        let mut desired = has_publisher(&self.manager, &self.stream).await;
        // A configured target on an on-demand stream is standing demand:
        // whenever the stream has neither a publisher nor active senders,
        // kick its sources. Retried with the same backoff as send failures,
        // so an unreachable destination caps at roughly one source restart
        // per minute.
        #[cfg(feature = "source")]
        let standing_demand = self.manager.is_on_demand_stream(&self.stream);

        info!(
            "[target] [{}] rtp target towards {}",
            self.stream, self.dest
        );

        loop {
            #[cfg(feature = "source")]
            if standing_demand && !desired && senders.is_none() {
                if let Err(e) = self.manager.ensure_on_demand_source(&self.stream).await {
                    failures = failures.saturating_add(1);
                    let delay = reconnect_delay(failures);
                    warn!(
                        "[target] [{}] on-demand source start failed: {:?}; retrying in {:?}",
                        self.stream, e, delay
                    );
                    if self.wait(delay).await {
                        break;
                    }
                    desired = has_publisher(&self.manager, &self.stream).await;
                    continue;
                }
                // The kick blocks until the source bridge is up, so the
                // virtual publisher is already visible in the snapshot — no
                // need to wait for PublishStarted.
                desired = has_publisher(&self.manager, &self.stream).await;
            }

            if desired && senders.is_none() {
                match self.start_senders(&exit_tx).await {
                    Ok(epoch) => {
                        senders = Some(epoch);
                        failures = 0;
                    }
                    Err(e) => {
                        failures = failures.saturating_add(1);
                        let delay = reconnect_delay(failures);
                        warn!(
                            "[target] [{}] rtp send to {} failed: {:?}; retrying in {:?}",
                            self.stream, self.dest, e, delay
                        );
                        if self.wait(delay).await {
                            break;
                        }
                        // The backoff wait is event-blind: the media may have
                        // gone away mid-sleep, and sending now would stamp
                        // packets for a publisher that is already gone.
                        desired = has_publisher(&self.manager, &self.stream).await;
                        continue;
                    }
                }
            } else if !desired && senders.is_some() {
                debug!(
                    "[target] [{}] media gone; stopping rtp senders towards {}",
                    self.stream, self.dest
                );
                if let Some(epoch) = senders.take() {
                    epoch.cancel();
                }
                continue;
            }

            tokio::select! {
                _ = self.cancel.cancelled() => break,
                _ = exit_rx.recv() => {
                    // A send task ended: the publish tracks changed (codec
                    // switch, displacement) or a bus closed. Tear down and
                    // reconcile; the loop re-establishes against the current
                    // tracks when media is still there.
                    if let Some(epoch) = senders.take() {
                        epoch.cancel();
                    }
                    desired = has_publisher(&self.manager, &self.stream).await;
                }
                event = events.recv() => match event {
                    Ok(Event::PublishStarted { stream, .. }) => {
                        if stream == self.stream {
                            desired = true;
                        }
                    }
                    Ok(Event::PublishStopped { stream, .. }) => {
                        if stream == self.stream {
                            desired = false;
                            if let Some(epoch) = senders.take() {
                                epoch.cancel();
                            }
                        }
                    }
                    Ok(Event::StreamDeleted { stream, reason }) => {
                        if stream != self.stream {
                            continue;
                        }
                        // Only an outright removal ends the target. A
                        // provisioned stream cannot be removed (its resets
                        // arrive as a Reset pair), so this is defensive.
                        if reason != StreamDeleteReason::Reset {
                            info!(
                                "[target] [{}] stream deleted, stopping rtp send to {}",
                                self.stream, self.dest
                            );
                            break;
                        }
                    }
                    // Missed events may have lost a publish transition:
                    // reconcile against the manager's actual state.
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(
                            "[target] [{}] dropped {} stream events, reconciling",
                            self.stream, n
                        );
                        if let Some(epoch) = senders.take() {
                            epoch.cancel();
                        }
                        desired = has_publisher(&self.manager, &self.stream).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                },
            }
        }

        if let Some(epoch) = senders.take() {
            epoch.cancel();
        }
        info!(
            "[target] [{}] stopped rtp send to {}",
            self.stream, self.dest
        );
    }

    /// Open the destination socket and spawn one send task per media track
    /// (first video, first audio). Returns the epoch token cancelling the
    /// tasks on teardown.
    async fn start_senders(
        &self,
        exit_tx: &mpsc::UnboundedSender<()>,
    ) -> anyhow::Result<CancellationToken> {
        let Some(forward) = self.manager.get_forward(&self.stream).await else {
            anyhow::bail!("stream forward not available yet");
        };

        let tracks = wait_for_tracks(&forward).await?;

        let socket = Arc::new(build_sender_socket(
            self.dest,
            self.multicast_interface.as_deref(),
            self.ttl,
        )?);

        let epoch = CancellationToken::new();
        let mut count = 0usize;
        let mut video_seen = false;
        let mut audio_seen = false;
        for track in &tracks {
            let kind = track.kind();
            let is_video = kind == RtpCodecKind::Video;
            if is_video {
                if video_seen {
                    continue;
                }
                video_seen = true;
            } else if kind == RtpCodecKind::Audio {
                if audio_seen {
                    continue;
                }
                audio_seen = true;
            } else {
                continue;
            }

            // Audio rides port + 2 (RTP/AVP: port + 1 stays reserved for
            // RTCP). Checked here, not at config time, because whether the
            // stream has audio is only known once the tracks exist.
            let dest = if is_video {
                self.dest
            } else {
                let Some(port) = self.dest.port().checked_add(2) else {
                    anyhow::bail!(
                        "rtp target port {} leaves no room for the audio stream (+2)",
                        self.dest.port()
                    );
                };
                SocketAddr::new(self.dest.ip(), port)
            };

            // Nudge the publisher towards an IDR so receivers joining now
            // get a decodable frame (and the SPS/PPS injector a frame to
            // attach the parameter sets to) without waiting for the sender's
            // own keyframe cadence.
            if is_video {
                let ssrc = track.source_ssrc().await;
                if ssrc != 0
                    && let Err(e) = forward
                        .send_rtcp_to_publish(RtcpMessage::PictureLossIndication, ssrc)
                        .await
                {
                    debug!("[target] [{}] PLI nudge failed: {:?}", self.stream, e);
                }
            }

            tokio::spawn(track_send_task(
                track.clone(),
                socket.clone(),
                dest,
                epoch.child_token(),
                exit_tx.clone(),
            ));
            count += 1;
        }

        if count == 0 {
            anyhow::bail!("no video or audio publish tracks yet");
        }

        if self.dest.ip().is_multicast() {
            info!(
                "[target] [{}] multicasting to {} ({} track(s))",
                self.stream, self.dest, count
            );
        } else {
            info!(
                "[target] [{}] sending to {} ({} track(s))",
                self.stream, self.dest, count
            );
        }

        Ok(epoch)
    }

    /// Sleep for `delay`, returning `true` early when shutdown is requested.
    async fn wait(&self, delay: Duration) -> bool {
        tokio::select! {
            _ = self.cancel.cancelled() => true,
            _ = tokio::time::sleep(delay) => false,
        }
    }
}

async fn track_send_task(
    track: PublishTrackRemote,
    socket: Arc<UdpSocket>,
    dest: SocketAddr,
    cancel: CancellationToken,
    exit_tx: mpsc::UnboundedSender<()>,
) {
    let codec = track.codec();
    let payload_type = codec.sdp_payload_type();
    let is_video = codec.kind == "video";
    let mime = format!("{}/{}", codec.kind, codec.codec);
    // Video is re-assembled and re-packetized: SPS/PPS get inlined ahead of
    // every IDR (idempotent for streams that already carry them, so a
    // virtual track that inject_rtp already repacketized passes through
    // unchanged) and oversized frames become clean FU-A fragments. Audio
    // forwards untouched.
    let mut rp: Box<dyn RePayload + Send> = if is_video {
        Box::new(RePayloadCodec::with_sprop_params(mime, &codec.fmtp))
    } else {
        Box::<Forward>::default()
    };

    let mut rx = track.subscribe();
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            packet = rx.recv() => {
                match packet {
                    Ok(packet) => {
                        for mut out in rp.payload(&packet) {
                            out.header.payload_type = payload_type;
                            let mut buf = vec![0u8; out.marshal_size()];
                            if Marshal::marshal_to(&out, &mut buf).is_err() {
                                continue;
                            }
                            if let Err(e) = socket.send_to(&buf, dest).await {
                                debug!("[target] rtp send to {dest} failed: {e}");
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // The repayloader re-baselines its sequence numbers
                        // on the gap; forward-passthrough tracks do not care.
                        warn!("[target] lagged {n} packets towards {dest}");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    // Whatever the exit reason, tell the supervisor to reconcile: the track
    // set may have changed under it.
    let _ = exit_tx.send(());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtp_target(url: &str) -> TargetConfig {
        TargetConfig {
            url: url.to_string(),
            multicast_interface: None,
            ttl: None,
        }
    }

    #[test]
    fn parse_rtp_url_accepts_ip_literals() {
        assert_eq!(
            parse_rtp_url("rtp://230.1.1.1:1720").unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(230, 1, 1, 1)), 1720)
        );
        assert_eq!(
            parse_rtp_url("rtp://192.168.1.10:5004").unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), 5004)
        );
        assert_eq!(
            parse_rtp_url("rtp://[ff12::1]:1720").unwrap(),
            SocketAddr::new(IpAddr::V6("ff12::1".parse().unwrap()), 1720)
        );
        // Scheme is case-insensitive.
        assert_eq!(
            parse_rtp_url("RTP://230.1.1.1:1720").unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(230, 1, 1, 1)), 1720)
        );
        // Surrounding whitespace is trimmed.
        assert!(parse_rtp_url("  rtp://230.1.1.1:1720  ").is_ok());
    }

    #[test]
    fn parse_rtp_url_rejects_non_socket_addrs() {
        for url in [
            "rtp://",
            "http://230.1.1.1:1720",
            "rtp://230.1.1.1",
            "rtp://camera.local:1720",
            "rtp://user@230.1.1.1:1720",
            "rtp://230.1.1.1:1720/x",
            "rtp://230.1.1.1:1720?x=1",
            "rtp://230.1.1.1:1720#x",
            "rtp://230.1.1.1:99999",
        ] {
            assert!(parse_rtp_url(url).is_err(), "{url} must be rejected");
        }
    }

    #[test]
    fn validate_rtp_target_accepts_multicast_with_v4_interface() {
        let target = TargetConfig {
            multicast_interface: Some("192.168.1.10".to_string()),
            ttl: Some(16),
            ..rtp_target("rtp://230.1.1.1:1720")
        };
        validate_rtp_target(&target).unwrap();
    }

    #[test]
    fn validate_rtp_target_accepts_unicast_without_options() {
        validate_rtp_target(&rtp_target("rtp://192.168.1.10:5004")).unwrap();
        validate_rtp_target(&rtp_target("rtp://[ff12::1]:1720")).unwrap();
    }

    #[test]
    fn validate_rtp_target_rejects_port_zero_and_bad_ttl() {
        assert!(validate_rtp_target(&rtp_target("rtp://230.1.1.1:0")).is_err());

        let target = TargetConfig {
            ttl: Some(256),
            ..rtp_target("rtp://230.1.1.1:1720")
        };
        assert!(validate_rtp_target(&target).is_err());
    }

    #[test]
    fn validate_rtp_target_rejects_interface_family_mismatch() {
        // IPv4 groups take an IPv4 interface address, not a name or index.
        for interface in ["eth0", "2"] {
            let target = TargetConfig {
                multicast_interface: Some(interface.to_string()),
                ..rtp_target("rtp://230.1.1.1:1720")
            };
            let err = validate_rtp_target(&target).unwrap_err().to_string();
            assert!(
                err.contains("multicast_interface"),
                "error must name multicast_interface: {err}"
            );
        }

        // IPv6 groups take an index or name, not an IPv4 address.
        let target = TargetConfig {
            multicast_interface: Some("192.168.1.10".to_string()),
            ..rtp_target("rtp://[ff12::1]:1720")
        };
        assert!(validate_rtp_target(&target).is_err());
    }

    #[test]
    fn validate_rtp_target_rejects_multicast_options_on_unicast() {
        for (interface, ttl) in [(Some("192.168.1.10"), None), (None, Some(16))] {
            let target = TargetConfig {
                multicast_interface: interface.map(str::to_string),
                ttl,
                ..rtp_target("rtp://192.168.1.10:5004")
            };
            let err = validate_rtp_target(&target).unwrap_err().to_string();
            assert!(
                err.contains("multicast"),
                "error must say multicast-only: {err}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn validate_rtp_target_accepts_v6_interface_name_and_index() {
        // "lo" always exists on Linux; indexes are family-agnostic.
        let target = TargetConfig {
            multicast_interface: Some("lo".to_string()),
            ..rtp_target("rtp://[ff12::1]:1720")
        };
        validate_rtp_target(&target).unwrap();

        let target = TargetConfig {
            multicast_interface: Some("2".to_string()),
            ..rtp_target("rtp://[ff12::1]:1720")
        };
        validate_rtp_target(&target).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn multicast_sender_reaches_joined_loopback_receiver() {
        // The data plane: a datagram sent to the group out of the configured
        // interface must surface on a receiver joined on loopback — what
        // separates a real sender socket from one that merely bound.
        let group_ip = Ipv4Addr::new(230, 1, 1, 1);
        let group = SocketAddr::new(IpAddr::V4(group_ip), 1720);

        // Receiver: join the group on loopback (SO_REUSEADDR, like the SDP
        // source's bind_receiver_socket).
        let recv = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        recv.set_reuse_address(true).unwrap();
        recv.set_nonblocking(true).unwrap();
        recv.bind(&SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 1720).into())
            .unwrap();
        let receiver = UdpSocket::from_std(recv.into()).unwrap();
        receiver
            .join_multicast_v4(group_ip, Ipv4Addr::LOCALHOST)
            .unwrap();

        let sender = build_sender_socket(group, Some("127.0.0.1"), None).unwrap();

        let payload = b"live777-rtp-target-loopback";
        let mut buf = [0u8; 64];
        // Membership propagation is not instantaneous; resend until one
        // datagram makes it through.
        let received = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                sender.send_to(payload, group).await.unwrap();
                if let Ok(Ok((n, _))) =
                    tokio::time::timeout(Duration::from_millis(100), receiver.recv_from(&mut buf))
                        .await
                {
                    break n;
                }
            }
        })
        .await
        .expect("datagram sent to the group must reach the joined receiver");
        assert_eq!(&buf[..received], payload);
    }
}
