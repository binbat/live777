//! WHEP pull source.
//!
//! Connects out to an upstream WHEP endpoint (typically another live777
//! node) and ingests the media as this stream's input, on par with the RTSP
//! and SDP sources. Built on livetwo's WHEP peer machinery, so it takes part
//! in the whole source lifecycle (on-demand start/stop, reconnect, codec
//! readiness, RTCP feedback) like any other source.

use super::{InternalSourceConfig, MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
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
    /// kinds are present, first otherwise. Mirrors the bridge's
    /// `ChannelMapping`.
    fn audio_channel(&self) -> u8 {
        if self.video.is_some() { 2 } else { 0 }
    }
}

struct WhepClientContext {
    stream_id: String,
    whep_url: String,
    token: Option<String>,
    config: InternalSourceConfig,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state: Arc<RwLock<StreamSourceState>>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    snapshot: Arc<RwLock<Option<CodecSnapshot>>>,
    peer_store: Arc<RwLock<Option<Arc<dyn PeerConnection>>>>,
}

enum AttemptEnd {
    Shutdown,
    Failed(String),
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
    state: Arc<RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    task_handle: Option<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    snapshot: Arc<RwLock<Option<CodecSnapshot>>>,
    peer_store: Arc<RwLock<Option<Arc<dyn PeerConnection>>>>,
}

impl WhepSource {
    pub fn new(config: InternalSourceConfig, whep_url: &str) -> Result<Self> {
        let (http_url, token) = parse_whep_url(whep_url)?;
        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Ok(Self {
            config,
            whep_url: http_url,
            token,
            state: Arc::new(RwLock::new(StreamSourceState::Initializing)),
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

            match Self::run_attempt(&ctx, &mut shutdown_rx).await {
                AttemptEnd::Shutdown => break,
                AttemptEnd::Failed(reason) => {
                    warn!("[{}] WHEP session ended: {}", ctx.stream_id, reason);
                    Self::emit_state_change(
                        &ctx.state,
                        &ctx.state_tx,
                        StreamSourceState::Disconnected,
                        Some(reason),
                    )
                    .await;
                }
            }

            if !ctx.config.reconnect_enabled() {
                info!("[{}] Reconnect disabled, exiting", ctx.stream_id);
                break;
            }

            reconnect_count += 1;

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
                ctx.config.reconnect_interval_ms(),
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
                    ctx.config.reconnect_interval_ms(),
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
        let mut client = Client::new(
            ctx.whep_url.clone(),
            Client::get_auth_header_map(ctx.token.clone()),
        );
        let (video_tx, mut video_rx) = mpsc::channel::<Vec<u8>>(MEDIA_CHANNEL_CAPACITY);
        let (audio_tx, mut audio_rx) = mpsc::channel::<Vec<u8>>(MEDIA_CHANNEL_CAPACITY);
        let codec_info = Arc::new(Mutex::new(rtsp::CodecInfo::new()));

        let (peer, answer, ..) = match livetwo::whep::setup_whep_peer(
            ct.clone(),
            &mut client,
            video_tx,
            audio_tx,
            codec_info.clone(),
            None,
            None,
        )
        .await
        {
            Ok(connected) => connected,
            Err(e) => {
                error!("[{}] WHEP connection failed: {}", ctx.stream_id, e);
                return AttemptEnd::Failed(format!("Connection failed: {}", e));
            }
        };
        *ctx.peer_store.write().await = Some(peer.clone());

        // The codec snapshot is taken once and reused across reconnects so
        // the bridge's channel mapping stays stable.
        let snapshot = ctx.snapshot.read().await.clone();
        let snapshot = match snapshot {
            Some(snapshot) => Some(snapshot),
            None => {
                match Self::wait_for_codecs(ctx, &ct, &codec_info, &answer.sdp, shutdown_rx).await {
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
            None => AttemptEnd::Failed("Codec wait timed out".to_string()),
        };

        Self::cleanup(ctx, &mut client, peer).await;
        end
    }

    async fn cleanup(ctx: &WhepClientContext, client: &mut Client, peer: Arc<dyn PeerConnection>) {
        *ctx.peer_store.write().await = None;
        graceful_shutdown("WHEP source", client, peer).await;
    }

    /// Wait until every media kind negotiated in the WHEP answer delivered
    /// its first RTP packet (making its payload type known), accepting a
    /// partially ready set after a grace period.
    async fn wait_for_codecs(
        ctx: &WhepClientContext,
        ct: &CancellationToken,
        codec_info: &Arc<Mutex<rtsp::CodecInfo>>,
        answer_sdp: &str,
        shutdown_rx: &mut oneshot::Receiver<()>,
    ) -> WaitOutcome {
        let expected = expected_media_kinds(answer_sdp);
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

        loop {
            tokio::select! {
                _ = &mut *shutdown_rx => {
                    info!("[{}] Shutdown requested", ctx.stream_id);
                    return AttemptEnd::Shutdown;
                }
                _ = ct.cancelled() => {
                    return AttemptEnd::Failed("Connection closed".to_string());
                }
                result = video_rx.recv() => {
                    match result {
                        Some(data) => {
                            let _ = ctx.rtp_tx.send(MediaPacket::Rtp {
                                channel: 0,
                                data: data.into(),
                            });
                        }
                        None => {
                            return AttemptEnd::Failed("Video channel closed".to_string());
                        }
                    }
                }
                result = audio_rx.recv() => {
                    match result {
                        Some(data) => {
                            let _ = ctx.rtp_tx.send(MediaPacket::Rtp {
                                channel: audio_channel,
                                data: data.into(),
                            });
                        }
                        None => {
                            return AttemptEnd::Failed("Audio channel closed".to_string());
                        }
                    }
                }
            }
        }
    }

    async fn emit_state_change(
        state: &Arc<RwLock<StreamSourceState>>,
        state_tx: &broadcast::Sender<StateChangeEvent>,
        new_state: StreamSourceState,
        error: Option<String>,
    ) {
        let mut s = state.write().await;
        let old_state = *s;
        *s = new_state;

        let event = StateChangeEvent {
            old_state,
            new_state,
            error,
        };

        let _ = state_tx.send(event);
    }
}

/// Map a `whep://` / `wheps://` source URL to the `http(s)://` URL the WHEP
/// client POSTs to. A Bearer token can be carried as userinfo:
/// `whep://token@host:port/whep/stream`.
fn parse_whep_url(raw: &str) -> Result<(String, Option<String>)> {
    // Scheme replacement is done textually: `whep` is not a WHATWG "special"
    // scheme, so `Url::set_scheme` refuses the conversion to `http(s)`.
    let http_url = if let Some(rest) = raw.strip_prefix("whep://") {
        format!("http://{}", rest)
    } else if let Some(rest) = raw.strip_prefix("wheps://") {
        format!("https://{}", rest)
    } else {
        anyhow::bail!("Unsupported WHEP source URL: {}", raw);
    };

    let mut url = url::Url::parse(&http_url)?;
    if url.host_str().is_none() {
        anyhow::bail!("Invalid WHEP source URL (no host): {}", raw);
    }

    let token = (!url.username().is_empty()).then(|| url.username().to_string());
    if token.is_some() {
        url.set_username("")
            .map_err(|_| anyhow::anyhow!("Invalid WHEP source URL: {}", raw))?;
        url.set_password(None)
            .map_err(|_| anyhow::anyhow!("Invalid WHEP source URL: {}", raw))?;
    }

    Ok((url.to_string(), token))
}

/// Derive the media kinds negotiated in the WHEP answer SDP. A rejected
/// m-line (port 0) does not count as expected.
fn expected_media_kinds(answer_sdp: &str) -> (bool, bool) {
    let mut video = false;
    let mut audio = false;
    for line in answer_sdp.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("m=video") {
            video = media_line_active(rest);
        } else if let Some(rest) = line.strip_prefix("m=audio") {
            audio = media_line_active(rest);
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
        *self.state.blocking_read()
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
        if self.peer_store.read().await.is_none() {
            return None;
        }

        let (wrapper_tx, mut wrapper_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let peer_store = self.peer_store.clone();
        let stream_id = self.config.stream_id.clone();

        tokio::spawn(async move {
            // The peer is looked up per message so RTCP feedback keeps
            // flowing across WHEP reconnects.
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
    fn parse_whep_url_rejects_other_schemes() {
        assert!(parse_whep_url("rtsp://example.com/stream").is_err());
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
}
