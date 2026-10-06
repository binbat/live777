//! Static RTSP output targets (RTSP client push).
//!
//! A `[[stream.<name>.targets]]` config entry with an
//! `rtsp://[user:pass@]host[:port]/path` URL pushes the stream's media to an
//! RTSP server (mediamtx, another live777's RTSP server, gst-rtsp-server)
//! with ANNOUNCE/SETUP/RECORD — the counterpart of the RTSP source
//! (`source-rtsp`), which pulls from an RTSP server as a client.
//!
//! The push is media-driven, mirroring the WHIP/RTP target supervisors: the
//! RTSP session is established when the stream gains a publisher
//! (`PublishStarted`, real WHIP or a source's virtual one) and torn down when
//! the publisher goes away (`PublishStopped`). Negotiating per media epoch
//! keeps the announced codecs matched to the current publisher. Failures
//! retry with the same exponential backoff, and a target on an `on_demand`
//! stream acts as standing demand for its sources.
//!
//! Transport follows the whepfrom convention: UDP by default,
//! `?transport=tcp` in the URL selects TCP interleaved. UDP senders bind the
//! local port announced via SETUP `client_port`, because strict servers
//! (mediamtx) drop RTP from any other source port. Teardown sends TEARDOWN
//! (UDP, via the client's cancel token) or closes the connection (TCP), so
//! the server releases the published path immediately instead of after a
//! session timeout.
//!
//! Media handling per track mirrors the RTP target: only the first video and
//! the first audio track are announced; video is re-packetized through
//! [`RePayloadCodec`] (SPS/PPS inlined ahead of every IDR, oversized frames
//! become clean FU-A fragments), audio is forwarded untouched. Keyframe
//! requests coming back from the server (PLI/FIR relayed by e.g. mediamtx or
//! another live777) are forwarded to the publisher.

use std::sync::Arc;
use std::time::Duration;

use livetwo::payload::{Forward, RePayload, RePayloadCodec};
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::shared::marshal::{Marshal, MarshalSize};
use rtc_rtcp::payload_feedbacks::full_intra_request::FullIntraRequest;
use rtc_rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::TargetConfig;
use crate::event::{Event, StreamDeleteReason};
use crate::forward::message::Codec;
use crate::forward::track::PublishTrackRemote;
use crate::forward::{PeerForward, rtcp::RtcpMessage};
use crate::reconnect::reconnect_delay;
use crate::stream::manager::Manager;

/// Timeout for the whole ANNOUNCE/SETUP/RECORD handshake: without it a dead
/// server would park the supervisor (and with it event handling) on a
/// connect that the kernel only gives up on after minutes.
const ESTABLISH_TIMEOUT: Duration = Duration::from_secs(10);

/// A parsed `rtsp://` target: the URL handed to the RTSP client (credentials
/// kept for the auth exchange, query stripped), the server host, and the
/// chosen transport.
struct RtspTargetUrl {
    url: String,
    host: String,
    use_tcp: bool,
}

/// Parse an `rtsp://[user:pass@]host[:port]/path[?transport=tcp|udp]` target
/// URL. `rtsps://` is rejected explicitly: the RTSP client has no TLS
/// support, and silently opening a plain connection would be worse than a
/// clear error.
fn parse_rtsp_url(raw: &str) -> anyhow::Result<RtspTargetUrl> {
    let raw = raw.trim();
    let scheme = raw.split(':').next().unwrap_or("").to_ascii_lowercase();
    if scheme == "rtsps" {
        anyhow::bail!(
            "rtsps:// targets are not supported (the RTSP client has no TLS support); use rtsp://"
        );
    }
    if scheme != "rtsp" {
        anyhow::bail!("invalid rtsp:// target url '{raw}': scheme must be rtsp");
    }
    let mut url = url::Url::parse(raw)?;
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("invalid rtsp:// target url '{raw}': missing host"))?;
    let mut use_tcp = false;
    for (key, value) in url.query_pairs() {
        if key != "transport" {
            anyhow::bail!(
                "invalid rtsp:// target url '{raw}': unsupported query parameter \
                 '{key}' (only 'transport' is known)"
            );
        }
        use_tcp = match value.to_ascii_lowercase().as_str() {
            "tcp" => true,
            "udp" => false,
            other => anyhow::bail!(
                "invalid rtsp:// target url '{raw}': transport must be 'tcp' or 'udp', got '{other}'"
            ),
        };
    }
    url.set_query(None);
    Ok(RtspTargetUrl {
        url: url.to_string(),
        host,
        use_tcp,
    })
}

/// The URL as it may appear in log lines: credentials and query stripped.
fn redact_url(raw: &str) -> String {
    match url::Url::parse(raw.trim()) {
        Ok(mut url) => {
            let _ = url.set_password(None);
            let _ = url.set_username("");
            url.set_query(None);
            url.to_string()
        }
        Err(_) => "<invalid>".to_string(),
    }
}

/// Validate a configured `rtsp://` target: URL shape, transport parameter,
/// and that none of the rtp-only options leaked in. Called from
/// `Config::validate` (via `TargetConfig::validate`) so misconfiguration
/// fails at startup instead of surfacing once in a supervisor log line.
pub(crate) fn validate_rtsp_target(target: &TargetConfig) -> anyhow::Result<()> {
    parse_rtsp_url(&target.url)?;
    validate_rtsp_options(target)
}

/// Shared option validation for [`validate_rtsp_target`] and
/// [`RtspTargetContext::new`] (which covers programmatic targets that never
/// went through `Config::validate`).
fn validate_rtsp_options(target: &TargetConfig) -> anyhow::Result<()> {
    if target.multicast_interface.is_some()
        || target.ttl.is_some()
        || target.payload_type.is_some()
        || target.sdp_file.is_some()
    {
        anyhow::bail!(
            "multicast_interface, ttl, payload_type and sdp_file \
             are only valid with an rtp:// target"
        );
    }
    Ok(())
}

/// Wait until the stream's forward has every publish track the current
/// publisher negotiated. `PublishStarted` fires when the session is
/// negotiated; the tracks arrive afterwards, one `on_track` each, so
/// snapshotting the first non-empty set would race an AV publisher into an
/// audio-only announcement when its audio track lands first. The expected
/// counts come from the publish session's negotiated media info. Virtual
/// publishers (source bridges, RTSP pushes) carry no session media info —
/// their tracks are added before the supervisor can observe them — so a
/// non-empty snapshot is accepted for them.
async fn wait_for_tracks(forward: &PeerForward) -> anyhow::Result<Vec<PublishTrackRemote>> {
    let mut rx = forward.subscribe_tracks_change();

    loop {
        let (expected_video, expected_audio) = forward
            .internal
            .negotiated_publish_track_counts()
            .await
            .unwrap_or((0, 0));

        let tracks = forward.publish_tracks().await;
        let video = tracks
            .iter()
            .filter(|t| t.kind() == RtpCodecKind::Video)
            .count();
        let audio = tracks
            .iter()
            .filter(|t| t.kind() == RtpCodecKind::Audio)
            .count();
        if !tracks.is_empty() && video >= expected_video && audio >= expected_audio {
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

/// The RTSP target supervisor; constructed and spawned by
/// `crate::target::start_target` (static config and runtime API alike).
pub(crate) struct RtspTargetContext {
    manager: Arc<Manager>,
    stream: String,
    /// URL handed to the RTSP client: query stripped, credentials kept (they
    /// drive the Digest/Basic auth exchange), so it must stay out of logs.
    url: String,
    /// Credential-free rendering of the target for log lines.
    display: String,
    host: String,
    use_tcp: bool,
    cancel: CancellationToken,
}

impl RtspTargetContext {
    pub(crate) fn new(
        manager: Arc<Manager>,
        stream: String,
        target: TargetConfig,
        cancel: CancellationToken,
    ) -> anyhow::Result<Self> {
        let parsed = parse_rtsp_url(&target.url)
            .map_err(|e| anyhow::anyhow!("[{}] invalid RTSP target: {}", stream, e))?;
        validate_rtsp_options(&target)
            .map_err(|e| anyhow::anyhow!("[{}] invalid RTSP target: {}", stream, e))?;
        Ok(Self {
            manager,
            stream,
            url: parsed.url,
            display: redact_url(&target.url),
            host: parsed.host,
            use_tcp: parsed.use_tcp,
            cancel,
        })
    }

    pub(crate) async fn run(self) {
        // Subscribe before the initial snapshot/kick so media transitions
        // happening in between are still observed.
        let mut events = self.manager.subscribe_event();
        // Session epoch: while `Some`, the RTSP session is up and the
        // per-track send tasks are running, cancelled on teardown. A task
        // exiting on its own (track set changed, connection lost) also
        // signals via `exit_rx`. The generation tag distinguishes a live
        // epoch's spontaneous exit from a belated exit of an already
        // torn-down epoch — without it a stale message would cancel the
        // *next* epoch, self-sustaining a restart storm.
        let mut session: Option<(u64, CancellationToken)> = None;
        let mut generation: u64 = 0;
        let (exit_tx, mut exit_rx) = mpsc::unbounded_channel::<u64>();
        // Consecutive failed kick/session attempts, reset once a session is
        // up. The attempt spacing mirrors the RTSP/WHEP source reconnect
        // policy.
        let mut failures: u32 = 0;
        let mut desired = self.manager.has_publisher(&self.stream).await;
        // A configured target on an on-demand stream is standing demand:
        // whenever the stream has neither a publisher nor an active session,
        // kick its sources. Retried with the same backoff as session
        // failures, so an unreachable server caps at roughly one source
        // restart per minute.
        #[cfg(feature = "source")]
        let standing_demand = self.manager.is_on_demand_stream(&self.stream);
        // Register as a virtual subscriber for the supervisor's whole
        // lifetime: the session taps the forward track broadcast directly
        // (no subscribe session), so without this the on-demand idle check
        // would stop the sources every close_after underneath it
        // (live777#481). The registration also lists the target in the
        // stream's `subscribe.sessions`.
        let virtual_id = crate::target::virtual_target_session_id(&self.display);
        self.manager
            .add_virtual_subscriber(&self.stream, virtual_id.clone())
            .await;

        info!(
            "[target] [{}] rtsp target towards {} ({})",
            self.stream,
            self.display,
            if self.use_tcp { "tcp" } else { "udp" }
        );

        loop {
            #[cfg(feature = "source")]
            if standing_demand && !desired && session.is_none() {
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

            if desired && session.is_none() {
                generation = generation.wrapping_add(1);
                match self.start_session(generation, &exit_tx).await {
                    Ok(epoch) => {
                        session = Some((generation, epoch));
                        failures = 0;
                    }
                    Err(e) => {
                        failures = failures.saturating_add(1);
                        let delay = reconnect_delay(failures);
                        warn!(
                            "[target] [{}] rtsp push to {} failed: {:?}; retrying in {:?}",
                            self.stream, self.display, e, delay
                        );
                        if self.wait(delay).await {
                            break;
                        }
                        // The backoff wait is event-blind: the media may have
                        // gone away mid-sleep, and announcing now would
                        // negotiate the session with the wrong codecs.
                        desired = self.manager.has_publisher(&self.stream).await;
                        continue;
                    }
                }
            } else if !desired && session.is_some() {
                debug!(
                    "[target] [{}] media gone; tearing down rtsp session towards {}",
                    self.stream, self.display
                );
                if let Some((_, epoch)) = session.take() {
                    epoch.cancel();
                }
                continue;
            }

            tokio::select! {
                _ = self.cancel.cancelled() => break,
                exited = exit_rx.recv() => {
                    // A session task ended: the publish tracks changed (codec
                    // switch, displacement) or the connection dropped. Tear
                    // down and reconcile; the loop re-establishes when media
                    // is still there. Exits of an epoch the supervisor
                    // already tore down are stale and ignored.
                    let current = session.as_ref().is_some_and(|(tag, _)| Some(*tag) == exited);
                    if current
                        && let Some((_, epoch)) = session.take()
                    {
                        epoch.cancel();
                        desired = self.manager.has_publisher(&self.stream).await;
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
                            if let Some((_, epoch)) = session.take() {
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
                                "[target] [{}] stream deleted, stopping rtsp push to {}",
                                self.stream, self.display
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
                        if let Some((_, epoch)) = session.take() {
                            epoch.cancel();
                        }
                        desired = self.manager.has_publisher(&self.stream).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                },
            }
        }

        if let Some((_, epoch)) = session.take() {
            epoch.cancel();
        }
        self.manager
            .remove_virtual_subscriber(&self.stream, &virtual_id)
            .await;
        info!(
            "[target] [{}] stopped rtsp push to {}",
            self.stream, self.display
        );
    }

    /// Establish the RTSP session (ANNOUNCE/SETUP/RECORD) and spawn one send
    /// task per media track (first video, first audio) plus the RTCP return
    /// path. Returns the epoch token cancelling everything on teardown.
    /// `generation` tags the tasks' exit notifications so the supervisor can
    /// tell a live epoch's spontaneous exit from a stale one.
    async fn start_session(
        &self,
        generation: u64,
        exit_tx: &mpsc::UnboundedSender<u64>,
    ) -> anyhow::Result<CancellationToken> {
        let Some(forward) = self.manager.get_forward(&self.stream).await else {
            anyhow::bail!("stream forward not available yet");
        };

        let tracks = wait_for_tracks(&forward).await?;

        // Select the first RTSP-compatible video and audio track; every
        // other track is ignored. The codec whitelist mirrors the SDP
        // builder's: anything else would be dropped from the announcement
        // anyway, and the server would never see a SETUP for it.
        let mut selected: Vec<(PublishTrackRemote, RtpCodecKind, Codec, u8)> = Vec::new();
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
            if !matches!(
                codec.codec.as_str(),
                "h264"
                    | "h265"
                    | "hevc"
                    | "vp8"
                    | "vp9"
                    | "av1"
                    | "opus"
                    | "g722"
                    | "pcma"
                    | "pcmu"
            ) {
                continue;
            }
            let payload_type = codec.sdp_payload_type();
            selected.push((track.clone(), kind, codec, payload_type));
        }

        if selected.is_empty() {
            anyhow::bail!("no RTSP-compatible video or audio publish tracks yet");
        }

        let sdp = build_announce_sdp(
            &self.stream,
            &selected
                .iter()
                .map(|(_, kind, codec, pt)| (*kind, codec.clone(), *pt))
                .collect::<Vec<_>>(),
        );

        let epoch = CancellationToken::new();
        let established = tokio::time::timeout(
            ESTABLISH_TIMEOUT,
            rtsp::setup_rtsp_session(
                &self.url,
                Some(sdp),
                &self.host,
                rtsp::RtspMode::Push,
                self.use_tcp,
                epoch.clone(),
            ),
        )
        .await;
        let (media_info, channels) = match established {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                epoch.cancel();
                return Err(e);
            }
            Err(_) => {
                epoch.cancel();
                anyhow::bail!("rtsp session setup timed out after {:?}", ESTABLISH_TIMEOUT);
            }
        };

        // Resolve every track's egress and RTCP path before spawning
        // anything: a UDP bind failure (e.g. the SETUP-announced client port
        // was grabbed by someone else) must fail the epoch, not leave a
        // running session with zero media.
        let mut sends: Vec<TrackSend> = Vec::new();
        // UDP mode only: one RTCP listener socket per track.
        let mut rtcp_sockets: Vec<UdpSocket> = Vec::new();
        match &channels {
            Some((tx, _)) => {
                for (track, kind, codec, payload_type) in &selected {
                    let Some(rtsp::TransportInfo::Tcp { rtp_channel, .. }) =
                        transport_for(&media_info, *kind)
                    else {
                        epoch.cancel();
                        anyhow::bail!("server did not set up a {:?} media over TCP", kind);
                    };
                    sends.push(TrackSend {
                        track: track.clone(),
                        sink: TrackSink::Tcp(tx.clone(), *rtp_channel),
                        payload_type: *payload_type,
                        is_video: codec.kind == "video",
                        generation,
                    });
                }
            }
            None => {
                for (track, kind, codec, payload_type) in &selected {
                    let Some(rtsp::TransportInfo::Udp {
                        rtp_send_port: Some(send_port),
                        rtp_recv_port,
                        rtcp_recv_port,
                        server_addr,
                        ..
                    }) = transport_for(&media_info, *kind)
                    else {
                        epoch.cancel();
                        anyhow::bail!("server did not set up a {:?} media over UDP", kind);
                    };
                    let target_host = server_addr
                        .map(|addr| addr.ip().to_string())
                        .unwrap_or_else(|| self.host.clone());
                    let listen_host = wildcard_for(&target_host);
                    // Bind the SETUP-announced local port: strict servers
                    // (mediamtx) drop RTP whose source port differs.
                    let socket =
                        UdpSocket::bind(bind_addr(listen_host, rtp_recv_port.unwrap_or(0)))
                            .await
                            .map_err(|e| {
                                epoch.cancel();
                                anyhow::anyhow!(
                                    "failed to bind {:?} RTP socket on {}: {}",
                                    kind,
                                    bind_addr(listen_host, rtp_recv_port.unwrap_or(0)),
                                    e
                                )
                            })?;
                    if let Some(rtcp_port) = rtcp_recv_port {
                        match UdpSocket::bind(bind_addr(listen_host, *rtcp_port)).await {
                            Ok(socket) => rtcp_sockets.push(socket),
                            Err(e) => {
                                debug!(
                                    "[target] [{}] {:?} RTCP listener bind on port {} failed: {}",
                                    self.stream, kind, rtcp_port, e
                                );
                            }
                        }
                    }
                    sends.push(TrackSend {
                        track: track.clone(),
                        sink: TrackSink::Udp(
                            Arc::new(socket),
                            format!("{target_host}:{send_port}"),
                        ),
                        payload_type: *payload_type,
                        is_video: codec.kind == "video",
                        generation,
                    });
                }
            }
        }

        for send in sends {
            tokio::spawn(track_send_task(send, epoch.child_token(), exit_tx.clone()));
        }

        // RTCP return path: relay the server's keyframe requests (a puller's
        // PLI forwarded by mediamtx or another live777) to the publisher.
        match channels {
            Some((_, rx)) => {
                tokio::spawn(tcp_rtcp_drain_task(
                    rx,
                    forward.clone(),
                    self.stream.clone(),
                    epoch.child_token(),
                    generation,
                    exit_tx.clone(),
                ));
            }
            None => {
                for socket in rtcp_sockets {
                    tokio::spawn(udp_rtcp_task(
                        socket,
                        forward.clone(),
                        self.stream.clone(),
                        epoch.child_token(),
                    ));
                }
            }
        }

        // Nudge the publisher towards an IDR so the server (and its own
        // readers) get a decodable frame — and the SPS/PPS injector a frame
        // to attach the parameter sets to — without waiting for the sender's
        // own keyframe cadence.
        for (track, kind, ..) in &selected {
            if *kind == RtpCodecKind::Video {
                let ssrc = track.source_ssrc().await;
                if ssrc != 0
                    && let Err(e) = forward
                        .send_rtcp_to_publish(RtcpMessage::PictureLossIndication, ssrc)
                        .await
                {
                    debug!("[target] [{}] PLI nudge failed: {:?}", self.stream, e);
                }
            }
        }

        info!(
            "[target] [{}] pushing to {} ({} track(s), {})",
            self.stream,
            self.display,
            selected.len(),
            if self.use_tcp { "tcp" } else { "udp" }
        );

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

fn transport_for(media_info: &rtsp::MediaInfo, kind: RtpCodecKind) -> Option<&rtsp::TransportInfo> {
    match kind {
        RtpCodecKind::Video => media_info.video_transport.as_ref(),
        RtpCodecKind::Audio => media_info.audio_transport.as_ref(),
        _ => None,
    }
}

/// The wildcard address matching a destination's family, for binding the
/// UDP sender/RTCP sockets. Hostnames fall back to IPv4.
fn wildcard_for(dest: &str) -> &'static str {
    if dest
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_ipv6())
    {
        "::"
    } else {
        "0.0.0.0"
    }
}

fn bind_addr(host: &str, port: u16) -> String {
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Build the ANNOUNCE SDP for the selected tracks: one `m=` line per media
/// with rtpmap/fmtp and an `a=control:` attribute — the same shape the RTSP
/// server's DESCRIBE produces (`rtsp_server`'s `build_sdp_from_tracks`), and
/// the attribute the client session derives the per-media SETUP URLs from.
fn build_announce_sdp(stream: &str, tracks: &[(RtpCodecKind, Codec, u8)]) -> String {
    // A CR/LF in the stream name would break the SDP's line structure.
    let session: String = stream
        .chars()
        .filter(|c| !matches!(c, '\r' | '\n'))
        .collect();
    let mut lines = vec![
        "v=0".to_string(),
        "o=- 0 0 IN IP4 127.0.0.1".to_string(),
        format!("s=live777-{session}"),
        "t=0 0".to_string(),
    ];

    for (kind, codec, pt) in tracks {
        let (media, clock_rate, channels) = match codec.codec.as_str() {
            "h264" | "h265" | "hevc" | "vp8" | "vp9" | "av1" => ("video", codec.clock_rate, None),
            "opus" | "g722" | "pcma" | "pcmu" => (
                "audio",
                codec.clock_rate,
                // 0 means the negotiated rtpmap carried no channel count
                // (conventional for G.711/G.722): omit the parameter rather
                // than emit an invalid `/0`.
                Some(codec.channels as u8).filter(|ch| *ch > 0),
            ),
            _ => continue,
        };

        lines.push(format!("m={media} 0 RTP/AVP {pt}"));
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
        let control = match kind {
            RtpCodecKind::Video => "video",
            RtpCodecKind::Audio => "audio",
            _ => continue,
        };
        lines.push(format!("a=control:{control}"));
    }

    lines.join("\r\n") + "\r\n"
}

/// One send task's egress.
enum TrackSink {
    /// TCP interleaved: the shared session sender plus this track's
    /// negotiated RTP channel.
    Tcp(mpsc::Sender<(u8, Vec<u8>)>, u8),
    /// UDP: a socket bound to the SETUP-announced local port, and the
    /// server's RTP endpoint as `host:port`.
    Udp(Arc<UdpSocket>, String),
}

/// One send task's wiring: the tapped track, its egress and payload type,
/// and the epoch generation tagged onto the exit notification.
struct TrackSend {
    track: PublishTrackRemote,
    sink: TrackSink,
    payload_type: u8,
    is_video: bool,
    generation: u64,
}

async fn track_send_task(
    send: TrackSend,
    cancel: CancellationToken,
    exit_tx: mpsc::UnboundedSender<u64>,
) {
    let TrackSend {
        track,
        sink,
        payload_type,
        is_video,
        generation,
    } = send;
    let codec = track.codec();
    let mime = format!("{}/{}", codec.kind, codec.codec);
    // Video is re-assembled and re-packetized: SPS/PPS get inlined ahead of
    // every IDR (idempotent for streams that already carry them) and
    // oversized frames become clean FU-A fragments, so the server's readers
    // joining mid-GOP decode from the next IDR. Audio forwards untouched.
    let mut rp: Box<dyn RePayload + Send> = if is_video {
        Box::new(RePayloadCodec::with_sprop_params(mime, &codec.fmtp))
    } else {
        Box::<Forward>::default()
    };

    let mut rx = track.subscribe();
    // Reused across packets (UDP): high-bitrate video means a thousand
    // marshals a second.
    let mut buf = Vec::with_capacity(1500);
    'outer: loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            packet = rx.recv() => {
                match packet {
                    Ok(packet) => {
                        for mut out in rp.payload(&packet) {
                            out.header.payload_type = payload_type;
                            match &sink {
                                TrackSink::Tcp(tx, channel) => {
                                    let Ok(bytes) = out.marshal() else {
                                        continue;
                                    };
                                    // A closed sender means the connection
                                    // (and the session) is down: end the task
                                    // so the supervisor re-establishes.
                                    if tx.send((*channel, bytes.to_vec())).await.is_err() {
                                        break 'outer;
                                    }
                                }
                                TrackSink::Udp(socket, dest) => {
                                    buf.clear();
                                    buf.resize(out.marshal_size(), 0);
                                    if Marshal::marshal_to(&out, &mut buf).is_err() {
                                        continue;
                                    }
                                    if let Err(e) = socket.send_to(&buf, dest).await {
                                        debug!("[target] rtsp udp send to {dest} failed: {e}");
                                    }
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // The repayloader re-baselines its sequence numbers
                        // on the gap; forward-passthrough tracks do not care.
                        warn!("[target] lagged {n} packets towards the rtsp server");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    // Whatever the exit reason, tell the supervisor which epoch ended so it
    // can reconcile: the track set may have changed under it, or the
    // connection dropped. The generation tag lets it ignore this message
    // when it already tore the epoch down.
    let _ = exit_tx.send(generation);
}

/// Drain the TCP interleaved receive channel: odd channels carry RTCP from
/// the server. The channel closing means the connection (and the session)
/// is gone — report the epoch as exited so the supervisor re-establishes.
async fn tcp_rtcp_drain_task(
    mut rx: mpsc::Receiver<(u8, Vec<u8>)>,
    forward: PeerForward,
    stream: String,
    cancel: CancellationToken,
    generation: u64,
    exit_tx: mpsc::UnboundedSender<u64>,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            msg = rx.recv() => match msg {
                Some((channel, data)) => {
                    if channel % 2 == 1 {
                        forward_server_rtcp(&forward, &stream, &data).await;
                    }
                }
                None => break,
            },
        }
    }
    let _ = exit_tx.send(generation);
}

/// Listen on the UDP RTCP port announced to the server and relay keyframe
/// requests to the publisher. UDP has no connection state to watch, so the
/// task only ends on teardown or a socket error.
async fn udp_rtcp_task(
    socket: UdpSocket,
    forward: PeerForward,
    stream: String,
    cancel: CancellationToken,
) {
    let mut buf = vec![0u8; 1500];
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = socket.recv_from(&mut buf) => match result {
                Ok((n, _)) => forward_server_rtcp(&forward, &stream, &buf[..n]).await,
                Err(e) => {
                    debug!("[target] [{}] rtcp receive error: {}", stream, e);
                    break;
                }
            },
        }
    }
}

/// Parse RTCP arriving from the server and forward keyframe requests
/// (PLI/FIR) to the publisher — the reverse direction of the RTSP server's
/// pull-side forwarding (`rtsp_server`'s `forward_rtcp_to_publish`).
async fn forward_server_rtcp(forward: &PeerForward, stream: &str, data: &[u8]) {
    let mut reader = data;
    let packets = match rtc_rtcp::packet::unmarshal(&mut reader) {
        Ok(packets) => packets,
        Err(e) => {
            debug!("[target] [{}] failed to parse server RTCP: {}", stream, e);
            return;
        }
    };

    for packet in packets {
        let any = packet.as_any();
        let (msg, ssrc) = if let Some(pli) = any.downcast_ref::<PictureLossIndication>() {
            (RtcpMessage::PictureLossIndication, pli.media_ssrc)
        } else if let Some(fir) = any.downcast_ref::<FullIntraRequest>() {
            (RtcpMessage::_FullIntraRequest, fir.media_ssrc)
        } else {
            continue;
        };
        if let Err(e) = forward.send_rtcp_to_publish(msg, ssrc).await {
            debug!(
                "[target] [{}] forwarding server RTCP to publish failed: {:?}",
                stream, e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rtsp_target(url: &str) -> TargetConfig {
        TargetConfig {
            url: url.to_string(),
            multicast_interface: None,
            ttl: None,
            payload_type: None,
            sdp_file: None,
        }
    }

    #[test]
    fn parse_rtsp_url_defaults_to_udp() {
        let parsed = parse_rtsp_url("rtsp://example.com:8554/live/stream").unwrap();
        assert_eq!(parsed.url, "rtsp://example.com:8554/live/stream");
        assert_eq!(parsed.host, "example.com");
        assert!(!parsed.use_tcp);
    }

    #[test]
    fn parse_rtsp_url_keeps_credentials() {
        let parsed = parse_rtsp_url("rtsp://user:pass@example.com/live").unwrap();
        assert_eq!(parsed.url, "rtsp://user:pass@example.com/live");
        assert_eq!(parsed.host, "example.com");
    }

    #[test]
    fn parse_rtsp_url_transport_param() {
        let parsed = parse_rtsp_url("rtsp://example.com/live?transport=tcp").unwrap();
        assert!(parsed.use_tcp);
        assert_eq!(parsed.url, "rtsp://example.com/live");
        let parsed = parse_rtsp_url("rtsp://example.com/live?transport=UDP").unwrap();
        assert!(!parsed.use_tcp);
        assert!(parse_rtsp_url("rtsp://example.com/live?transport=sctp").is_err());
        assert!(parse_rtsp_url("rtsp://example.com/live?foo=bar").is_err());
    }

    #[test]
    fn parse_rtsp_url_rejects_bad_urls() {
        assert!(parse_rtsp_url("rtsps://example.com/live").is_err());
        assert!(parse_rtsp_url("http://example.com/live").is_err());
        assert!(parse_rtsp_url("rtsp:///live").is_err());
        assert!(parse_rtsp_url("not a url").is_err());
    }

    #[test]
    fn validate_rejects_rtp_only_options() {
        let mut target = rtsp_target("rtsp://example.com/live");
        assert!(validate_rtsp_target(&target).is_ok());
        target.ttl = Some(16);
        assert!(validate_rtsp_target(&target).is_err());
        target.ttl = None;
        target.payload_type = Some(96);
        assert!(validate_rtsp_target(&target).is_err());
        target.payload_type = None;
        target.sdp_file = Some("/tmp/x.sdp".to_string());
        assert!(validate_rtsp_target(&target).is_err());
        target.sdp_file = None;
        target.multicast_interface = Some("eth0".to_string());
        assert!(validate_rtsp_target(&target).is_err());
    }

    #[test]
    fn redact_url_strips_credentials_and_query() {
        assert_eq!(
            redact_url("rtsp://user:pass@example.com:8554/live?transport=tcp"),
            "rtsp://example.com:8554/live"
        );
    }

    fn video_codec() -> Codec {
        Codec {
            kind: "video".to_string(),
            codec: "h264".to_string(),
            fmtp: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                .to_string(),
            payload_type: 102,
            clock_rate: 90000,
            channels: 0,
        }
    }

    #[test]
    fn announce_sdp_shape() {
        let sdp = build_announce_sdp(
            "cam",
            &[
                (RtpCodecKind::Video, video_codec(), 102),
                (
                    RtpCodecKind::Audio,
                    Codec {
                        kind: "audio".to_string(),
                        codec: "opus".to_string(),
                        fmtp: "minptime=10;useinbandfec=1".to_string(),
                        payload_type: 111,
                        clock_rate: 48000,
                        channels: 2,
                    },
                    111,
                ),
            ],
        );
        assert!(sdp.starts_with("v=0\r\n"));
        assert!(sdp.contains("s=live777-cam\r\n"));
        assert!(sdp.contains("m=video 0 RTP/AVP 102\r\n"));
        assert!(sdp.contains("a=rtpmap:102 H264/90000\r\n"));
        assert!(sdp.contains(
            "a=fmtp:102 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f\r\n"
        ));
        assert!(sdp.contains("a=control:video\r\n"));
        assert!(sdp.contains("m=audio 0 RTP/AVP 111\r\n"));
        assert!(sdp.contains("a=rtpmap:111 OPUS/48000/2\r\n"));
        assert!(sdp.contains("a=control:audio\r\n"));
        // The client session must be able to re-parse its own announcement
        // to derive the codecs and per-media SETUP URLs.
        let info = rtsp::parse_media_info_from_sdp(sdp.as_bytes()).unwrap();
        assert!(info.video_codec.is_some());
        assert!(info.audio_codec.is_some());
    }

    #[test]
    fn announce_sdp_omits_zero_channels() {
        let sdp = build_announce_sdp(
            "cam",
            &[(
                RtpCodecKind::Audio,
                Codec {
                    kind: "audio".to_string(),
                    codec: "g722".to_string(),
                    fmtp: String::new(),
                    payload_type: 9,
                    clock_rate: 8000,
                    channels: 0,
                },
                9,
            )],
        );
        assert!(sdp.contains("a=rtpmap:9 G722/8000\r\n"));
        assert!(!sdp.contains("G722/8000/"));
    }
}
