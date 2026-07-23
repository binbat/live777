//! WHEP pull source.
//!
//! Connects out to an upstream WHEP endpoint (typically another live777
//! node) and ingests the media as this stream's input, on par with the RTSP
//! and SDP sources. Built on livetwo's WHEP peer machinery, so it takes part
//! in the whole source lifecycle (on-demand start/stop, reconnect, codec
//! readiness, RTCP feedback) like any other source.

use super::{
    ChannelMapping, InternalSourceConfig, MediaPacket, SourceNetConfig, StateChangeEvent,
    StreamSource, StreamSourceState,
};
use anyhow::Result;
use async_trait::async_trait;
use libwish::Client;
use livetwo::utils::graceful_shutdown;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use webrtc::peer_connection::PeerConnection;

use rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters;

/// Bounded capacity of the per-kind media channels between the WHEP track
/// handlers and the pump, same as livetwo's WHEP receiver.
const MEDIA_CHANNEL_CAPACITY: usize = 512;

/// Upper bound of one codec wait: the negotiated media kinds must deliver
/// their first RTP packet within this budget.
const CODEC_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP timeout for the WHEP POST: an upstream liveion may hold the request
/// until its on-demand source is ready (up to 35 s for WHEP-sourced
/// streams, `WHEP_ON_DEMAND_CODEC_WAIT`), so libwish's default 30 s would
/// cut off healthy chained pulls. 40 s = 35 s + margin; deeper chains need
/// their budgets aligned per deployment.
const WHEP_HTTP_TIMEOUT: Duration = Duration::from_secs(40);

/// After the first codec becomes known, accept a partially ready codec set
/// (e.g. a negotiated audio track that never sends) once this grace elapses.
const PARTIAL_READY_GRACE: Duration = Duration::from_secs(2);

const CODEC_WAIT_POLL: Duration = Duration::from_millis(100);

/// Codec parameters captured once the first connection reports its media.
/// Reused across reconnects so the source bridge's channel mapping and
/// virtual tracks stay stable.
#[derive(Clone, Default)]
struct CodecSnapshot {
    video: Option<RTCRtpCodecParameters>,
    audio: Option<RTCRtpCodecParameters>,
}

impl CodecSnapshot {
    /// RTP channel carrying audio on the source bridge: after video when both
    /// kinds are present, first otherwise. Uses the same `ChannelMapping` the
    /// bridge applies, so the two sides cannot drift apart.
    fn audio_channel(&self) -> u8 {
        ChannelMapping::new(self.video.is_some(), self.audio.is_some())
            .audio_rtp
            .unwrap_or(0)
    }

    /// Whether the snapshot lacks a media kind that `current` (the freshly
    /// connected session) delivers. A kind added upstream cannot be routed by
    /// the bridge's fixed channel mapping, so the attempt must fail instead
    /// of silently misrouting it.
    fn lacks_kind_of(&self, current: &CodecSnapshot) -> bool {
        (current.video.is_some() && self.video.is_none())
            || (current.audio.is_some() && self.audio.is_none())
    }

    /// Whether a codec that exists in both snapshots changed across the
    /// reconnect (mime type, clock rate, or channel count). The bridge's
    /// virtual tracks and repayloader were built from the snapshot, so a
    /// changed codec would be forwarded as undecodable garbage; the attempt
    /// must fail instead. fmtp is deliberately not compared: a same-codec
    /// parameter change (e.g. new SPS/PPS) still passes through, matching
    /// the RTSP source's behavior.
    fn codec_mismatch_of(&self, current: &CodecSnapshot) -> bool {
        fn differs(a: &Option<RTCRtpCodecParameters>, b: &Option<RTCRtpCodecParameters>) -> bool {
            match (a, b) {
                (Some(a), Some(b)) => {
                    !a.rtp_codec
                        .mime_type
                        .eq_ignore_ascii_case(&b.rtp_codec.mime_type)
                        || a.rtp_codec.clock_rate != b.rtp_codec.clock_rate
                        || a.rtp_codec.channels != b.rtp_codec.channels
                }
                _ => false,
            }
        }
        differs(&self.video, &current.video) || differs(&self.audio, &current.audio)
    }
}

struct WhepClientContext {
    stream_id: String,
    whep_url: String,
    token: Option<String>,
    config: InternalSourceConfig,
    /// Server-wide WebRTC network settings (`[[ice_servers]]`, ICE UDP
    /// addrs), used for the outgoing peer instead of any hardcoded default.
    net: SourceNetConfig,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state: Arc<std::sync::RwLock<StreamSourceState>>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    snapshot: Arc<RwLock<Option<CodecSnapshot>>>,
    peer_store: Arc<RwLock<Option<Arc<dyn PeerConnection>>>>,
}

enum AttemptEnd {
    Shutdown,
    Failed { reason: String, connected: bool },
}

fn next_reconnect_count(current: u32, connected: bool) -> u32 {
    if connected {
        1
    } else {
        current.saturating_add(1)
    }
}

enum WaitOutcome {
    Ready(CodecSnapshot),
    Shutdown,
    Failed,
}

pub struct WhepSource {
    config: InternalSourceConfig,
    whep_url: String,
    token: Option<String>,
    net: SourceNetConfig,
    state: Arc<std::sync::RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    task_handle: Option<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    snapshot: Arc<RwLock<Option<CodecSnapshot>>>,
    peer_store: Arc<RwLock<Option<Arc<dyn PeerConnection>>>>,
}

impl WhepSource {
    pub fn new(config: InternalSourceConfig, whep_url: &str, net: SourceNetConfig) -> Result<Self> {
        let (http_url, token) = parse_whep_url(whep_url)?;
        // The token reaches the Authorization header verbatim on every
        // attempt; reject invalid header values now, at construction,
        // instead of failing inside the source task.
        Client::get_auth_header_map(token.clone())?;
        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Ok(Self {
            config,
            whep_url: http_url,
            token,
            net,
            state: Arc::new(std::sync::RwLock::new(StreamSourceState::Initializing)),
            rtp_tx,
            state_tx,
            task_handle: None,
            shutdown_tx: None,
            snapshot: Arc::new(RwLock::new(None)),
            peer_store: Arc::new(RwLock::new(None)),
        })
    }

    async fn run_whep_client(ctx: WhepClientContext, mut shutdown_rx: oneshot::Receiver<()>) {
        let mut reconnect_count = 0u32;

        loop {
            Self::emit_state_change(
                &ctx.state,
                &ctx.state_tx,
                if reconnect_count > 0 {
                    StreamSourceState::Reconnecting
                } else {
                    StreamSourceState::Initializing
                },
                None,
            )
            .await;

            let connected = match Self::run_attempt(&ctx, &mut shutdown_rx).await {
                AttemptEnd::Shutdown => break,
                AttemptEnd::Failed { reason, connected } => {
                    warn!("[{}] WHEP session ended: {}", ctx.stream_id, reason);
                    Self::emit_state_change(
                        &ctx.state,
                        &ctx.state_tx,
                        StreamSourceState::Disconnected,
                        Some(reason),
                    )
                    .await;
                    connected
                }
            };

            if !ctx.config.reconnect_enabled() {
                info!("[{}] Reconnect disabled, exiting", ctx.stream_id);
                break;
            }

            reconnect_count = next_reconnect_count(reconnect_count, connected);

            if ctx.config.max_reconnect_attempts() > 0
                && reconnect_count > ctx.config.max_reconnect_attempts()
            {
                error!(
                    "[{}] Max reconnect attempts ({}) reached",
                    ctx.stream_id,
                    ctx.config.max_reconnect_attempts()
                );
                Self::emit_state_change(
                    &ctx.state,
                    &ctx.state_tx,
                    StreamSourceState::Error,
                    Some("Max reconnect attempts reached".to_string()),
                )
                .await;
                break;
            }

            info!(
                "[{}] Reconnecting in {}ms (attempt {})",
                ctx.stream_id,
                ctx.config.reconnect_delay_ms(reconnect_count),
                reconnect_count,
            );

            tokio::select! {
                _ = &mut shutdown_rx => {
                    info!(
                        "[{}] Shutdown signal received during reconnect wait",
                        ctx.stream_id
                    );
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(
                    ctx.config.reconnect_delay_ms(reconnect_count),
                )) => {}
            }
        }

        Self::emit_state_change(
            &ctx.state,
            &ctx.state_tx,
            StreamSourceState::Disconnected,
            None,
        )
        .await;
        info!("[{}] Task exited", ctx.stream_id);
    }

    /// One WHEP session: connect, wait for the codecs once, pump media until
    /// the peer dies or a shutdown is requested.
    async fn run_attempt(
        ctx: &WhepClientContext,
        shutdown_rx: &mut oneshot::Receiver<()>,
    ) -> AttemptEnd {
        info!("[{}] Connecting to {}", ctx.stream_id, ctx.whep_url);

        let ct = CancellationToken::new();
        // Validated at construction (WhepSource::new); an error here is
        // defensive and fails the attempt instead of panicking the task.
        let auth = match Client::get_auth_header_map(ctx.token.clone()) {
            Ok(auth) => auth,
            Err(e) => {
                error!("[{}] Invalid auth token: {}", ctx.stream_id, e);
                return AttemptEnd::Failed {
                    reason: format!("Invalid auth token: {}", e),
                    connected: false,
                };
            }
        };
        let mut client = Client::new(ctx.whep_url.clone(), auth).with_timeout(WHEP_HTTP_TIMEOUT);
        let (video_tx, mut video_rx) = mpsc::channel::<Vec<u8>>(MEDIA_CHANNEL_CAPACITY);
        let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(MEDIA_CHANNEL_CAPACITY);
        let codec_info = Arc::new(Mutex::new(rtsp::CodecInfo::new()));

        let (peer, answer, ..) = match livetwo::whep::setup_whep_peer(
            ct.clone(),
            &mut client,
            video_tx,
            audio_tx,
            codec_info.clone(),
            livetwo::whep::WhepPeerOptions {
                ice_servers: ctx.net.ice_servers.clone(),
                ice_udp_addrs: ctx.net.ice_udp_addrs.clone(),
                // Pure pull: no need to join the upstream's WHEP control group.
                control_channel: false,
            },
            None,
            None,
        )
        .await
        {
            Ok(connected) => connected,
            Err(e) => {
                error!("[{}] WHEP connection failed: {}", ctx.stream_id, e);
                return AttemptEnd::Failed {
                    reason: format!("Connection failed: {}", e),
                    connected: false,
                };
            }
        };
        *ctx.peer_store.write().await = Some(peer.clone());

        // The codec snapshot is taken once and reused across reconnects so
        // the bridge's channel mapping stays stable. A reconnect still waits
        // for the known kinds and fails the attempt when the upstream started
        // delivering a kind the snapshot does not have — the fixed mapping
        // could not route it.
        let snapshot = ctx.snapshot.read().await.clone();
        let snapshot = match snapshot {
            Some(snapshot) => {
                let expected = (snapshot.video.is_some(), snapshot.audio.is_some());
                match Self::wait_for_codecs(ctx, &ct, &codec_info, expected, shutdown_rx).await {
                    WaitOutcome::Ready(current) => {
                        if snapshot.lacks_kind_of(&current) {
                            error!(
                                "[{}] Upstream added a media kind across reconnect (was video={}, audio={})",
                                ctx.stream_id,
                                snapshot.video.is_some(),
                                snapshot.audio.is_some()
                            );
                            Self::cleanup(ctx, &mut client, peer).await;
                            return AttemptEnd::Failed {
                                reason: "Upstream media kinds changed".to_string(),
                                connected: false,
                            };
                        }
                        if snapshot.codec_mismatch_of(&current) {
                            error!(
                                "[{}] Upstream codec changed across reconnect (video {} -> {}, audio {} -> {})",
                                ctx.stream_id,
                                mime_of(&snapshot.video),
                                mime_of(&current.video),
                                mime_of(&snapshot.audio),
                                mime_of(&current.audio),
                            );
                            Self::cleanup(ctx, &mut client, peer).await;
                            return AttemptEnd::Failed {
                                reason: "Upstream codec changed".to_string(),
                                connected: false,
                            };
                        }
                        Some(snapshot)
                    }
                    WaitOutcome::Shutdown => {
                        Self::cleanup(ctx, &mut client, peer).await;
                        return AttemptEnd::Shutdown;
                    }
                    WaitOutcome::Failed => None,
                }
            }
            None => {
                let expected = expected_media_kinds(&answer.sdp);
                match Self::wait_for_codecs(ctx, &ct, &codec_info, expected, shutdown_rx).await {
                    WaitOutcome::Ready(snapshot) => {
                        info!(
                            "[{}] Codec ready: video={}, audio={}",
                            ctx.stream_id,
                            snapshot.video.is_some(),
                            snapshot.audio.is_some()
                        );
                        *ctx.snapshot.write().await = Some(snapshot.clone());
                        Some(snapshot)
                    }
                    WaitOutcome::Shutdown => {
                        Self::cleanup(ctx, &mut client, peer).await;
                        return AttemptEnd::Shutdown;
                    }
                    WaitOutcome::Failed => None,
                }
            }
        };

        let end = match snapshot {
            Some(snapshot) => {
                Self::emit_state_change(
                    &ctx.state,
                    &ctx.state_tx,
                    StreamSourceState::Connected,
                    None,
                )
                .await;
                Self::pump(
                    ctx,
                    snapshot,
                    &mut video_rx,
                    &mut audio_rx,
                    &ct,
                    shutdown_rx,
                )
                .await
            }
            None => AttemptEnd::Failed {
                reason: "Codec wait timed out".to_string(),
                connected: false,
            },
        };

        Self::cleanup(ctx, &mut client, peer).await;
        end
    }

    async fn cleanup(ctx: &WhepClientContext, client: &mut Client, peer: Arc<dyn PeerConnection>) {
        *ctx.peer_store.write().await = None;
        graceful_shutdown("WHEP source", client, peer).await;
    }

    /// Wait until every expected media kind delivered its first RTP packet
    /// (making its payload type known), accepting a partially ready set after
    /// a grace period.
    async fn wait_for_codecs(
        ctx: &WhepClientContext,
        ct: &CancellationToken,
        codec_info: &Arc<Mutex<rtsp::CodecInfo>>,
        expected: (bool, bool),
        shutdown_rx: &mut oneshot::Receiver<()>,
    ) -> WaitOutcome {
        let start = Instant::now();
        let mut first_ready_at: Option<Instant> = None;

        loop {
            let info = codec_info.lock().await.clone();
            let video = info.video_codec.is_some();
            let audio = info.audio_codec.is_some();
            if video || audio {
                first_ready_at.get_or_insert_with(Instant::now);
            }
            let partial_grace_elapsed =
                first_ready_at.is_some_and(|at| at.elapsed() >= PARTIAL_READY_GRACE);

            if codec_wait_satisfied(expected, video, audio, partial_grace_elapsed) {
                return WaitOutcome::Ready(CodecSnapshot {
                    video: info.video_codec,
                    audio: info.audio_codec,
                });
            }

            if start.elapsed() >= CODEC_WAIT_TIMEOUT {
                error!(
                    "[{}] Codec not ready within {:?} (expected video={}, audio={})",
                    ctx.stream_id, CODEC_WAIT_TIMEOUT, expected.0, expected.1
                );
                return WaitOutcome::Failed;
            }

            tokio::select! {
                _ = &mut *shutdown_rx => return WaitOutcome::Shutdown,
                _ = ct.cancelled() => {
                    return WaitOutcome::Failed;
                }
                _ = tokio::time::sleep(CODEC_WAIT_POLL) => {}
            }
        }
    }

    async fn pump(
        ctx: &WhepClientContext,
        snapshot: CodecSnapshot,
        video_rx: &mut mpsc::Receiver<Vec<u8>>,
        audio_rx: &mut mpsc::Receiver<Vec<u8>>,
        ct: &CancellationToken,
        shutdown_rx: &mut oneshot::Receiver<()>,
    ) -> AttemptEnd {
        let audio_channel = snapshot.audio_channel();
        // A kind missing from the snapshot (accepted via the partial-ready
        // grace) must be dropped here, not forwarded: the bridge's fixed
        // channel mapping would route it onto the *other* kind's virtual
        // track (e.g. late video would land on the audio channel of an
        // audio-only mapping).
        let has_video = snapshot.video.is_some();
        let has_audio = snapshot.audio.is_some();
        let mut video_dropped = 0u64;
        let mut audio_dropped = 0u64;

        loop {
            tokio::select! {
                _ = &mut *shutdown_rx => {
                    info!("[{}] Shutdown requested", ctx.stream_id);
                    return AttemptEnd::Shutdown;
                }
                _ = ct.cancelled() => {
                    return AttemptEnd::Failed {
                        reason: "Connection closed".to_string(),
                        connected: true,
                    };
                }
                result = video_rx.recv() => {
                    match result {
                        Some(data) => {
                            if has_video {
                                let _ = ctx.rtp_tx.send(MediaPacket::Rtp {
                                    channel: 0,
                                    data: data.into(),
                                });
                            } else {
                                video_dropped += 1;
                                if video_dropped == 1 || video_dropped.is_multiple_of(1000) {
                                    warn!(
                                        "[{}] Dropping video packets (not in codec snapshot): {}",
                                        ctx.stream_id, video_dropped
                                    );
                                }
                            }
                        }
                        None => {
                            return AttemptEnd::Failed {
                                reason: "Video channel closed".to_string(),
                                connected: true,
                            };
                        }
                    }
                }
                result = audio_rx.recv() => {
                    match result {
                        Some(data) => {
                            if has_audio {
                                let _ = ctx.rtp_tx.send(MediaPacket::Rtp {
                                    channel: audio_channel,
                                    data: data.into(),
                                });
                            } else {
                                audio_dropped += 1;
                                if audio_dropped == 1 || audio_dropped.is_multiple_of(1000) {
                                    warn!(
                                        "[{}] Dropping audio packets (not in codec snapshot): {}",
                                        ctx.stream_id, audio_dropped
                                    );
                                }
                            }
                        }
                        None => {
                            return AttemptEnd::Failed {
                                reason: "Audio channel closed".to_string(),
                                connected: true,
                            };
                        }
                    }
                }
            }
        }
    }

    async fn emit_state_change(
        state: &Arc<std::sync::RwLock<StreamSourceState>>,
        state_tx: &broadcast::Sender<StateChangeEvent>,
        new_state: StreamSourceState,
        error: Option<String>,
    ) {
        let event = {
            let mut s = state.write().unwrap();
            let old_state = *s;
            // No-op transitions carry no information (e.g. Disconnected after
            // an already-reported failure); skip them like the RTSP source
            // does instead of broadcasting duplicates.
            if old_state == new_state {
                return;
            }
            *s = new_state;

            StateChangeEvent {
                old_state,
                new_state,
                error,
            }
        };

        let _ = state_tx.send(event);
    }
}

/// Map a `whep://` / `wheps://` source URL to the `http(s)://` URL the WHEP
/// client POSTs to. A Bearer token can be carried as userinfo:
/// `whep://token@host:port/whep/stream`.
fn parse_whep_url(raw: &str) -> Result<(String, Option<String>)> {
    // Scheme matching is case-insensitive (RFC 3986). The replacement itself
    // is done textually: `whep` is not a WHATWG "special" scheme, so
    // `Url::set_scheme` refuses the conversion to `http(s)`.
    let http_url = match raw.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("whep") => format!("http://{rest}"),
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("wheps") => format!("https://{rest}"),
        _ => anyhow::bail!("Unsupported WHEP source URL: {}", super::redact_url(raw)),
    };

    let mut url = url::Url::parse(&http_url)?;
    if url.host_str().is_none() {
        anyhow::bail!(
            "Invalid WHEP source URL (no host): {}",
            super::redact_url(raw)
        );
    }

    // Only token-in-username is supported. A password means the user:pass
    // form, which has no mapping onto Bearer auth — fail fast instead of
    // silently dropping it (the error must not echo the URL: it contains
    // the credential).
    if url.password().is_some() {
        anyhow::bail!(
            "WHEP source URL must not carry a password; use whep://token@host… for Bearer auth"
        );
    }

    // `Url::username` is still percent-encoded; decode so tokens containing
    // reserved characters reach the Bearer header in their original form.
    let token = (!url.username().is_empty()).then(|| {
        percent_encoding::percent_decode_str(url.username())
            .decode_utf8_lossy()
            .into_owned()
    });

    // Strip userinfo unconditionally: the URL is used for requests and log
    // lines, neither of which may see the credential.
    url.set_username("")
        .map_err(|_| anyhow::anyhow!("Invalid WHEP source URL"))?;
    url.set_password(None)
        .map_err(|_| anyhow::anyhow!("Invalid WHEP source URL"))?;

    Ok((url.to_string(), token))
}

/// Short codec label for logs: the mime type, or `-` when absent.
fn mime_of(codec: &Option<RTCRtpCodecParameters>) -> &str {
    codec
        .as_ref()
        .map(|c| c.rtp_codec.mime_type.as_str())
        .unwrap_or("-")
}

/// Derive the media kinds negotiated in the WHEP answer SDP. A rejected
/// m-line (port 0) does not count as expected; with multiple m-lines of the
/// same kind, any active one counts (accumulate, not last-wins).
fn expected_media_kinds(answer_sdp: &str) -> (bool, bool) {
    let mut video = false;
    let mut audio = false;
    for line in answer_sdp.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("m=video") {
            video |= media_line_active(rest);
        } else if let Some(rest) = line.strip_prefix("m=audio") {
            audio |= media_line_active(rest);
        }
    }
    (video, audio)
}

fn media_line_active(rest: &str) -> bool {
    rest.split_whitespace()
        .next()
        .is_some_and(|port| port != "0")
}

fn codec_wait_satisfied(
    expected: (bool, bool),
    video_ready: bool,
    audio_ready: bool,
    partial_grace_elapsed: bool,
) -> bool {
    match expected {
        // Defensive fallback for an answer SDP we cannot classify.
        (false, false) => video_ready || audio_ready,
        (expected_video, expected_audio) => {
            // Prefer every negotiated codec, but allow a short partial-ready
            // grace for kinds that were negotiated yet never deliver.
            ((!expected_video || video_ready) && (!expected_audio || audio_ready))
                || ((video_ready || audio_ready) && partial_grace_elapsed)
        }
    }
}

#[async_trait]
impl StreamSource for WhepSource {
    fn stream_id(&self) -> &str {
        &self.config.stream_id
    }

    fn state(&self) -> StreamSourceState {
        *self.state.read().unwrap()
    }

    async fn start(&mut self) -> Result<()> {
        if self.task_handle.is_some() {
            anyhow::bail!("Source already started");
        }

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        let ctx = WhepClientContext {
            stream_id: self.config.stream_id.clone(),
            whep_url: self.whep_url.clone(),
            token: self.token.clone(),
            config: self.config.clone(),
            net: self.net.clone(),
            rtp_tx: self.rtp_tx.clone(),
            state: self.state.clone(),
            state_tx: self.state_tx.clone(),
            snapshot: self.snapshot.clone(),
            peer_store: self.peer_store.clone(),
        };

        let handle = tokio::spawn(async move {
            Self::run_whep_client(ctx, shutdown_rx).await;
        });

        self.task_handle = Some(handle);

        info!("[{}] Started", self.config.stream_id);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }

        Self::emit_state_change(
            &self.state,
            &self.state_tx,
            StreamSourceState::Disconnected,
            None,
        )
        .await;

        info!("[{}] Stopped", self.config.stream_id);
        Ok(())
    }

    fn subscribe_rtp(&self) -> broadcast::Receiver<MediaPacket> {
        self.rtp_tx.subscribe()
    }

    fn subscribe_state(&self) -> broadcast::Receiver<StateChangeEvent> {
        self.state_tx.subscribe()
    }

    async fn get_video_codec(&self) -> Option<RTCRtpCodecParameters> {
        self.snapshot.read().await.as_ref()?.video.clone()
    }

    async fn get_audio_codec(&self) -> Option<RTCRtpCodecParameters> {
        self.snapshot.read().await.as_ref()?.audio.clone()
    }

    async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        // Always hand out the wrapper, even while the peer is down: the
        // forwarding task resolves the current peer per message, so feedback
        // keeps flowing across reconnects and a bridge created during a
        // reconnect window does not permanently lose keyframe requests.
        let (wrapper_tx, mut wrapper_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let peer_store = self.peer_store.clone();
        let stream_id = self.config.stream_id.clone();

        tokio::spawn(async move {
            while let Some(data) = wrapper_rx.recv().await {
                let peer = peer_store.read().await.clone();
                match peer {
                    Some(peer) => livetwo::whep::forward_rtcp_to_peer(&data, &peer).await,
                    None => {
                        debug!("[{}] Dropping RTCP: WHEP peer not connected", stream_id);
                    }
                }
            }
        });

        Some(wrapper_tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_whep_url_maps_scheme() {
        let (url, token) = parse_whep_url("whep://example.com:7777/whep/cam1").unwrap();
        assert_eq!(url, "http://example.com:7777/whep/cam1");
        assert_eq!(token, None);
    }

    #[test]
    fn parse_whep_url_scheme_is_case_insensitive() {
        let (url, _) = parse_whep_url("WHEP://example.com:7777/whep/cam1").unwrap();
        assert_eq!(url, "http://example.com:7777/whep/cam1");
        let (url, _) = parse_whep_url("Wheps://example.com/whep/cam1").unwrap();
        assert_eq!(url, "https://example.com/whep/cam1");
    }

    #[test]
    fn parse_wheps_url_maps_to_https() {
        let (url, token) = parse_whep_url("wheps://example.com/whep/cam1").unwrap();
        assert_eq!(url, "https://example.com/whep/cam1");
        assert_eq!(token, None);
    }

    #[test]
    fn parse_whep_url_extracts_userinfo_token() {
        let (url, token) = parse_whep_url("whep://secret@example.com/whep/cam1").unwrap();
        assert_eq!(url, "http://example.com/whep/cam1");
        assert_eq!(token, Some("secret".to_string()));
    }

    #[test]
    fn parse_whep_url_decodes_percent_encoded_token() {
        let (url, token) = parse_whep_url("whep://tok%2Fen%3D@example.com/whep/cam1").unwrap();
        assert_eq!(url, "http://example.com/whep/cam1");
        assert_eq!(token, Some("tok/en=".to_string()));
    }

    #[test]
    fn parse_whep_url_rejects_other_schemes() {
        assert!(parse_whep_url("rtsp://example.com/stream").is_err());
    }

    #[test]
    fn parse_whep_url_rejects_password_without_leaking_it() {
        for raw in [
            "whep://user:s3cret@example.com/whep/cam1",
            "whep://:s3cret@example.com/whep/cam1",
        ] {
            let err = parse_whep_url(raw).unwrap_err();
            assert!(
                !err.to_string().contains("s3cret"),
                "error leaks the credential: {err}"
            );
        }
    }

    #[test]
    fn parse_whep_url_error_redacts_credentials() {
        let err = parse_whep_url("whelp://secret@example.com/whep/cam1").unwrap_err();
        assert!(!err.to_string().contains("secret"));
    }

    #[test]
    fn expected_kinds_ignore_rejected_m_lines() {
        let sdp = "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\na=rtpmap:96 H264/90000\r\nm=audio 0 UDP/TLS/RTP/SAVPF 111\r\n";
        assert_eq!(expected_media_kinds(sdp), (true, false));
    }

    #[test]
    fn expected_kinds_detect_both_kinds() {
        let sdp = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n";
        assert_eq!(expected_media_kinds(sdp), (true, true));
    }

    #[test]
    fn expected_kinds_accumulate_multiple_m_lines() {
        // An active m-line followed by a rejected one of the same kind must
        // still count (any active wins, not last-wins)…
        let sdp = "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\nm=video 0 UDP/TLS/RTP/SAVPF 97\r\n";
        assert_eq!(expected_media_kinds(sdp), (true, false));
        // …and a rejected line before the active one.
        let sdp = "v=0\r\nm=audio 0 UDP/TLS/RTP/SAVPF 111\r\nm=audio 9 UDP/TLS/RTP/SAVPF 8\r\n";
        assert_eq!(expected_media_kinds(sdp), (false, true));
    }

    #[test]
    fn codec_wait_requires_every_negotiated_kind() {
        assert!(!codec_wait_satisfied((true, true), true, false, false));
        assert!(codec_wait_satisfied((true, true), true, true, false));
        assert!(codec_wait_satisfied((true, false), true, false, false));
        assert!(!codec_wait_satisfied((false, true), true, false, false));
    }

    #[test]
    fn codec_wait_allows_partial_after_grace() {
        assert!(codec_wait_satisfied((true, true), true, false, true));
        assert!(!codec_wait_satisfied((true, true), false, false, true));
    }

    #[test]
    fn codec_wait_falls_back_to_any_codec() {
        assert!(codec_wait_satisfied((false, false), false, true, false));
        assert!(!codec_wait_satisfied((false, false), false, false, true));
    }

    #[test]
    fn snapshot_lacks_kind_only_when_upstream_adds_one() {
        let av = CodecSnapshot {
            video: Some(RTCRtpCodecParameters::default()),
            audio: Some(RTCRtpCodecParameters::default()),
        };
        let video_only = CodecSnapshot {
            video: Some(RTCRtpCodecParameters::default()),
            audio: None,
        };
        let empty = CodecSnapshot::default();

        // Same or fewer kinds across a reconnect are routable.
        assert!(!av.lacks_kind_of(&av));
        assert!(!av.lacks_kind_of(&video_only));
        assert!(!av.lacks_kind_of(&empty));
        // An added kind cannot be routed by the fixed channel mapping.
        assert!(video_only.lacks_kind_of(&av));
        assert!(empty.lacks_kind_of(&video_only));
    }

    #[test]
    fn audio_channel_matches_bridge_mapping() {
        let av = CodecSnapshot {
            video: Some(RTCRtpCodecParameters::default()),
            audio: Some(RTCRtpCodecParameters::default()),
        };
        let audio_only = CodecSnapshot {
            video: None,
            audio: Some(RTCRtpCodecParameters::default()),
        };
        assert_eq!(av.audio_channel(), 2);
        assert_eq!(audio_only.audio_channel(), 0);
    }

    #[test]
    fn connected_attempt_resets_reconnect_count() {
        assert_eq!(next_reconnect_count(0, false), 1);
        assert_eq!(next_reconnect_count(1, false), 2);
        assert_eq!(next_reconnect_count(5, true), 1);
    }

    fn codec_with(mime: &str, clock_rate: u32, channels: u16) -> RTCRtpCodecParameters {
        let mut codec = RTCRtpCodecParameters::default();
        codec.rtp_codec.mime_type = mime.to_string();
        codec.rtp_codec.clock_rate = clock_rate;
        codec.rtp_codec.channels = channels;
        codec
    }

    #[test]
    fn codec_mismatch_detects_changed_codec() {
        let h264 = CodecSnapshot {
            video: Some(codec_with("video/H264", 90000, 0)),
            audio: Some(codec_with("audio/opus", 48000, 2)),
        };

        // Same codecs (case-insensitive mime) are not a mismatch.
        let same = CodecSnapshot {
            video: Some(codec_with("video/h264", 90000, 0)),
            audio: Some(codec_with("audio/opus", 48000, 2)),
        };
        assert!(!h264.codec_mismatch_of(&same));

        // A kind present only on one side is not a mismatch (that is
        // lacks_kind_of's job).
        assert!(!h264.codec_mismatch_of(&CodecSnapshot::default()));

        for current in [
            CodecSnapshot {
                video: Some(codec_with("video/VP9", 90000, 0)),
                ..h264.clone()
            },
            CodecSnapshot {
                video: Some(codec_with("video/H264", 8000, 0)),
                ..h264.clone()
            },
            CodecSnapshot {
                audio: Some(codec_with("audio/opus", 48000, 1)),
                ..h264.clone()
            },
        ] {
            assert!(h264.codec_mismatch_of(&current));
        }
    }

    /// Spawn a pump with the given snapshot, feed it one video and one audio
    /// packet, and collect what reaches the bridge broadcast channel.
    async fn pump_collect(snapshot: CodecSnapshot) -> Vec<(u8, Vec<u8>)> {
        let (rtp_tx, _) = broadcast::channel(16);
        let mut rtp_rx = rtp_tx.subscribe();
        let (state_tx, _) = broadcast::channel(1);
        let ctx = WhepClientContext {
            stream_id: "test".to_string(),
            whep_url: String::new(),
            token: None,
            config: InternalSourceConfig {
                stream_id: "test".to_string(),
                url: String::new(),
            },
            net: SourceNetConfig::default(),
            rtp_tx,
            state: Arc::new(std::sync::RwLock::new(StreamSourceState::Initializing)),
            state_tx,
            snapshot: Arc::new(RwLock::new(None)),
            peer_store: Arc::new(RwLock::new(None)),
        };
        let (video_tx, mut video_rx) = mpsc::channel::<Vec<u8>>(16);
        let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(16);
        let ct = CancellationToken::new();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let pump = tokio::spawn({
            let ct = ct.clone();
            async move {
                WhepSource::pump(
                    &ctx,
                    snapshot,
                    &mut video_rx,
                    &mut audio_rx,
                    &ct,
                    &mut shutdown_rx,
                )
                .await
            }
        });

        video_tx.send(vec![1, 1, 1]).await.unwrap();
        audio_tx.send(vec![2, 2, 2]).await.unwrap();

        // Collect until the pump goes idle: kinds missing from the snapshot
        // must never produce a packet here.
        let mut received = Vec::new();
        loop {
            match tokio::time::timeout(Duration::from_millis(200), rtp_rx.recv()).await {
                Ok(Ok(packet)) => match packet {
                    MediaPacket::Rtp { channel, data } => received.push((channel, data.to_vec())),
                    #[allow(unreachable_patterns)]
                    other => panic!("unexpected non-RTP packet: {other:?}"),
                },
                Ok(Err(_)) => panic!("broadcast channel closed or lagged"),
                Err(_) => break,
            }
        }

        shutdown_tx.send(()).unwrap();
        assert!(matches!(pump.await.unwrap(), AttemptEnd::Shutdown));
        received
    }

    #[tokio::test]
    async fn pump_forwards_snapshot_kinds_on_mapped_channels() {
        let received = pump_collect(CodecSnapshot {
            video: Some(RTCRtpCodecParameters::default()),
            audio: Some(RTCRtpCodecParameters::default()),
        })
        .await;

        assert_eq!(received.len(), 2);
        assert!(received.contains(&(0, vec![1, 1, 1])));
        assert!(received.contains(&(2, vec![2, 2, 2])));
    }

    #[tokio::test]
    async fn pump_drops_video_missing_from_snapshot() {
        // Audio-only snapshot (accepted via the partial-ready grace): late
        // video packets must be dropped, not misrouted onto channel 0, which
        // the audio-only bridge mapping assigns to *audio*.
        let received = pump_collect(CodecSnapshot {
            video: None,
            audio: Some(RTCRtpCodecParameters::default()),
        })
        .await;

        assert_eq!(received, vec![(0, vec![2, 2, 2])]);
    }

    #[tokio::test]
    async fn pump_drops_audio_missing_from_snapshot() {
        // Video-only snapshot: late audio must be dropped; audio_channel()
        // degenerates to 0 here, which is the *video* channel.
        let received = pump_collect(CodecSnapshot {
            video: Some(RTCRtpCodecParameters::default()),
            audio: None,
        })
        .await;

        assert_eq!(received, vec![(0, vec![1, 1, 1])]);
    }
}
