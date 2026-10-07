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
//! to port + 2, and each track's RTCP goes to its RTP port + 1 — while media
//! flows, every send task emits a periodic RTCP sender report there (packet
//! and octet counters plus the NTP ↔ RTP timestamp mapping, so receivers can
//! estimate jitter/loss and lip-sync). Inbound receiver reports are not
//! consumed. The video payload type defaults to the publisher's negotiated
//! PT (96 for dynamic codecs) and can be pinned from the config; audio
//! always keeps the automatic choice. On request a receiver-side SDP file
//! is written when sending starts (ffmpeg's `-sdp_file` behavior), directly
//! consumable by live777's own SDP file source.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use livetwo::payload::{Forward, RePayload, RePayloadCodec};
use livetwo::transport::multicast;
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::shared::marshal::{Marshal, MarshalSize};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::config::TargetConfig;
use crate::event::{Event, StreamDeleteReason};
use crate::forward::message::Codec;
use crate::forward::track::{PublishTrackRemote, system_time_to_ntp};
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
    validate_rtp_options(&dest, target).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Shared option validation for [`validate_rtp_target`] and
/// [`RtpTargetContext::new`] (which covers programmatic targets that never
/// went through `Config::validate`).
fn validate_rtp_options(dest: &SocketAddr, target: &TargetConfig) -> anyhow::Result<()> {
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
    if let Some(pt) = target.payload_type
        && !(96..=127).contains(&pt)
    {
        anyhow::bail!("rtp:// target payload_type must be in 96..=127 (dynamic range), got {pt}");
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
        multicast::resolve_interface(&dest.ip(), interface)?;
    }
    Ok(())
}
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
    /// Optional payload type stamped on the video track (audio keeps the
    /// automatic choice).
    payload_type: Option<u32>,
    /// Optional receiver-side SDP written when sending starts.
    sdp_file: Option<String>,
    cancel: CancellationToken,
    /// Test-only observability for the supervisor's restart behavior: the
    /// stale-exit regression test asserts a teardown/restart cycle does not
    /// multiply into a restart storm.
    #[cfg(test)]
    start_calls: Arc<std::sync::atomic::AtomicU64>,
}

impl RtpTargetContext {
    fn new(manager: Arc<Manager>, stream: String, target: TargetConfig) -> anyhow::Result<Self> {
        let dest = parse_rtp_url(&target.url)
            .map_err(|e| anyhow::anyhow!("[{}] invalid RTP target: {}", stream, e))?;
        // Config validation already reports these; re-checking covers
        // programmatic targets (a multicast-only option on a unicast
        // destination would otherwise silently do nothing).
        validate_rtp_options(&dest, &target)
            .map_err(|e| anyhow::anyhow!("[{}] invalid RTP target: {}", stream, e))?;
        let cancel = manager.cancel_token();
        Ok(Self {
            manager,
            stream,
            dest,
            multicast_interface: target.multicast_interface,
            ttl: target.ttl,
            payload_type: target.payload_type,
            sdp_file: target.sdp_file,
            cancel,
            #[cfg(test)]
            start_calls: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    async fn run(self) {
        // Subscribe before the initial snapshot/kick so media transitions
        // happening in between are still observed.
        let mut events = self.manager.subscribe_event();
        // Send epoch: while `Some`, per-track send tasks are running and
        // cancelled on teardown. A task exiting on its own also signals via
        // `exit_rx` (track set changed or a bus closed). The generation tag
        // distinguishes a live epoch's spontaneous exit from a belated exit
        // of an already torn-down epoch — without it a stale message would
        // cancel the *next* epoch, whose tasks report their own exits in
        // turn, self-sustaining a restart storm.
        let mut senders: Option<(u64, CancellationToken)> = None;
        let mut generation: u64 = 0;
        let (exit_tx, mut exit_rx) = mpsc::unbounded_channel::<u64>();
        // Consecutive failed kick/sender attempts, reset once sending works.
        let mut failures: u32 = 0;
        let mut desired = self.manager.has_publisher(&self.stream).await;
        // A configured target on an on-demand stream is standing demand:
        // whenever the stream has neither a publisher nor active senders,
        // kick its sources. Retried with the same backoff as send failures,
        // so an unreachable destination caps at roughly one source restart
        // per minute.
        #[cfg(feature = "source")]
        let standing_demand = self.manager.is_on_demand_stream(&self.stream);
        // Register as a virtual subscriber for the supervisor's whole
        // lifetime: the senders tap the forward track broadcast directly (no
        // subscribe session), so without this the on-demand idle check would
        // stop the sources every close_after underneath them (live777#481).
        // The registration also lists the target in the stream's
        // `subscribe.sessions`.
        let virtual_id = crate::target::virtual_target_session_id(&format!("rtp://{}", self.dest));
        self.manager
            .add_virtual_subscriber(&self.stream, virtual_id.clone())
            .await;

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
                    desired = self.manager.has_publisher(&self.stream).await;
                    continue;
                }
                // The kick blocks until the source bridge is up, so the
                // virtual publisher is already visible in the snapshot — no
                // need to wait for PublishStarted.
                desired = self.manager.has_publisher(&self.stream).await;
            }

            if desired && senders.is_none() {
                generation = generation.wrapping_add(1);
                match self.start_senders(generation, &exit_tx).await {
                    Ok(epoch) => {
                        senders = Some((generation, epoch));
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
                        desired = self.manager.has_publisher(&self.stream).await;
                        continue;
                    }
                }
            } else if !desired && senders.is_some() {
                debug!(
                    "[target] [{}] media gone; stopping rtp senders towards {}",
                    self.stream, self.dest
                );
                if let Some((_, epoch)) = senders.take() {
                    epoch.cancel();
                }
                continue;
            }

            tokio::select! {
                _ = self.cancel.cancelled() => break,
                exited = exit_rx.recv() => {
                    // A send task ended: the publish tracks changed (codec
                    // switch, displacement) or a bus closed. Tear down and
                    // reconcile; the loop re-establishes against the current
                    // tracks when media is still there. Exits of an epoch the
                    // supervisor already tore down are stale and ignored.
                    let current = senders.as_ref().is_some_and(|(tag, _)| Some(*tag) == exited);
                    if current
                        && let Some((_, epoch)) = senders.take()
                    {
                        epoch.cancel();
                        desired = self.manager.has_publisher(&self.stream).await;
                        // Pace the rebuild the way the WHIP target paces a
                        // dead push session (the SubscribeStopped arm): a
                        // flapping track set (a publisher renegotiating in a
                        // loop) must not rebind the socket, rewrite the SDP
                        // file and PLI the publisher at the churn rate
                        // (live777#477). An epoch that was up resets the
                        // backoff, but the first retry still waits the base
                        // delay. No wait when the media is gone.
                        failures = 1;
                        if desired {
                            if self.wait(reconnect_delay(1)).await {
                                break;
                            }
                            // The wait is event-blind: re-check the media is
                            // still there before rebuilding.
                            desired = self.manager.has_publisher(&self.stream).await;
                        }
                    }
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
                            if let Some((_, epoch)) = senders.take() {
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
                        let mut torn_down = false;
                        if let Some((_, epoch)) = senders.take() {
                            epoch.cancel();
                            torn_down = true;
                        }
                        desired = self.manager.has_publisher(&self.stream).await;
                        // Same rebuild pacing as the spontaneous-exit path,
                        // including the post-wait media re-check.
                        if torn_down && desired {
                            failures = 1;
                            if self.wait(reconnect_delay(1)).await {
                                break;
                            }
                            desired = self.manager.has_publisher(&self.stream).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                },
            }
        }

        if let Some((_, epoch)) = senders.take() {
            epoch.cancel();
        }
        self.manager
            .remove_virtual_subscriber(&self.stream, &virtual_id)
            .await;
        info!(
            "[target] [{}] stopped rtp send to {}",
            self.stream, self.dest
        );
    }

    /// Open the destination socket and spawn one send task per media track
    /// (first video, first audio). Returns the epoch token cancelling the
    /// tasks on teardown. `generation` tags the tasks' exit notifications so
    /// the supervisor can tell a live epoch's spontaneous exit from a stale
    /// one.
    async fn start_senders(
        &self,
        generation: u64,
        exit_tx: &mpsc::UnboundedSender<u64>,
    ) -> anyhow::Result<CancellationToken> {
        #[cfg(test)]
        self.start_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let Some(forward) = self.manager.get_forward(&self.stream).await else {
            anyhow::bail!("stream forward not available yet");
        };

        let tracks = wait_for_tracks(&forward).await?;

        let socket = Arc::new(multicast::sender_socket(
            self.dest,
            self.multicast_interface.as_deref(),
            self.ttl,
        )?);

        // Select the first video and the first audio track; every other
        // track is ignored. Per-track resolved payload type and destination
        // ride along: the video PT may be overridden from the config, audio
        // always keeps the automatic choice (one value cannot fit both
        // media), audio rides port + 2, and each track's RTCP (sender
        // reports) goes to its RTP port + 1 — the RTP/AVP convention. The
        // port overflow is checked here, not at config time, because whether
        // the stream has audio is only known once the tracks exist.
        let mut selected: Vec<SelectedTrack> = Vec::new();
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

            let codec = track.codec();
            let payload_type = if is_video {
                self.payload_type
                    .map(|pt| pt as u8)
                    .unwrap_or_else(|| codec.sdp_payload_type())
            } else {
                codec.sdp_payload_type()
            };
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
            // Sender reports go to the RTP port + 1. An overflowing port is
            // not worth failing an otherwise working send for: log once and
            // skip RTCP for the track.
            let rtcp_dest = match dest.port().checked_add(1) {
                Some(port) => Some(SocketAddr::new(dest.ip(), port)),
                None => {
                    warn!(
                        "[target] [{}] rtp dest port {} leaves no room for RTCP (+1); \
                         not sending sender reports for the {:?} track",
                        self.stream,
                        dest.port(),
                        kind
                    );
                    None
                }
            };
            selected.push(SelectedTrack {
                track: track.clone(),
                kind,
                codec,
                payload_type,
                dest,
                rtcp_dest,
            });
        }

        if selected.is_empty() {
            anyhow::bail!("no video or audio publish tracks yet");
        }

        // Receiver-side SDP, rewritten on every send start (ffmpeg's
        // -sdp_file behavior): the file describes a multicast group or a
        // unicast placeholder address that stays valid while the sender is
        // away. A write failure must never stop the media: it is an aid,
        // not a precondition.
        if let Some(path) = &self.sdp_file {
            let sdp = build_receiver_sdp(
                &self.stream,
                self.dest,
                &selected
                    .iter()
                    .map(|sel| (sel.kind, sel.codec.clone(), sel.payload_type))
                    .collect::<Vec<_>>(),
            );
            if let Err(e) = tokio::fs::write(path, sdp).await {
                warn!(
                    "[target] [{}] failed to write receiver SDP to {}: {:?}",
                    self.stream, path, e
                );
            }
        }

        let epoch = CancellationToken::new();
        for sel in &selected {
            // Nudge the publisher towards an IDR so receivers joining now
            // get a decodable frame (and the SPS/PPS injector a frame to
            // attach the parameter sets to) without waiting for the sender's
            // own keyframe cadence.
            if sel.kind == RtpCodecKind::Video {
                let ssrc = sel.track.source_ssrc().await;
                if ssrc != 0
                    && let Err(e) = forward
                        .send_rtcp_to_publish(RtcpMessage::PictureLossIndication, ssrc)
                        .await
                {
                    debug!("[target] [{}] PLI nudge failed: {:?}", self.stream, e);
                }
            }

            tokio::spawn(track_send_task(
                TrackSend {
                    track: sel.track.clone(),
                    socket: socket.clone(),
                    dest: sel.dest,
                    rtcp_dest: sel.rtcp_dest,
                    payload_type: sel.payload_type,
                    is_video: sel.codec.kind == "video",
                    generation,
                },
                epoch.child_token(),
                exit_tx.clone(),
            ));
        }

        if self.dest.ip().is_multicast() {
            info!(
                "[target] [{}] multicasting to {} ({} track(s))",
                self.stream,
                self.dest,
                selected.len()
            );
        } else {
            info!(
                "[target] [{}] sending to {} ({} track(s))",
                self.stream,
                self.dest,
                selected.len()
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

/// Build a receiver-side SDP describing a target's output: the multicast
/// group as the connection address for multicast destinations (a unicast
/// destination gets a loopback placeholder the receiver must replace),
/// video on the URL's port, audio on port + 2, and the resolved payload
/// types. The result parses with `rtsp::parse_media_info_from_sdp`, so
/// another live777 can consume it directly as an SDP file source, like
/// ffmpeg's `-sdp_file` output.
fn build_receiver_sdp(
    stream: &str,
    dest: SocketAddr,
    tracks: &[(RtpCodecKind, Codec, u8)],
) -> String {
    let (origin, connection) = match dest.ip() {
        IpAddr::V4(v4) if v4.is_multicast() => {
            ("o=- 0 0 IN IP4 127.0.0.1", format!("c=IN IP4 {v4}"))
        }
        IpAddr::V6(v6) if v6.is_multicast() => ("o=- 0 0 IN IP6 ::1", format!("c=IN IP6 {v6}")),
        // Unicast receivers must point the connection address at themselves.
        IpAddr::V4(_) => ("o=- 0 0 IN IP4 127.0.0.1", "c=IN IP4 127.0.0.1".to_string()),
        IpAddr::V6(_) => ("o=- 0 0 IN IP6 ::1", "c=IN IP6 ::1".to_string()),
    };
    // A CR/LF in the stream name would break the file's line structure.
    let session: String = stream
        .chars()
        .filter(|c| !matches!(c, '\r' | '\n'))
        .collect();

    let mut lines = vec![
        "v=0".to_string(),
        origin.to_string(),
        format!("s=live777-{session}"),
        connection,
        "t=0 0".to_string(),
    ];

    for (kind, codec, pt) in tracks {
        let (media, port, channels) = match kind {
            RtpCodecKind::Video => ("video", dest.port(), None),
            RtpCodecKind::Audio => (
                "audio",
                dest.port().saturating_add(2),
                // 0 means the negotiated rtpmap carried no channel count
                // (conventional for G.711/G.722): omit the parameter rather
                // than emit an invalid `/0`.
                Some(codec.channels as u8).filter(|ch| *ch > 0),
            ),
            _ => continue,
        };
        let (media, clock_rate, channels) = match codec.codec.as_str() {
            "h264" | "h265" | "hevc" | "vp8" | "vp9" | "av1" => (media, codec.clock_rate, None),
            "opus" | "g722" | "pcma" | "pcmu" => (media, codec.clock_rate, channels),
            _ => continue,
        };

        lines.push(format!("m={media} {port} RTP/AVP {pt}"));
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
    }

    lines.join("\r\n") + "\r\n"
}

/// A publish track selected for sending, with its resolved payload type,
/// RTP destination and RTCP (sender report) destination (`None` when the
/// RTP port + 1 overflows).
struct SelectedTrack {
    track: PublishTrackRemote,
    kind: RtpCodecKind,
    codec: Codec,
    payload_type: u8,
    dest: SocketAddr,
    rtcp_dest: Option<SocketAddr>,
}

/// One send task's wiring: the tapped track, the shared socket, its
/// destination and payload type, and the epoch generation tagged onto the
/// exit notification.
struct TrackSend {
    track: PublishTrackRemote,
    socket: Arc<UdpSocket>,
    dest: SocketAddr,
    /// The track's RTCP destination (RTP port + 1); periodic sender reports
    /// are sent here while media flows. `None` disables them.
    rtcp_dest: Option<SocketAddr>,
    payload_type: u8,
    is_video: bool,
    generation: u64,
}

/// How often each send task emits an RTCP sender report (mirroring the RTSP
/// server's sender-report loop).
const SR_INTERVAL: Duration = Duration::from_secs(5);

async fn track_send_task(
    send: TrackSend,
    cancel: CancellationToken,
    exit_tx: mpsc::UnboundedSender<u64>,
) {
    let TrackSend {
        track,
        socket,
        dest,
        rtcp_dest,
        payload_type,
        is_video,
        generation,
    } = send;
    let codec = track.codec();
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
    // Sender-report state, fed at the marshal point: receivers estimating
    // jitter/loss or lip-syncing from SRs need the wire-accurate
    // (NTP wall clock, RTP timestamp) pair and packet/octet counters of
    // what this task actually put on the socket. The RTP timestamp is the
    // outgoing (re-based) one, so receivers can map it directly.
    let mut sr_tick = tokio::time::interval(SR_INTERVAL);
    let mut sr_packets: u32 = 0;
    let mut sr_octets: u32 = 0;
    let mut sr_ssrc: u32 = 0;
    let mut sr_rtp_time: u32 = 0;
    let mut sr_ntp_time_ms: u64 = 0;
    // Reused across packets: high-bitrate video means a thousand marshals a
    // second.
    let mut buf = Vec::with_capacity(1500);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = sr_tick.tick() => {
                let Some(rtcp_dest) = rtcp_dest else {
                    continue;
                };
                // Nothing sent yet (the first tick fires immediately): an SR
                // with zero counters and no timestamp mapping is useless.
                if sr_packets == 0 {
                    continue;
                }
                let report = rtc_rtcp::sender_report::SenderReport {
                    ssrc: sr_ssrc,
                    ntp_time: system_time_to_ntp(
                        std::time::UNIX_EPOCH + Duration::from_millis(sr_ntp_time_ms),
                    ),
                    rtp_time: sr_rtp_time,
                    packet_count: sr_packets,
                    octet_count: sr_octets,
                    ..Default::default()
                };
                match report.marshal() {
                    Ok(bytes) => {
                        if let Err(e) = socket.send_to(&bytes, rtcp_dest).await {
                            debug!("[target] rtcp send to {rtcp_dest} failed: {e}");
                        }
                    }
                    Err(e) => debug!("[target] failed to marshal sender report: {e}"),
                }
            }
            packet = rx.recv() => {
                match packet {
                    Ok(packet) => {
                        for mut out in rp.payload(&packet) {
                            out.header.payload_type = payload_type;
                            buf.clear();
                            buf.resize(out.marshal_size(), 0);
                            if Marshal::marshal_to(&out, &mut buf).is_err() {
                                continue;
                            }
                            if let Err(e) = socket.send_to(&buf, dest).await {
                                debug!("[target] rtp send to {dest} failed: {e}");
                            } else {
                                sr_packets = sr_packets.wrapping_add(1);
                                // RFC 3550 octet count: payload only.
                                sr_octets =
                                    sr_octets.wrapping_add(out.payload.len() as u32);
                                sr_ssrc = out.header.ssrc;
                                sr_rtp_time = out.header.timestamp;
                                sr_ntp_time_ms = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis()
                                    as u64;
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

    // Whatever the exit reason, tell the supervisor which epoch ended so it
    // can reconcile: the track set may have changed under it. The generation
    // tag lets it ignore this message when it already tore the epoch down.
    let _ = exit_tx.send(generation);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn rtp_target(url: &str) -> TargetConfig {
        TargetConfig {
            url: url.to_string(),
            multicast_interface: None,
            ttl: None,
            payload_type: None,
            sdp_file: None,
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

    fn test_codec(
        kind: &str,
        codec: &str,
        fmtp: &str,
        payload_type: u8,
        clock_rate: u32,
        channels: u16,
    ) -> Codec {
        Codec {
            kind: kind.to_string(),
            codec: codec.to_string(),
            fmtp: fmtp.to_string(),
            payload_type,
            clock_rate,
            channels,
        }
    }

    const H264_FMTP: &str = "profile-level-id=42001f;packetization-mode=1;sprop-parameter-sets=Z0IAH5WoFAFuQA==,aM4yyA==";

    #[test]
    fn build_receiver_sdp_describes_multicast_group_and_tracks() {
        let dest = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(230, 1, 1, 1)), 1720);
        let tracks = [
            (
                RtpCodecKind::Video,
                test_codec("video", "h264", H264_FMTP, 96, 90000, 0),
                96u8,
            ),
            (
                RtpCodecKind::Audio,
                test_codec("audio", "opus", "", 111, 48000, 2),
                111u8,
            ),
        ];

        let sdp = build_receiver_sdp("robot-cam", dest, &tracks);

        assert!(
            sdp.starts_with(
                "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=live777-robot-cam\r\nc=IN IP4 230.1.1.1\r\n"
            ),
            "unexpected session head: {sdp}"
        );
        // Video on the URL's port, audio on port + 2 (RTP/AVP).
        assert!(sdp.contains("m=video 1720 RTP/AVP 96\r\n"), "SDP: {sdp}");
        assert!(sdp.contains("a=rtpmap:96 H264/90000\r\n"), "SDP: {sdp}");
        assert!(
            sdp.contains(&format!("a=fmtp:96 {H264_FMTP}\r\n")),
            "SDP: {sdp}"
        );
        assert!(sdp.contains("m=audio 1722 RTP/AVP 111\r\n"), "SDP: {sdp}");
        assert!(sdp.contains("a=rtpmap:111 OPUS/48000/2\r\n"), "SDP: {sdp}");
    }

    #[test]
    fn build_receiver_sdp_uses_resolved_pt_and_unicast_placeholder() {
        // A unicast destination cannot know the receiver's address: the
        // connection address is a placeholder the receiver must replace.
        let dest = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), 5004);
        let tracks = [(
            RtpCodecKind::Video,
            test_codec("video", "h264", "", 96, 90000, 0),
            120u8,
        )];

        let sdp = build_receiver_sdp("cam", dest, &tracks);

        assert!(sdp.contains("c=IN IP4 127.0.0.1\r\n"), "SDP: {sdp}");
        // The resolved (overridden) PT is what the m= line advertises.
        assert!(sdp.contains("m=video 5004 RTP/AVP 120\r\n"), "SDP: {sdp}");
        // An empty fmtp leaves no a=fmtp line behind.
        assert!(
            !sdp.contains("a=fmtp:120"),
            "empty fmtp must not emit an a=fmtp line: {sdp}"
        );
    }

    /// The generated file must be directly consumable by live777's own SDP
    /// file source: `rtsp::parse_media_info_from_sdp` is the parser behind
    /// `[[stream.<name>.sources]] url = "*.sdp"`.
    #[cfg(any(feature = "source", feature = "rtsp"))]
    #[test]
    fn receiver_sdp_round_trips_through_the_sdp_source_parser() {
        let dest = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(230, 1, 1, 1)), 1720);
        let tracks = [(
            RtpCodecKind::Video,
            test_codec("video", "h264", H264_FMTP, 96, 90000, 0),
            96u8,
        )];

        let sdp = build_receiver_sdp("robot-cam", dest, &tracks);

        let parsed = rtsp::parse_media_info_from_sdp(sdp.as_bytes()).unwrap();
        match parsed.video_codec {
            Some(rtsp::VideoCodecParams::H264 {
                payload_type,
                clock_rate,
                sps,
                pps,
                ..
            }) => {
                assert_eq!(payload_type, 96);
                assert_eq!(clock_rate, 90000);
                assert!(
                    !sps.is_empty() && !pps.is_empty(),
                    "sprop parameter sets must survive the round trip"
                );
            }
            other => panic!("expected H264, got {other:?}"),
        }

        // The video media line carries the URL's port.
        let m_video = sdp.lines().find(|l| l.starts_with("m=video")).unwrap();
        assert!(m_video.starts_with("m=video 1720 RTP/AVP 96"));
    }

    /// Receive datagrams until one contains `idr`, asserting every datagram
    /// was re-stamped to payload type `pt`. Returns the RTP payloads (the
    /// fixed 12-byte header stripped) in arrival order.
    #[cfg(all(
        feature = "source",
        any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "source-whep",
            feature = "rtsp",
            feature = "native-source"
        )
    ))]
    async fn recv_idr_stream(listener: &UdpSocket, idr: &[u8], pt: u8) -> Vec<Vec<u8>> {
        let mut buf = [0u8; 1500];
        let mut payloads = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (n, _) = listener.recv_from(&mut buf).await.unwrap();
                assert_eq!(buf[1] & 0x7f, pt, "payload type must be re-stamped to {pt}");
                // These tests produce no CSRC lists or extensions, so the
                // header is exactly the fixed 12 bytes.
                let payload = buf[12..n].to_vec();
                let done = payload.windows(idr.len()).any(|w| w == idr);
                payloads.push(payload);
                if done {
                    break;
                }
            }
        })
        .await
        .expect("timed out waiting for the IDR datagram");
        payloads
    }

    /// H264 codec params whose fmtp seeds SPS/PPS through
    /// `sprop-parameter-sets` (the SDPs both Unitree cameras and live777's
    /// own SDP carry).
    #[cfg(all(
        feature = "source",
        any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "source-whep",
            feature = "rtsp",
            feature = "native-source"
        )
    ))]
    fn h264_codec_params() -> rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters {
        use base64::Engine;
        use rtc::peer_connection::configuration::media_engine::MIME_TYPE_H264;
        use rtc::rtp_transceiver::rtp_sender::RTCRtpCodec;

        const SPS: [u8; 5] = [0x67, 0x42, 0x00, 0x1f, 0xaa];
        const PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];

        let b64 = base64::engine::general_purpose::STANDARD;
        let fmtp = format!(
            "profile-level-id=42001f;packetization-mode=1;sprop-parameter-sets={},{}",
            b64.encode(SPS),
            b64.encode(PPS)
        );
        rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_H264.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: fmtp,
                rtcp_feedback: vec![],
            },
            payload_type: 96,
        }
    }

    /// Build the H264 IDR packet (bare single NAL, marker set) that the
    /// data-plane tests publish. `payload_type` deliberately differs from
    /// the negotiated 96 so the re-stamp is observable.
    #[cfg(all(
        feature = "source",
        any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "source-whep",
            feature = "rtsp",
            feature = "native-source"
        )
    ))]
    fn h264_idr_packet(payload_type: u8) -> rtc::rtp::packet::Packet {
        use bytes::Bytes;

        const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x21];
        rtc::rtp::packet::Packet {
            header: rtc::rtp::header::Header {
                version: 2,
                marker: true,
                payload_type,
                sequence_number: 1,
                timestamp: 1000,
                ssrc: 0x1234,
                ..Default::default()
            },
            payload: Bytes::from(IDR.to_vec()),
        }
    }

    /// Publish IDRs on `track` every 50 ms until `stop` fires. The send
    /// task subscribes to the track broadcast asynchronously, and a
    /// broadcast receiver does not see messages sent before it subscribed,
    /// so the first injections may be dropped; injection errors (no
    /// receivers yet, or none left) are expected and ignored.
    #[cfg(all(
        feature = "source",
        any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "source-whep",
            feature = "rtsp",
            feature = "native-source"
        )
    ))]
    async fn pump_idrs(track: PublishTrackRemote, stop: CancellationToken) {
        loop {
            if let PublishTrackRemote::Virtual(v) = &track {
                let _ = v.inject_rtp(Arc::new(h264_idr_packet(105)));
            }
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
    }

    /// The send task's data plane: packets broadcast on a publish track
    /// reach the UDP destination with the payload type re-stamped to the
    /// codec's SDP payload type, and the sprop-seeded SPS/PPS are inlined
    /// ahead of the IDR so a receiver joining mid-GOP can decode.
    #[cfg(all(
        feature = "source",
        any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "source-whep",
            feature = "rtsp",
            feature = "native-source"
        )
    ))]
    #[tokio::test]
    async fn track_send_task_restamps_pt_and_injects_seeded_params() {
        use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;

        use crate::forward::track::VirtualPublishTrack;

        const SPS: [u8; 5] = [0x67, 0x42, 0x00, 0x1f, 0xaa];
        const PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];
        const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x21];

        let track = PublishTrackRemote::Virtual(Arc::new(VirtualPublishTrack::new(
            "test".to_string(),
            RtpCodecKind::Video,
            h264_codec_params(),
        )));

        let socket =
            Arc::new(multicast::sender_socket("127.0.0.1:0".parse().unwrap(), None, None).unwrap());
        let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let dest = listener.local_addr().unwrap();

        let cancel = CancellationToken::new();
        let (exit_tx, mut exit_rx) = mpsc::unbounded_channel::<u64>();
        let task = tokio::spawn(track_send_task(
            TrackSend {
                track: track.clone(),
                socket,
                dest,
                rtcp_dest: None,
                payload_type: 96,
                is_video: true,
                generation: 7,
            },
            cancel.clone(),
            exit_tx,
        ));

        let pump_stop = CancellationToken::new();
        tokio::spawn(pump_idrs(track, pump_stop.clone()));

        let datagrams = recv_idr_stream(&listener, &IDR, 96).await;
        pump_stop.cancel();
        // The payloader aggregates the parameter sets into a STAP-A ahead
        // of the IDR single-NAL packet (both RFC 6184), so two datagrams:
        // [STAP-A(SPS, PPS)], [IDR].
        assert_eq!(
            datagrams.len(),
            2,
            "expected a parameter-set datagram and an IDR datagram: {datagrams:02x?}"
        );
        let find = |haystack: &[u8], needle: &[u8]| {
            haystack.windows(needle.len()).position(|w| w == needle)
        };
        let (sps_at, pps_at) = (find(&datagrams[0], &SPS), find(&datagrams[0], &PPS));
        assert!(
            sps_at.is_some() && pps_at.is_some(),
            "parameter sets must be inlined ahead of the IDR: {datagrams:02x?}"
        );
        assert!(sps_at < pps_at, "SPS must precede PPS: {datagrams:02x?}");
        assert_eq!(datagrams[1], IDR.to_vec(), "datagrams: {datagrams:02x?}");

        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("send task must exit on cancel")
            .unwrap();
        // The task notifies its exit (tagged with its epoch generation) so
        // the supervisor can reconcile.
        let exited = tokio::time::timeout(Duration::from_secs(5), exit_rx.recv())
            .await
            .expect("send task must signal its exit");
        assert_eq!(exited, Some(7), "the exit carries the epoch generation");
    }

    /// Bind a consecutive (rtp, rtp + 1) UDP socket pair on loopback,
    /// retrying against a taken +1 port.
    #[cfg(all(
        feature = "source",
        any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "source-whep",
            feature = "rtsp",
            feature = "native-source"
        )
    ))]
    async fn bind_rtp_rtcp_pair() -> (UdpSocket, UdpSocket) {
        for _ in 0..32 {
            let rtp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let mut rtcp_addr = rtp.local_addr().unwrap();
            rtcp_addr.set_port(rtcp_addr.port() + 1);
            if let Ok(rtcp) = UdpSocket::bind(rtcp_addr).await {
                return (rtp, rtcp);
            }
        }
        panic!("could not bind a consecutive RTP/RTCP port pair");
    }

    /// The send task emits a periodic RTCP sender report to the RTP port + 1
    /// while media flows: it carries the outgoing SSRC, the last sent
    /// packet's (NTP, RTP) timestamp pair, and the packet/octet counters of
    /// what was actually put on the socket (live777#471).
    #[cfg(all(
        feature = "source",
        any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "source-whep",
            feature = "rtsp",
            feature = "native-source"
        )
    ))]
    #[tokio::test]
    async fn track_send_task_emits_sender_reports() {
        use crate::forward::track::VirtualPublishTrack;

        let track = PublishTrackRemote::Virtual(Arc::new(VirtualPublishTrack::new(
            "test".to_string(),
            RtpCodecKind::Video,
            h264_codec_params(),
        )));

        let socket =
            Arc::new(multicast::sender_socket("127.0.0.1:0".parse().unwrap(), None, None).unwrap());
        let socket_port = socket.local_addr().unwrap().port();
        let (listener, rtcp_listener) = bind_rtp_rtcp_pair().await;
        let dest = listener.local_addr().unwrap();
        let rtcp_dest = rtcp_listener.local_addr().unwrap();
        assert_eq!(rtcp_dest.port(), dest.port() + 1, "test setup: RTCP on +1");

        let cancel = CancellationToken::new();
        let (exit_tx, _exit_rx) = mpsc::unbounded_channel::<u64>();
        let task = tokio::spawn(track_send_task(
            TrackSend {
                track: track.clone(),
                socket,
                dest,
                rtcp_dest: Some(rtcp_dest),
                payload_type: 96,
                is_video: true,
                generation: 1,
            },
            cancel.clone(),
            exit_tx,
        ));

        let pump_stop = CancellationToken::new();
        tokio::spawn(pump_idrs(track, pump_stop.clone()));

        // The first report arrives after one SR_INTERVAL; allow slack for
        // CI scheduling.
        let mut buf = [0u8; 1500];
        let (n, from) = tokio::time::timeout(
            SR_INTERVAL + Duration::from_secs(10),
            rtcp_listener.recv_from(&mut buf),
        )
        .await
        .expect("a sender report must arrive")
        .unwrap();
        assert_eq!(from.port(), socket_port);
        // RTCP SR carries packet type 200 in byte 1 (RTP would have the
        // re-stamped payload type 96 there): no RTP leaks onto the RTCP port.
        assert_eq!(buf[1], 200, "the datagram must be RTCP, not RTP");

        let mut cursor = std::io::Cursor::new(&buf[..n]);
        let packets = rtc_rtcp::packet::unmarshal(&mut cursor).unwrap();
        let sr = packets
            .iter()
            .find_map(|p| {
                p.as_any()
                    .downcast_ref::<rtc_rtcp::sender_report::SenderReport>()
            })
            .expect("the datagram must be a sender report");
        // The pumped packets all carry SSRC 0x1234 and timestamp 1000; the
        // SR reports the last sent packet's values.
        assert_eq!(sr.ssrc, 0x1234);
        assert_eq!(sr.rtp_time, 1000);
        assert!(sr.ntp_time != 0, "the NTP wall clock must be mapped");
        assert!(
            sr.packet_count >= 2,
            "packets were sent: {}",
            sr.packet_count
        );
        // One frame on the wire is a 13-byte STAP-A plus the 4-byte IDR.
        assert!(
            sr.octet_count >= 17,
            "octet count covers the sent payload bytes: {}",
            sr.octet_count
        );

        pump_stop.cancel();
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("send task must exit on cancel")
            .unwrap();
    }

    /// `start_senders` end to end against a real manager and forward: the
    /// stream's virtual track is wired to the UDP destination and its media
    /// arrives re-stamped, with the seeded parameter sets inlined.
    #[cfg(all(
        feature = "source",
        any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "source-whep",
            feature = "rtsp",
            feature = "native-source"
        )
    ))]
    #[tokio::test]
    async fn start_senders_streams_virtual_track_to_udp() {
        use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;

        const SPS: [u8; 5] = [0x67, 0x42, 0x00, 0x1f, 0xaa];
        const PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];
        const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x21];

        let cancel = CancellationToken::new();
        let manager =
            Arc::new(Manager::new(crate::config::Config::default(), cancel.clone()).await);
        manager.stream_create("cam".to_string()).await.unwrap();
        let forward = manager.get_forward("cam").await.unwrap();
        forward
            .add_virtual_track(RtpCodecKind::Video, h264_codec_params())
            .await
            .unwrap();

        let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let dest = listener.local_addr().unwrap();

        // A receiver-SDP write failure must not fail the send: the file is
        // an aid, not a precondition. This pass also exercises the video
        // payload_type override (105 on the wire, nothing here reads it).
        let bad_sdp_path = "/nonexistent-live777-dir/robot-cam.sdp";
        let ctx_bad_sdp = RtpTargetContext::new(
            manager.clone(),
            "cam".to_string(),
            TargetConfig {
                payload_type: Some(105),
                sdp_file: Some(bad_sdp_path.to_string()),
                ..rtp_target(&format!("rtp://{dest}"))
            },
        )
        .unwrap();
        let (bad_exit_tx, mut bad_exit_rx) = mpsc::unbounded_channel::<u64>();
        let bad_epoch = ctx_bad_sdp.start_senders(1, &bad_exit_tx).await.unwrap();
        bad_epoch.cancel();
        tokio::time::timeout(Duration::from_secs(5), bad_exit_rx.recv())
            .await
            .expect("send task must signal its exit");
        assert!(!std::path::Path::new(bad_sdp_path).exists());

        // The real pass: a receiver-side SDP file is written on send start.
        let sdp_file = tempfile::NamedTempFile::new().unwrap();
        let sdp_path = sdp_file.path().to_path_buf();
        let ctx = RtpTargetContext::new(
            manager.clone(),
            "cam".to_string(),
            TargetConfig {
                sdp_file: Some(sdp_path.to_string_lossy().into_owned()),
                ..rtp_target(&format!("rtp://{dest}"))
            },
        )
        .unwrap();

        let (exit_tx, mut exit_rx) = mpsc::unbounded_channel::<u64>();
        let epoch = ctx.start_senders(2, &exit_tx).await.unwrap();

        let sdp = tokio::fs::read_to_string(&sdp_path).await.unwrap();
        assert!(
            sdp.contains(&format!("m=video {} RTP/AVP 96", dest.port())),
            "receiver SDP must advertise the video port and PT: {sdp}"
        );
        assert!(sdp.contains("a=rtpmap:96 H264/90000\r\n"), "SDP: {sdp}");

        let tracks = forward.publish_tracks().await;
        let track = tracks
            .iter()
            .find(|t| t.kind() == RtpCodecKind::Video)
            .expect("the virtual video track")
            .clone();
        let pump_stop = CancellationToken::new();
        tokio::spawn(pump_idrs(track, pump_stop.clone()));

        let datagrams = recv_idr_stream(&listener, &IDR, 96).await;
        pump_stop.cancel();
        // Same shape as the direct send-task test: parameter sets inlined
        // (STAP-A) ahead of the IDR single-NAL packet.
        assert_eq!(
            datagrams.len(),
            2,
            "expected a parameter-set datagram and an IDR datagram: {datagrams:02x?}"
        );
        let find = |haystack: &[u8], needle: &[u8]| {
            haystack.windows(needle.len()).position(|w| w == needle)
        };
        let (sps_at, pps_at) = (find(&datagrams[0], &SPS), find(&datagrams[0], &PPS));
        assert!(
            sps_at.is_some() && pps_at.is_some(),
            "parameter sets must be inlined ahead of the IDR: {datagrams:02x?}"
        );
        assert!(sps_at < pps_at, "SPS must precede PPS: {datagrams:02x?}");
        assert_eq!(datagrams[1], IDR.to_vec(), "datagrams: {datagrams:02x?}");

        // Tearing down the epoch stops the send task, which signals its
        // exit for the supervisor to reconcile.
        epoch.cancel();
        tokio::time::timeout(Duration::from_secs(5), exit_rx.recv())
            .await
            .expect("send task must signal its exit");
        cancel.cancel();
    }

    /// With no publisher the supervisor idles on the event bus; a shutdown
    /// cancels it and `run` must return promptly without sending anything.
    #[tokio::test]
    async fn run_exits_promptly_without_publisher_on_cancel() {
        let cancel = CancellationToken::new();
        let manager =
            Arc::new(Manager::new(crate::config::Config::default(), cancel.clone()).await);
        manager.stream_create("cam".to_string()).await.unwrap();

        let ctx = RtpTargetContext::new(
            manager.clone(),
            "cam".to_string(),
            rtp_target("rtp://127.0.0.1:5004"),
        )
        .unwrap();
        let handle = tokio::spawn(ctx.run());

        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("run() must exit when the manager is cancelled")
            .unwrap();
    }

    /// Regression for the stale-exit restart storm: a `PublishStopped`/
    /// `PublishStarted` cycle (source restart, publisher displacement) must
    /// tear the epoch down and build exactly one new one. The torn-down
    /// epoch's tasks report their exits too; without the generation tag
    /// that belated message would cancel the *new* epoch, whose tasks
    /// report their exits in turn — a self-sustaining restart storm that
    /// rewrites the SDP, rebinds the socket and PLIs the publisher
    /// thousands of times a second.
    #[cfg(all(
        feature = "source",
        any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "source-whep",
            feature = "rtsp",
            feature = "native-source"
        )
    ))]
    #[tokio::test]
    async fn publish_cycle_restarts_senders_exactly_once() {
        use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;

        use crate::event::SessionStopReason;

        const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x21];

        let cancel = CancellationToken::new();
        let manager =
            Arc::new(Manager::new(crate::config::Config::default(), cancel.clone()).await);
        manager.stream_create("cam".to_string()).await.unwrap();
        let forward = manager.get_forward("cam").await.unwrap();
        forward
            .add_virtual_track(RtpCodecKind::Video, h264_codec_params())
            .await
            .unwrap();

        let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let dest = listener.local_addr().unwrap();

        let ctx = RtpTargetContext::new(
            manager.clone(),
            "cam".to_string(),
            rtp_target(&format!("rtp://{dest}")),
        )
        .unwrap();
        let start_calls = ctx.start_calls.clone();
        let supervisor = tokio::spawn(ctx.run());

        // The first epoch comes up and media flows.
        let track = forward
            .publish_tracks()
            .await
            .into_iter()
            .find(|t| t.kind() == RtpCodecKind::Video)
            .expect("the virtual video track");
        let pump_stop = CancellationToken::new();
        tokio::spawn(pump_idrs(track, pump_stop.clone()));
        recv_idr_stream(&listener, &IDR, 96).await;
        assert_eq!(
            start_calls.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the initial publish starts one epoch"
        );

        // A source restart: the stop cancels the epoch (whose task then
        // also reports its exit — the stale message), the start rebuilds
        // against the lingering virtual track.
        manager.emit_source_publish_stopped("cam", SessionStopReason::IdleTimeout);
        manager.emit_source_publish_started("cam");
        recv_idr_stream(&listener, &IDR, 96).await;

        // Settle: a stale exit cancelling the fresh epoch would multiply
        // the count within this window, hot-looping rebuilds.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            start_calls.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "one teardown must yield exactly one rebuild, not a restart storm"
        );

        pump_stop.cancel();
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), supervisor)
            .await
            .expect("run() must exit when the manager is cancelled")
            .unwrap();
    }

    /// Event-bus lag forces a reconcile, and the reconcile paces the rebuild
    /// (live777#477): the torn-down epoch comes back after the base retry
    /// delay (5 s), not immediately — bounding socket rebind / SDP rewrite /
    /// PLI churn when the track set flaps.
    #[cfg(all(
        feature = "source",
        any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "source-whep",
            feature = "rtsp",
            feature = "native-source"
        )
    ))]
    #[tokio::test]
    async fn lagged_reconcile_paces_the_rebuild() {
        use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;

        const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x21];

        let cancel = CancellationToken::new();
        let manager =
            Arc::new(Manager::new(crate::config::Config::default(), cancel.clone()).await);
        manager.stream_create("cam".to_string()).await.unwrap();
        let forward = manager.get_forward("cam").await.unwrap();
        forward
            .add_virtual_track(RtpCodecKind::Video, h264_codec_params())
            .await
            .unwrap();

        let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let dest = listener.local_addr().unwrap();

        let ctx = RtpTargetContext::new(
            manager.clone(),
            "cam".to_string(),
            rtp_target(&format!("rtp://{dest}")),
        )
        .unwrap();
        let start_calls = ctx.start_calls.clone();
        let supervisor = tokio::spawn(ctx.run());

        let track = forward
            .publish_tracks()
            .await
            .into_iter()
            .find(|t| t.kind() == RtpCodecKind::Video)
            .expect("the virtual video track");
        let pump_stop = CancellationToken::new();
        tokio::spawn(pump_idrs(track, pump_stop.clone()));
        recv_idr_stream(&listener, &IDR, 96).await;
        assert_eq!(
            start_calls.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the initial publish starts one epoch"
        );

        // Lag the supervisor's event bus deterministically: the channel
        // holds 64 events and this burst never yields, so the
        // (single-threaded) runtime cannot poll it in between. Events name
        // an unrelated stream: the reconcile must find our publisher intact.
        for _ in 0..200 {
            manager.emit_source_publish_started("other");
        }

        // The reconcile tears the epoch down, then paces the rebuild: no new
        // epoch within the base retry delay...
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(
            start_calls.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "a lagged reconcile must pace the rebuild, not hot-loop"
        );

        // ...and the epoch comes back after it (5 s + slack).
        tokio::time::timeout(Duration::from_secs(10), async {
            while start_calls.load(std::sync::atomic::Ordering::Relaxed) == 1 {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the paced rebuild must re-establish the send epoch");
        recv_idr_stream(&listener, &IDR, 96).await;
        assert_eq!(
            start_calls.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "exactly one paced rebuild"
        );

        pump_stop.cancel();
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), supervisor)
            .await
            .expect("run() must exit when the manager is cancelled")
            .unwrap();
    }

    /// Regression for live777#481: a target on an on-demand stream is
    /// standing demand, so the on-demand idle timer must never stop the
    /// sources underneath the senders. Without the standing-demand
    /// registration the supervisor's kick re-arms the close-after safety
    /// net, which stops the source, and the resulting PublishStopped
    /// re-kick flaps the stream forever (bridge teardown/rebuild every
    /// close_after).
    #[cfg(feature = "source-sdp")]
    #[tokio::test]
    async fn on_demand_stream_with_target_does_not_flap() {
        // Minimal H264 SDP file source, port 0: OS-assigned receive ports.
        let sdp_path = std::env::temp_dir().join(format!(
            "live777-target-on-demand-{}.sdp",
            std::process::id()
        ));
        let sdp = "v=0\r\n\
                   o=- 0 0 IN IP4 127.0.0.1\r\n\
                   s=test\r\n\
                   c=IN IP4 127.0.0.1\r\n\
                   t=0 0\r\n\
                   m=video 0 RTP/AVP 96\r\n\
                   a=rtpmap:96 H264/90000\r\n";
        tokio::fs::write(&sdp_path, sdp).await.unwrap();

        let mut streams = std::collections::HashMap::new();
        streams.insert(
            "od".to_string(),
            crate::config::StreamEntry {
                sources: vec![crate::config::SourceConfig {
                    url: Some(format!("file://{}", sdp_path.to_string_lossy())),
                    multicast_interface: None,
                    #[cfg(feature = "native-source")]
                    capture: None,
                    #[cfg(feature = "native-source")]
                    encoder: None,
                    #[cfg(feature = "native-source")]
                    output: Default::default(),
                    #[cfg(feature = "native-source")]
                    tiers: vec![],
                }],
                on_demand: true,
                on_demand_close_after_ms: 200,
                on_demand_start_timeout_ms: 3000,
                ..Default::default()
            },
        );
        let config = crate::config::Config {
            stream: crate::config::StreamConfig { streams },
            ..Default::default()
        };
        let cancel = CancellationToken::new();
        let manager = Arc::new(Manager::new(config, cancel.clone()).await);
        manager.provision_streams().await;

        let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let dest = listener.local_addr().unwrap();
        let ctx = RtpTargetContext::new(
            manager.clone(),
            "od".to_string(),
            rtp_target(&format!("rtp://{dest}")),
        )
        .unwrap();
        let start_calls = ctx.start_calls.clone();
        let supervisor = tokio::spawn(ctx.run());

        // The standing demand kicks the on-demand sources and the senders
        // attach to the source's virtual tracks.
        tokio::time::timeout(Duration::from_secs(5), async {
            while !manager.source_manager.has_bridge("od").await {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the standing demand must start the on-demand sources");

        // The supervisor is registered as a virtual subscriber, listed in
        // the API like the source's `virtual-source` publisher.
        let virtual_id = crate::target::virtual_target_session_id(&format!("rtp://{dest}"));
        let infos = manager.info(vec!["od".to_string()]).await;
        assert!(
            infos[0]
                .subscribe
                .sessions
                .iter()
                .any(|s| s.id == virtual_id),
            "target must be listed as a virtual subscriber: {:?}",
            infos[0].subscribe.sessions
        );

        // Well past close_after (200ms): the idle timer must not have
        // stopped the sources, and exactly one send epoch must exist — the
        // flap would show up as repeated teardown/restart cycles.
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(
            manager.source_manager.has_source("od").await,
            "on-demand source stopped while the target was standing demand"
        );
        assert_eq!(
            start_calls.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the send epoch restarted: the stream is flapping"
        );

        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), supervisor)
            .await
            .expect("run() must exit when the manager is cancelled")
            .unwrap();
        let _ = std::fs::remove_file(&sdp_path);
    }
}
