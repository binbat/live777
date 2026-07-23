//! WHEP subscribe loadtest: N concurrent subscribers on one stream, exercising
//! the SFU's fan-out path. Each session drains the received WebRTC RTP tracks
//! internally and counts packets/bytes; it does not decode or forward media to
//! UDP, so the per-session overhead stays low.
//!
//! Optionally, a single rotating verifier decodes one session at a time with
//! rsmpeg/FFmpeg (requires the `rsmpeg` feature): every `verify_window` the
//! decoder switches to the next live session, so decode cost stays constant
//! regardless of the session count while coverage approaches all sessions over
//! a long run.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use libwish::Client;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::info;
use webrtc::peer_connection::RTCPeerConnectionState;

#[cfg(feature = "rsmpeg")]
use std::sync::atomic::AtomicBool;

use super::{LoadtestConfig, LoadtestStats, SessionMetrics, SessionOutcome, run_sessions};
use crate::utils::shutdown::graceful_shutdown;

/// Grace period for a transient `Disconnected` state: ICE connectivity loss
/// may recover on its own, so only a disconnection that outlives the grace
/// period (or a `Failed`/`Closed` state) ends the session.
const DISCONNECTED_GRACE: Duration = Duration::from_secs(5);

/// Grace period after setup completes during which a stop with zero received
/// packets still counts as cancelled rather than failed: media should flow
/// immediately once subscribed, so a short grace distinguishes "cancelled
/// right after setup" from "SFU never forwarded media".
const MEDIA_FLOW_GRACE: Duration = Duration::from_secs(2);

/// Capacity of the channel carrying RTP packets from the currently verified
/// session to the decoder task.
#[cfg(feature = "rsmpeg")]
const VERIFY_CHANNEL_CAPACITY: usize = 1024;

/// Parameters shared by all subscribe sessions.
#[derive(Debug, Clone)]
pub struct WhepLoadParams {
    /// WHEP endpoint of the published stream, e.g. `http://localhost:7777/whep/live`.
    /// A publisher must be running on that stream (e.g. `livewrk whip`; note
    /// that the whip subcommand appends a `-N` suffix to the last path
    /// segment).
    pub whep_url: String,
    pub token: Option<String>,
    /// STUN server URL used for ICE gathering. `None` or a blank string
    /// disables STUN (host candidates only), same as the WHIP side's
    /// `--stun-server ""`.
    pub stun_server: Option<String>,
    /// When set, a single rotating verifier decodes one session at a time and
    /// switches to the next live session every interval. Requires the
    /// `rsmpeg` feature.
    pub verify_window: Option<Duration>,
}

/// Aggregate stats of the rotating decode verifier.
#[derive(Debug, Default)]
pub struct VerifyStats {
    /// Windows that ran to completion; windows cut short by shutdown are not
    /// counted.
    pub windows_total: u64,
    /// Windows that decoded at least one valid frame.
    pub windows_ok: u64,
    /// Windows that decoded nothing or hit a decoder error.
    pub windows_failed: u64,
    /// Total frames decoded across all windows.
    pub frames_decoded: u64,
    /// Sessions that completed at least one window.
    pub sessions_covered: BTreeSet<usize>,
    /// Sessions with at least one failed window.
    pub sessions_failed: BTreeSet<usize>,
    /// Why verification produced no windows (e.g. an unsupported codec).
    pub note: Option<String>,
    /// Error of the most recent failed window.
    pub last_error: Option<String>,
}

/// Run `config.session_count` WHEP subscribers against `params.whep_url`.
///
/// Returns the aggregate loadtest stats and, when `params.verify_window` is
/// set, the stats of the rotating decode verifier.
pub async fn run(
    config: &LoadtestConfig,
    params: WhepLoadParams,
    ct: CancellationToken,
) -> Result<(LoadtestStats, Option<VerifyStats>)> {
    let session_ct = ct.child_token();
    let verifier = VerifierHandle::start(params.verify_window, &session_ct)?;
    let session_verify = verifier.session_verify();

    let run_ct = session_ct.clone();
    let result = run_sessions(config, session_ct.clone(), move |i| {
        let params = params.clone();
        let verify = session_verify.clone();
        let run_ct = run_ct.child_token();
        async move { run_one(i, params, verify, run_ct).await }
    })
    .await;

    // Stop the verifier however the sessions ended, and collect its stats
    // before propagating any session error.
    session_ct.cancel();
    let verify_stats = verifier.finish().await;
    let stats = result?;
    Ok((stats, verify_stats))
}

async fn run_one(
    index: usize,
    params: WhepLoadParams,
    verify: Option<SessionVerify>,
    ct: CancellationToken,
) -> (SessionMetrics, Result<SessionOutcome>) {
    let packets = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));

    let (connected_duration, result) =
        run_session(index, &params, verify, ct, packets.clone(), bytes.clone()).await;

    // Metrics are returned even on failure so traffic from sessions that die
    // mid-run still counts towards the aggregate.
    let metrics = SessionMetrics {
        packets: packets.load(Ordering::Relaxed),
        bytes: bytes.load(Ordering::Relaxed),
        connected_duration,
        ..Default::default()
    };
    (metrics, result)
}

async fn run_session(
    index: usize,
    params: &WhepLoadParams,
    verify: Option<SessionVerify>,
    ct: CancellationToken,
    packets: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
) -> (Duration, Result<SessionOutcome>) {
    const MEDIA_CHANNEL_CAPACITY: usize = 512;

    let (video_tx, video_rx) = mpsc::channel::<Vec<u8>>(MEDIA_CHANNEL_CAPACITY);
    let (audio_tx, audio_rx) = mpsc::channel::<Vec<u8>>(MEDIA_CHANNEL_CAPACITY);
    let codec_info = Arc::new(tokio::sync::Mutex::new(rtsp::CodecInfo::new()));
    let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
    let (video_mime_tx, mut video_mime_rx) = watch::channel(None::<String>);

    let auth = match Client::get_auth_header_map(params.token.clone()) {
        Ok(auth) => auth,
        Err(e) => return (Duration::ZERO, Err(e)),
    };
    let mut client = Client::new(params.whep_url.clone(), auth);
    let peer_ct = CancellationToken::new();

    // Race the setup (WHEP POST/answer + ICE) against cancellation so a
    // session stopped while connecting still reports back instead of being
    // aborted at the drain grace.
    let (peer, _answer, _stats, mut dc_recv_rx, _dc_send_tx) = tokio::select! {
        result = crate::whep::setup_whep_peer(
            peer_ct.clone(),
            &mut client,
            video_tx,
            audio_tx,
            codec_info,
            crate::whep::stun_ice_servers(params.stun_server.as_deref()),
            Some(state_tx),
            verify.is_some().then_some(video_mime_tx),
        ) => match result {
            Ok(parts) => parts,
            Err(e) => {
                // Same cleanup as the cancel branch below: the WHEP POST may
                // already have gone through (e.g. set_configuration or
                // set_remote_description failed), so remove the server-side
                // resource instead of leaking the session.
                peer_ct.cancel();
                if client.session_url.is_some() {
                    let _ = client.remove_resource().await;
                }
                return (Duration::ZERO, Err(e));
            }
        },
        _ = ct.cancelled() => {
            // The half-built peer goes away with the setup future; remove the
            // server-side resource when the WHEP POST already went through so
            // the stream does not leak.
            if client.session_url.is_some() {
                let _ = client.remove_resource().await;
            }
            peer_ct.cancel();
            return (Duration::ZERO, Ok(SessionOutcome::Cancelled));
        }
    };

    // The media path is ready: connected time starts here.
    let connected_at = std::time::Instant::now();

    if let Some(verify) = &verify {
        // Report the negotiated video codec to the verifier once the track
        // announces it; all sessions subscribe to the same stream, so one
        // shared value is enough.
        let mime_tx = verify.mime_tx.clone();
        let mime_ct = peer_ct.clone();
        tokio::spawn(async move {
            loop {
                if let Some(mime) = video_mime_rx.borrow().clone() {
                    let _ = mime_tx.send(Some(mime));
                    break;
                }
                tokio::select! {
                    changed = video_mime_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                    // Tied to the session so it cannot outlive it (audio-only
                    // stream or early peer error).
                    _ = mime_ct.cancelled() => break,
                }
            }
        });
    }

    // The data-channel receive side has no consumer in the loadtest; drain it
    // until the channel closes so a publisher with data-channel traffic
    // cannot grow the unbounded channel. Exits with the session.
    {
        let drain_ct = peer_ct.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = dc_recv_rx.recv() => {
                        if msg.is_none() {
                            break;
                        }
                    }
                    _ = drain_ct.cancelled() => break,
                }
            }
        });
    }

    // Only fully set-up sessions may be picked by the verifier; the guard
    // unregisters on every exit path.
    let active_guard = verify
        .as_ref()
        .map(|v| ActiveGuard::register(&v.active, index));

    let video_handle = tokio::spawn(drain_video(
        index,
        video_rx,
        packets.clone(),
        bytes.clone(),
        verify,
        peer_ct.clone(),
    ));
    let audio_handle = tokio::spawn(drain_media(
        "audio",
        audio_rx,
        packets.clone(),
        bytes.clone(),
        peer_ct.clone(),
    ));

    let stop_reason = wait_for_stop(ct.clone(), peer_ct.clone(), state_rx).await;
    // Unregister before teardown so the verifier cannot pick a session that
    // is already shutting down.
    drop(active_guard);
    // Connected time stops when the session is asked to stop, before the
    // teardown budget, so it measures actual connected time.
    let connected_duration = connected_at.elapsed();
    peer_ct.cancel();
    graceful_shutdown("WHEP loadtest", &mut client, peer).await;
    let drains = tokio::join!(video_handle, audio_handle);
    // A panicking drain task must not fail the session, but keep the signal.
    for (kind, result) in [("video", drains.0), ("audio", drains.1)] {
        if let Err(e) = result {
            tracing::warn!(kind, error = ?e, "WHEP loadtest media drain task failed");
        }
    }

    let packets = packets.load(Ordering::Relaxed);
    let bytes = bytes.load(Ordering::Relaxed);
    if let StopReason::PeerEnded(state) = stop_reason {
        return (
            connected_duration,
            Err(anyhow!(
                "WHEP subscriber peer ended unexpectedly: state={state}, packets={packets}, bytes={bytes}, url={}",
                params.whep_url
            )),
        );
    }
    if packets == 0 {
        // Within the media-flow grace this is a cancellation right after
        // setup, not a broken pipeline.
        if connected_duration < MEDIA_FLOW_GRACE {
            return (connected_duration, Ok(SessionOutcome::Cancelled));
        }
        return (
            connected_duration,
            Err(anyhow!(
                "WHEP subscriber received no media packets from {}",
                params.whep_url
            )),
        );
    }

    (connected_duration, Ok(SessionOutcome::Connected))
}

/// Drain the video track, counting packets/bytes. While this session holds
/// the verification token, packets are additionally forwarded to the rotating
/// verifier's decoder.
///
/// `ct` fires at session teardown: the channel senders live in the peer
/// connection's event handler, whose lifetime is not tied to teardown, so the
/// drains exit on the token instead of waiting for every sender to drop.
async fn drain_video(
    index: usize,
    mut rx: mpsc::Receiver<Vec<u8>>,
    packets: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
    verify: Option<SessionVerify>,
    ct: CancellationToken,
) {
    let Some(verify) = verify else {
        drain_media("video", rx, packets, bytes, ct).await;
        return;
    };
    let mut target_rx = verify.target_rx;
    let verify_tx = verify.verify_tx;

    loop {
        tokio::select! {
            packet = rx.recv() => {
                let Some(packet) = packet else { break };
                packets.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(packet.len() as u64, Ordering::Relaxed);
                if *target_rx.borrow() == Some(index) {
                    // The decoder is the only consumer and runs at its own
                    // pace; when it backs up, drop rather than stall the drain.
                    // The tag lets the verifier drop stale packets from a
                    // previously targeted session after a rotation.
                    let _ = verify_tx.try_send((index, packet));
                }
            }
            changed = target_rx.changed() => {
                if changed.is_err() {
                    // The verifier is gone (the run is ending); keep draining
                    // so the traffic counters stay accurate.
                    drain_media("video", rx, packets, bytes, ct).await;
                    return;
                }
            }
            _ = ct.cancelled() => break,
        }
    }

    info!(kind = "video", "WHEP loadtest media drain stopped");
}

async fn drain_media(
    kind: &'static str,
    mut rx: mpsc::Receiver<Vec<u8>>,
    packets: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
    ct: CancellationToken,
) {
    loop {
        tokio::select! {
            packet = rx.recv() => {
                let Some(packet) = packet else { break };
                packets.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(packet.len() as u64, Ordering::Relaxed);
            }
            _ = ct.cancelled() => break,
        }
    }

    info!(kind, "WHEP loadtest media drain stopped");
}

/// Per-session handle to the rotating verifier. Sessions consult `target_rx`
/// per packet and forward to `verify_tx` only while targeted, so at most one
/// session feeds the decoder at any time. Packets carry the sender's session
/// index so the verifier can drop stragglers from the previous target after
/// a rotation instead of feeding a foreign sequence space to the decoder.
#[derive(Clone)]
struct SessionVerify {
    target_rx: watch::Receiver<Option<usize>>,
    verify_tx: mpsc::Sender<(usize, Vec<u8>)>,
    active: Arc<std::sync::Mutex<BTreeSet<usize>>>,
    mime_tx: Arc<watch::Sender<Option<String>>>,
}

/// Registers a session as verifiable on creation and unregisters on drop.
struct ActiveGuard {
    active: Arc<std::sync::Mutex<BTreeSet<usize>>>,
    index: usize,
}

impl ActiveGuard {
    fn register(active: &Arc<std::sync::Mutex<BTreeSet<usize>>>, index: usize) -> Self {
        // Tolerate poisoning (like Drop does): a panicking session task must
        // not cascade-abort the whole run.
        active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(index);
        Self {
            active: Arc::clone(active),
            index,
        }
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(&self.index);
        }
    }
}

/// Owns the verifier task; created by [`run`] when verification is enabled.
struct VerifierHandle {
    #[cfg(feature = "rsmpeg")]
    inner: Option<VerifierInner>,
}

#[cfg(feature = "rsmpeg")]
struct VerifierInner {
    session: SessionVerify,
    task: tokio::task::JoinHandle<VerifyStats>,
}

impl VerifierHandle {
    #[cfg(feature = "rsmpeg")]
    fn start(window: Option<Duration>, parent_ct: &CancellationToken) -> Result<Self> {
        let Some(window) = window else {
            return Ok(Self { inner: None });
        };
        let (target_tx, target_rx) = watch::channel(None);
        let (verify_tx, verify_rx) = mpsc::channel(VERIFY_CHANNEL_CAPACITY);
        let active = Arc::new(std::sync::Mutex::new(BTreeSet::new()));
        let (mime_tx, mime_rx) = watch::channel(None::<String>);
        let ct = parent_ct.child_token();
        let task = tokio::spawn(run_verifier(
            verify_rx,
            target_tx,
            Arc::clone(&active),
            mime_rx,
            window,
            ct,
        ));
        Ok(Self {
            inner: Some(VerifierInner {
                session: SessionVerify {
                    target_rx,
                    verify_tx,
                    active,
                    mime_tx: Arc::new(mime_tx),
                },
                task,
            }),
        })
    }

    #[cfg(not(feature = "rsmpeg"))]
    fn start(window: Option<Duration>, _parent_ct: &CancellationToken) -> Result<Self> {
        if window.is_some() {
            anyhow::bail!(
                "WHEP decode verification requires the `rsmpeg` feature; rebuild with --features rsmpeg"
            );
        }
        Ok(Self {})
    }

    fn session_verify(&self) -> Option<SessionVerify> {
        #[cfg(feature = "rsmpeg")]
        if let Some(inner) = &self.inner {
            return Some(inner.session.clone());
        }
        None
    }

    async fn finish(self) -> Option<VerifyStats> {
        #[cfg(feature = "rsmpeg")]
        if let Some(inner) = self.inner {
            match inner.task.await {
                Ok(stats) => return Some(stats),
                Err(e) => {
                    tracing::warn!(error = ?e, "WHEP verifier task failed; all decode verification results were lost")
                }
            }
        }
        None
    }
}

/// The verifier loop: wait for the negotiated codec, then decode one live
/// session per window in round-robin order until shutdown.
#[cfg(feature = "rsmpeg")]
async fn run_verifier(
    mut verify_rx: mpsc::Receiver<(usize, Vec<u8>)>,
    target_tx: watch::Sender<Option<usize>>,
    active: Arc<std::sync::Mutex<BTreeSet<usize>>>,
    mut mime_rx: watch::Receiver<Option<String>>,
    window: Duration,
    ct: CancellationToken,
) -> VerifyStats {
    let mut stats = VerifyStats::default();

    // All sessions subscribe to the same stream, so the first negotiated
    // codec applies to every window.
    let mime = loop {
        if let Some(mime) = mime_rx.borrow().clone() {
            break mime;
        }
        tokio::select! {
            _ = ct.cancelled() => {
                stats.note = Some("no video track announced a codec".to_string());
                return stats;
            }
            changed = mime_rx.changed() => {
                if changed.is_err() {
                    // Every session went away without announcing a video codec
                    // (e.g. an audio-only stream, or the mime reporting task
                    // died early).
                    stats.note = Some("no video track announced a codec".to_string());
                    return stats;
                }
            }
        }
    };

    if !crate::probe::decoder::supports_mime(&mime) {
        tracing::warn!(%mime, "decode verifier does not support this codec, verification disabled");
        stats.note = Some(format!(
            "codec {mime} is not supported by the decode verifier"
        ));
        return stats;
    }

    info!(%mime, ?window, "WHEP decode verifier started");

    let mut cursor = 0usize;
    loop {
        let Some(index) = pick_next_active(&active, &mut cursor, &ct).await else {
            break;
        };

        // Switch the target first, then drop stragglers from the previously
        // verified session: its drain task may not have observed the switch
        // yet, and anything still queued belongs to the old session's
        // sequence space. Packets that race in afterwards are dropped by tag
        // inside the decode window.
        let _ = target_tx.send(Some(index));
        while verify_rx.try_recv().is_ok() {}

        match decode_window(&mime, &mut verify_rx, index, window, &ct).await {
            WindowOutcome::Cancelled => break,
            WindowOutcome::Done(result) => {
                stats.windows_total += 1;
                stats.sessions_covered.insert(index);
                match result {
                    Ok((width, height, frames)) if frames > 0 && width > 0 && height > 0 => {
                        stats.windows_ok += 1;
                        stats.frames_decoded += u64::from(frames);
                        tracing::debug!(session = index, frames, width, height, "verify window ok");
                    }
                    Ok((_, _, frames)) => {
                        stats.windows_failed += 1;
                        stats.sessions_failed.insert(index);
                        stats.last_error = Some(format!(
                            "session {index}: decoded {frames} frames in {window:?}"
                        ));
                        tracing::warn!(session = index, frames, "verify window failed");
                    }
                    Err(e) => {
                        stats.windows_failed += 1;
                        stats.sessions_failed.insert(index);
                        stats.last_error = Some(format!("session {index}: {e:#}"));
                        tracing::warn!(session = index, error = ?e, "verify window failed");
                    }
                }
            }
        }
    }

    stats
}

/// Pick the next live session in round-robin order, waiting for one to appear
/// when the set is empty. Returns `None` on shutdown.
#[cfg(feature = "rsmpeg")]
async fn pick_next_active(
    active: &Arc<std::sync::Mutex<BTreeSet<usize>>>,
    cursor: &mut usize,
    ct: &CancellationToken,
) -> Option<usize> {
    loop {
        {
            // Tolerate poisoning: a panicking session task must not
            // cascade-abort the whole run.
            let active = active.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(&index) = active
                .range(*cursor..)
                .next()
                .or_else(|| active.iter().next())
            {
                *cursor = index + 1;
                return Some(index);
            }
        }
        tokio::select! {
            _ = ct.cancelled() => return None,
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

/// Outcome of one verification window.
#[cfg(feature = "rsmpeg")]
enum WindowOutcome {
    /// Shutdown or the end of all sessions cut the window short; it is not
    /// recorded in the stats.
    Cancelled,
    /// The window ran its course (or the decoder died on its own); carries
    /// the decoder result `(width, height, frames)`.
    Done(Result<(u32, u32, u32)>),
}

/// Why the packet forwarding loop of a window ended.
#[cfg(feature = "rsmpeg")]
enum ForwardEnd {
    /// The window duration elapsed.
    Expired,
    /// The decoder thread is gone; its result carries the reason.
    DecoderGone,
    /// Shutdown started or every session went away.
    Shutdown,
}

/// Decode the currently targeted session's packets for `window`, then report
/// what the decoder saw. Packets tagged with a different session index are
/// stragglers from a previous target and are dropped, never decoded.
#[cfg(feature = "rsmpeg")]
async fn decode_window(
    mime: &str,
    verify_rx: &mut mpsc::Receiver<(usize, Vec<u8>)>,
    target: usize,
    window: Duration,
    ct: &CancellationToken,
) -> WindowOutcome {
    /// Extra time the decoder thread gets past the window to flush; the join
    /// timeout below adds a margin on top of its own deadline.
    const DECODER_GRACE: Duration = Duration::from_secs(2);

    let (packet_tx, packet_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(VERIFY_CHANNEL_CAPACITY);
    let cancelled = Arc::new(AtomicBool::new(false));
    let decoder = tokio::task::spawn_blocking({
        let mime = mime.to_string();
        let cancelled = Arc::clone(&cancelled);
        move || {
            crate::probe::decoder::run_ffi_decoder(
                mime,
                None,
                packet_rx,
                cancelled,
                window + DECODER_GRACE,
            )
        }
    });

    // Forward packets until the window expires, shutdown starts, every
    // session is gone, or the decoder thread dies. Only shutdown skips
    // recording; a dead decoder is exactly the failure the window measures.
    // The deadline is created once and pinned: a per-iteration `sleep(window)`
    // would restart on every packet and never fire under continuous traffic.
    let deadline = tokio::time::sleep(window);
    tokio::pin!(deadline);
    let end = tokio::select! {
        _ = ct.cancelled() => ForwardEnd::Shutdown,
        end = async {
            loop {
                tokio::select! {
                    packet = verify_rx.recv() => {
                        match packet {
                            Some((session, packet)) => {
                                // Stale packet from a previously targeted
                                // session whose forwarder had not observed the
                                // target switch yet: drop it instead of feeding
                                // a foreign sequence space to the decoder.
                                if session != target {
                                    continue;
                                }
                                match packet_tx.try_send(packet) {
                                    Ok(()) => {}
                                    // The decoder is backed up: drop the
                                    // packet (RTP decoding tolerates loss)
                                    // rather than stall the forward loop,
                                    // mirroring the upstream try_send.
                                    Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                                        break ForwardEnd::DecoderGone;
                                    }
                                }
                            }
                            None => break ForwardEnd::Shutdown,
                        }
                    }
                    _ = &mut deadline => break ForwardEnd::Expired,
                }
            }
        } => end,
    };

    cancelled.store(true, Ordering::Relaxed);
    let result = tokio::time::timeout(DECODER_GRACE * 2, decoder).await;
    if matches!(end, ForwardEnd::Shutdown) {
        return WindowOutcome::Cancelled;
    }
    match result {
        Ok(Ok(result)) => WindowOutcome::Done(result),
        // A JoinError also covers cancellation from runtime shutdown, so only
        // call it a panic when it actually is one.
        Ok(Err(e)) if e.is_panic() => {
            WindowOutcome::Done(Err(anyhow!("decoder task panicked: {e}")))
        }
        Ok(Err(e)) => WindowOutcome::Done(Err(anyhow!("decoder task was cancelled: {e}"))),
        // Aborting a blocking task does not stop the thread; it still exits
        // on its own deadline. Detach and count the window failed.
        Err(_) => WindowOutcome::Done(Err(anyhow!("decoder thread did not stop after the window"))),
    }
}

enum StopReason {
    Requested,
    PeerEnded(RTCPeerConnectionState),
}

async fn wait_for_stop(
    ct: CancellationToken,
    peer_ct: CancellationToken,
    mut state_rx: watch::Receiver<RTCPeerConnectionState>,
) -> StopReason {
    loop {
        let state = *state_rx.borrow_and_update();
        if matches!(
            state,
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed
        ) {
            return StopReason::PeerEnded(state);
        }
        if state == RTCPeerConnectionState::Disconnected {
            // Disconnected is potentially transient (ICE may recover); only
            // treat it as fatal when it outlives the grace period.
            tokio::select! {
                _ = ct.cancelled() => return StopReason::Requested,
                _ = peer_ct.cancelled() => {
                    return StopReason::PeerEnded(*state_rx.borrow());
                }
                _ = tokio::time::sleep(DISCONNECTED_GRACE) => {
                    return StopReason::PeerEnded(RTCPeerConnectionState::Disconnected);
                }
                changed = state_rx.changed() => {
                    if changed.is_err() {
                        return StopReason::PeerEnded(*state_rx.borrow());
                    }
                    continue;
                }
            }
        }
        tokio::select! {
            _ = ct.cancelled() => return StopReason::Requested,
            _ = peer_ct.cancelled() => {
                return StopReason::PeerEnded(*state_rx.borrow());
            }
            changed = state_rx.changed() => {
                if changed.is_err() {
                    return StopReason::PeerEnded(*state_rx.borrow());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drain_video_forwards_packets_while_targeted() {
        let (tx, rx) = mpsc::channel(8);
        let (_target_tx, target_rx) = watch::channel(Some(0usize));
        let (verify_tx, mut verify_rx) = mpsc::channel(8);
        let verify = SessionVerify {
            target_rx,
            verify_tx,
            active: Arc::new(std::sync::Mutex::new(BTreeSet::new())),
            mime_tx: Arc::new(watch::channel(None::<String>).0),
        };
        let packets = Arc::new(AtomicU64::new(0));
        let bytes = Arc::new(AtomicU64::new(0));

        let handle = tokio::spawn(drain_video(
            0,
            rx,
            packets.clone(),
            bytes.clone(),
            Some(verify),
            CancellationToken::new(),
        ));

        tx.send(vec![1, 2, 3]).await.unwrap();
        tx.send(vec![4, 5]).await.unwrap();
        drop(tx);
        handle.await.unwrap();

        assert_eq!(packets.load(Ordering::Relaxed), 2);
        assert_eq!(bytes.load(Ordering::Relaxed), 5);
        assert_eq!(verify_rx.recv().await.unwrap(), (0, vec![1, 2, 3]));
        assert_eq!(verify_rx.recv().await.unwrap(), (0, vec![4, 5]));
    }

    #[tokio::test]
    async fn drain_video_only_counts_when_not_targeted() {
        let (tx, rx) = mpsc::channel(8);
        let (_target_tx, target_rx) = watch::channel(Some(1usize));
        let (verify_tx, mut verify_rx) = mpsc::channel(8);
        let verify = SessionVerify {
            target_rx,
            verify_tx,
            active: Arc::new(std::sync::Mutex::new(BTreeSet::new())),
            mime_tx: Arc::new(watch::channel(None::<String>).0),
        };
        let packets = Arc::new(AtomicU64::new(0));
        let bytes = Arc::new(AtomicU64::new(0));

        let handle = tokio::spawn(drain_video(
            0,
            rx,
            packets.clone(),
            bytes.clone(),
            Some(verify),
            CancellationToken::new(),
        ));

        tx.send(vec![1, 2, 3]).await.unwrap();
        drop(tx);
        handle.await.unwrap();

        assert_eq!(packets.load(Ordering::Relaxed), 1);
        assert_eq!(bytes.load(Ordering::Relaxed), 3);
        // The session was never targeted: nothing reaches the verifier and the
        // channel is closed once the drain task exits.
        assert!(verify_rx.recv().await.is_none());
    }

    #[cfg(feature = "rsmpeg")]
    #[tokio::test]
    async fn pick_next_active_round_robins() {
        let active = Arc::new(std::sync::Mutex::new(BTreeSet::from([0, 2, 5])));
        let mut cursor = 0;
        let ct = CancellationToken::new();

        assert_eq!(pick_next_active(&active, &mut cursor, &ct).await, Some(0));
        assert_eq!(pick_next_active(&active, &mut cursor, &ct).await, Some(2));
        assert_eq!(pick_next_active(&active, &mut cursor, &ct).await, Some(5));
        // Wraps around to the beginning.
        assert_eq!(pick_next_active(&active, &mut cursor, &ct).await, Some(0));
        // Removed sessions are skipped.
        active.lock().unwrap().remove(&2);
        assert_eq!(pick_next_active(&active, &mut cursor, &ct).await, Some(5));
    }

    #[cfg(feature = "rsmpeg")]
    #[tokio::test]
    async fn pick_next_active_returns_none_when_cancelled() {
        let active = Arc::new(std::sync::Mutex::new(BTreeSet::new()));
        let mut cursor = 0;
        let ct = CancellationToken::new();
        ct.cancel();
        assert_eq!(pick_next_active(&active, &mut cursor, &ct).await, None);
    }

    #[tokio::test]
    async fn wait_for_stop_reports_requested_cancel() {
        let ct = CancellationToken::new();
        let peer_ct = CancellationToken::new();
        let (_state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);

        ct.cancel();

        assert!(matches!(
            wait_for_stop(ct, peer_ct, state_rx).await,
            StopReason::Requested
        ));
    }

    #[tokio::test]
    async fn wait_for_stop_reports_peer_terminal_state() {
        let ct = CancellationToken::new();
        let peer_ct = CancellationToken::new();
        let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);

        state_tx.send(RTCPeerConnectionState::Failed).unwrap();
        peer_ct.cancel();

        assert!(matches!(
            wait_for_stop(ct, peer_ct, state_rx).await,
            StopReason::PeerEnded(RTCPeerConnectionState::Failed)
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_stop_tolerates_transient_disconnect() {
        let ct = CancellationToken::new();
        let peer_ct = CancellationToken::new();
        let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::Connected);

        let wait = tokio::spawn(wait_for_stop(ct.clone(), peer_ct, state_rx));

        state_tx.send(RTCPeerConnectionState::Disconnected).unwrap();
        tokio::time::sleep(DISCONNECTED_GRACE / 2).await;
        state_tx.send(RTCPeerConnectionState::Connected).unwrap();
        tokio::time::sleep(DISCONNECTED_GRACE * 2).await;
        assert!(!wait.is_finished());

        ct.cancel();
        assert!(matches!(wait.await.unwrap(), StopReason::Requested));
    }

    #[tokio::test(start_paused = true)]
    async fn wait_for_stop_fails_after_disconnect_grace() {
        let ct = CancellationToken::new();
        let peer_ct = CancellationToken::new();
        let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::Connected);

        let wait = tokio::spawn(wait_for_stop(ct, peer_ct, state_rx));

        state_tx.send(RTCPeerConnectionState::Disconnected).unwrap();
        tokio::time::sleep(DISCONNECTED_GRACE * 2).await;

        assert!(matches!(
            wait.await.unwrap(),
            StopReason::PeerEnded(RTCPeerConnectionState::Disconnected)
        ));
    }
}
