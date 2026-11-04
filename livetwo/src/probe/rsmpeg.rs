use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use libwish::Client;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;
use webrtc::peer_connection::RTCPeerConnectionState;

use crate::probe::decoder::DecodedAudio;
use crate::probe::worker::{DecodeSummary, DecodeWorker, DecodeWorkerConfig, VideoDecodeResult};
use crate::probe::{ProbeBackend, ProbeConfig, ProbeResult};
use crate::utils::shutdown::graceful_shutdown;

/// WHEP probe backend that receives RTP payloads directly from the WebRTC
/// peer connection and sends them to isolated whepprobe decode workers. The
/// workers reuse the rsmpeg/FFmpeg RTP frame decoder; no RTP/UDP bridge or
/// in-process FFmpeg decoding is involved.
#[derive(Debug, Clone, Copy)]
pub struct RsmpegProbe {
    /// How long to decode after the subscribe session becomes connected.
    pub decode_duration: Duration,
}

impl Default for RsmpegProbe {
    fn default() -> Self {
        Self {
            decode_duration: Duration::from_secs(5),
        }
    }
}

#[async_trait]
impl ProbeBackend for RsmpegProbe {
    fn name(&self) -> &'static str {
        "rsmpeg"
    }

    async fn probe(&self, config: &ProbeConfig) -> Result<ProbeResult> {
        let start = tokio::time::Instant::now();

        info!(
            whep_url = %config.whep_url,
            codec = ?config.video_codec,
            "Starting rsmpeg WHEP probe (decode-worker path)"
        );

        let ct = CancellationToken::new();
        let (state_tx, mut state_rx) = tokio::sync::watch::channel(RTCPeerConnectionState::New);
        // Bound the WebRTC track readers independently from the worker pipes so
        // slow decoding cannot cause unbounded in-process memory growth.
        let (video_tx, video_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
        let (audio_tx, audio_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
        let codec_info = Arc::new(Mutex::new(rtsp::CodecInfo::new()));
        let (video_mime_tx, mut video_mime_rx) = tokio::sync::watch::channel(None::<String>);
        let audio_bytes_received = Arc::new(AtomicU64::new(0));

        let mut client = Client::new(
            config.whep_url.clone(),
            Client::get_auth_header_map(config.token.clone())?,
        );

        // Create the peer connection in the current task so that the returned
        // `Arc<dyn PeerConnection>` stays alive for the whole probe duration.
        let setup = crate::whep::setup_whep_peer(
            ct.clone(),
            &mut client,
            video_tx,
            audio_tx,
            codec_info.clone(),
            crate::whep::WhepPeerOptions {
                ice_servers: config.ice_servers.clone(),
                setup_timeout: Some(config.timeout),
                ..Default::default()
            },
            Some(state_tx),
            Some(video_mime_tx),
        )
        .await;
        let (peer, _answer, _stats, _dc_recv_rx, _dc_send_tx) = match setup {
            Ok(parts) => parts,
            Err(e) => {
                ct.cancel();
                return Err(e);
            }
        };

        let mut result = ProbeResult {
            success: false,
            connected: false,
            backend: self.name(),
            // Filled in with the negotiated codec once the track reports it;
            // the configured expectation must not leak into failure results.
            video_codec: None,
            audio_codec: None,
            audio_frame_count: 0,
            audio_sample_rate: 0,
            audio_channels: 0,
            width: 0,
            height: 0,
            frame_count: 0,
            duration_ms: 0,
            video_tracks: 0,
            audio_tracks: 0,
            video_bytes_received: 0,
            audio_bytes_received: 0,
            error: None,
        };

        // The overall probe must respect config.timeout across all phases:
        // connection, codec detection, decoding, and worker collection.
        let deadline = start + config.timeout;

        let connected = wait_for_state(
            &mut state_rx,
            RTCPeerConnectionState::Connected,
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await?;

        result.connected = connected;
        result.duration_ms = start.elapsed().as_millis() as u64;

        if !connected {
            ct.cancel();
            result.error = Some("WHEP peer connection did not reach Connected".to_string());
            result.audio_bytes_received = audio_bytes_received.load(Ordering::Relaxed);
            result.audio_tracks = u32::from(result.audio_bytes_received > 0);
            result.audio_codec = negotiated_audio_codec(&codec_info).await;
            graceful_shutdown("WHEP", &mut client, peer).await;
            return Ok(result);
        }

        // Wait briefly for the track to report its negotiated codec. Use the
        // remaining probe budget, capped at 5 s so a missing track fails fast.
        let mime_type = match wait_for_video_mime(
            &mut video_mime_rx,
            Duration::from_secs(5)
                .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
        )
        .await
        {
            Ok(mime) => mime,
            Err(e) => {
                ct.cancel();
                result.error = Some(format!("timed out waiting for video codec: {e}"));
                result.audio_bytes_received = audio_bytes_received.load(Ordering::Relaxed);
                result.audio_tracks = u32::from(result.audio_bytes_received > 0);
                result.audio_codec = negotiated_audio_codec(&codec_info).await;
                graceful_shutdown("WHEP", &mut client, peer).await;
                result.duration_ms = start.elapsed().as_millis() as u64;
                return Ok(result);
            }
        };
        info!("WHEP peer connected, video mime type: {mime_type}");
        // Report the negotiated codec, not the configured expectation —
        // otherwise the runner's codec assertion compares the config to itself.
        // Strip the "video/" prefix so it matches ffprobe-style codec names.
        result.video_codec = Some(
            mime_type
                .rsplit('/')
                .next()
                .unwrap_or(&mime_type)
                .to_ascii_lowercase(),
        );

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        // Keep a small margin for normal EOF, decoder flush, and child reaping.
        // The caller's decode window is only bounded by the remaining probe
        // budget; a hard cap here silently truncated `--decode-duration`.
        let decode_duration = self
            .decode_duration
            .min(remaining.saturating_sub(Duration::from_secs(2)));
        if decode_duration.is_zero() {
            ct.cancel();
            result.error = Some("probe timeout expired before decoding could start".to_string());
            result.audio_bytes_received = audio_bytes_received.load(Ordering::Relaxed);
            result.audio_tracks = u32::from(result.audio_bytes_received > 0);
            result.audio_codec = negotiated_audio_codec(&codec_info).await;
            graceful_shutdown("WHEP", &mut client, peer).await;
            result.duration_ms = start.elapsed().as_millis() as u64;
            return Ok(result);
        }

        let video_bytes_received = Arc::new(AtomicU64::new(0));
        // These futures are joined in-place rather than spawned. Dropping the
        // probe therefore drops both workers and synchronously reaps them.
        let video_decode = run_video_decoder(
            mime_type,
            config.sprop_params.clone(),
            video_rx,
            video_bytes_received.clone(),
            ct.clone(),
            decode_duration,
            deadline,
        );
        let audio_decode = run_audio_decoder(
            audio_rx,
            codec_info.clone(),
            audio_bytes_received.clone(),
            ct.clone(),
            decode_duration,
            deadline,
        );
        let (decode_result, audio_decode_result) = tokio::join!(video_decode, audio_decode);
        ct.cancel();

        let mut video_success = false;
        match decode_result {
            Ok(decoded) => {
                result.width = decoded.width;
                result.height = decoded.height;
                result.frame_count = decoded.frame_count;
                result.video_tracks = u32::from(decoded.frame_count > 0);
                video_success = decoded.frame_count > 0 && decoded.width > 0 && decoded.height > 0;
            }
            Err(e) => {
                result.error = Some(format!("video decoder error: {e:?}"));
            }
        }

        result.video_bytes_received = video_bytes_received.load(Ordering::Relaxed);
        result.audio_bytes_received = audio_bytes_received.load(Ordering::Relaxed);
        result.audio_tracks = u32::from(result.audio_bytes_received > 0);
        result.audio_codec = negotiated_audio_codec(&codec_info).await;

        match audio_decode_result {
            Some(Ok(decoded)) => {
                result.audio_frame_count = decoded.frame_count;
                result.audio_sample_rate = decoded.sample_rate;
                result.audio_channels = decoded.channels;
            }
            Some(Err(e)) => {
                result.error = Some(format!("audio decoder error: {e:?}"));
            }
            None => {}
        }
        let audio_success = result.audio_bytes_received == 0 || result.audio_frame_count > 0;
        if result.audio_bytes_received > 0
            && result.audio_frame_count == 0
            && result.error.is_none()
        {
            result.error = Some("audio decoder produced no frames".to_string());
        }
        result.success = video_success && audio_success && result.error.is_none();

        ct.cancel();
        graceful_shutdown("WHEP", &mut client, peer).await;
        result.duration_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }
}

async fn run_video_decoder(
    mime_type: String,
    sprop_params: Option<String>,
    video_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    bytes_received: Arc<AtomicU64>,
    ct: CancellationToken,
    decode_duration: Duration,
    hard_deadline: tokio::time::Instant,
) -> Result<VideoDecodeResult> {
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
    let forward = forward_packets(
        video_rx,
        packet_tx,
        bytes_received,
        ct.clone(),
        decode_duration,
    );
    let decode = async {
        let mut worker = DecodeWorker::start(
            DecodeWorkerConfig {
                mime_type,
                sprop_params,
            },
            &ct,
            hard_deadline,
        )
        .await?;
        while let Some(packet) = packet_rx.recv().await {
            worker.send_rtp(&packet, &ct, hard_deadline).await?;
        }
        match worker.finish(&ct, hard_deadline).await? {
            DecodeSummary::Video(decoded) => Ok(decoded),
            DecodeSummary::Audio(_) => bail!("video decode worker returned an audio result"),
        }
    };
    let (_, decoded) = tokio::join!(forward, decode);
    decoded
}

async fn run_audio_decoder(
    audio_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    codec_info: Arc<Mutex<rtsp::CodecInfo>>,
    bytes_received: Arc<AtomicU64>,
    ct: CancellationToken,
    decode_duration: Duration,
    hard_deadline: tokio::time::Instant,
) -> Option<Result<DecodedAudio>> {
    let (packet_tx, packet_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
    let forward = forward_packets(
        audio_rx,
        packet_tx,
        bytes_received,
        ct.clone(),
        decode_duration,
    );
    let (_, decoded) = tokio::join!(
        forward,
        decode_audio_packets(packet_rx, codec_info, ct, hard_deadline)
    );
    decoded
}

/// Preserve the original bounded packet queue ahead of FFmpeg. The forward and
/// decode futures are joined in-place, so this extra buffering does not detach
/// either side when the outer probe is dropped.
async fn forward_packets(
    mut source_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    bytes_received: Arc<AtomicU64>,
    ct: CancellationToken,
    decode_duration: Duration,
) {
    let deadline = tokio::time::sleep(decode_duration);
    tokio::pin!(deadline);
    'forward: loop {
        tokio::select! {
            packet = source_rx.recv() => {
                let Some(packet) = packet else { break };
                bytes_received.fetch_add(packet.len() as u64, Ordering::Relaxed);
                tokio::select! {
                    result = packet_tx.send(packet) => {
                        if result.is_err() {
                            break 'forward;
                        }
                    }
                    _ = ct.cancelled() => break 'forward,
                    _ = &mut deadline => break 'forward,
                }
            }
            _ = ct.cancelled() => break,
            _ = &mut deadline => break,
        }
    }
}

async fn decode_audio_packets(
    mut packet_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    codec_info: Arc<Mutex<rtsp::CodecInfo>>,
    ct: CancellationToken,
    hard_deadline: tokio::time::Instant,
) -> Option<Result<DecodedAudio>> {
    let mut worker = None;
    let result = async {
        while let Some(packet) = packet_rx.recv().await {
            if worker.is_none() {
                let Some(mime_type) = negotiated_audio_mime(&codec_info).await else {
                    return Err(anyhow!("audio RTP arrived without codec information"));
                };
                worker = Some(
                    DecodeWorker::start(
                        DecodeWorkerConfig {
                            mime_type,
                            sprop_params: None,
                        },
                        &ct,
                        hard_deadline,
                    )
                    .await?,
                );
            }
            worker
                .as_mut()
                .expect("audio worker was just initialized")
                .send_rtp(&packet, &ct, hard_deadline)
                .await?;
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        return Some(Err(error));
    }

    match worker {
        Some(worker) => Some(
            async {
                match worker.finish(&ct, hard_deadline).await? {
                    DecodeSummary::Audio(decoded) => Ok(decoded),
                    DecodeSummary::Video(_) => bail!("audio decode worker returned a video result"),
                }
            }
            .await,
        ),
        None => None,
    }
}

/// Wait until the peer connection reaches `target_state` or the timeout expires.
async fn wait_for_state(
    state_rx: &mut tokio::sync::watch::Receiver<RTCPeerConnectionState>,
    target_state: RTCPeerConnectionState,
    timeout: Duration,
) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if *state_rx.borrow() == target_state {
            return Ok(true);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        match tokio::time::timeout(remaining, state_rx.changed()).await {
            Ok(Ok(())) => {
                let state = *state_rx.borrow();
                if state == target_state {
                    return Ok(true);
                }
                if state == RTCPeerConnectionState::Failed
                    || state == RTCPeerConnectionState::Closed
                {
                    return Ok(false);
                }
            }
            Ok(Err(_)) => return Ok(false),
            Err(_) => return Ok(false),
        }
    }
}

/// Wait until the WHEP track reports its negotiated video codec mime type or
/// the timeout expires.
async fn wait_for_video_mime(
    video_mime_rx: &mut tokio::sync::watch::Receiver<Option<String>>,
    timeout: Duration,
) -> Result<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(ref mime) = *video_mime_rx.borrow() {
            return Ok(mime.clone());
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(anyhow!("Timed out waiting for video codec info"));
        }
        tokio::time::timeout(remaining, video_mime_rx.changed())
            .await
            .map_err(|_| anyhow!("Timed out waiting for video codec info"))?
            .map_err(|_| anyhow!("Video codec watch channel closed"))?;
    }
}

async fn negotiated_audio_mime(codec_info: &Arc<Mutex<rtsp::CodecInfo>>) -> Option<String> {
    codec_info
        .lock()
        .await
        .audio_codec
        .as_ref()
        .map(|codec| codec.rtp_codec.mime_type.clone())
}

/// Read the audio codec negotiated for the WHEP session from the shared codec
/// info, which the track reader populates once the first audio RTP packet
/// arrives. Returns the bare codec name, e.g. `opus`.
async fn negotiated_audio_codec(codec_info: &Arc<Mutex<rtsp::CodecInfo>>) -> Option<String> {
    negotiated_audio_mime(codec_info).await.map(|mime_type| {
        mime_type
            .rsplit('/')
            .next()
            .unwrap_or(&mime_type)
            .to_ascii_lowercase()
    })
}
