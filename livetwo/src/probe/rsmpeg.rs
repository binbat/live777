use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use libwish::Client;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;
use webrtc::peer_connection::RTCPeerConnectionState;

use crate::probe::{ProbeBackend, ProbeConfig, ProbeResult};
use crate::utils::shutdown::graceful_shutdown;

const DECODER_JOIN_TIMEOUT: Duration = Duration::from_secs(2);

/// WHEP probe backend that receives RTP payloads directly from the WebRTC
/// peer connection, depacketizes them into encoded frames, and decodes them
/// with rsmpeg/FFmpeg through FFI.
///
/// Unlike the previous implementation this avoids an intermediate RTP/UDP
/// bridge: encoded frames travel from the Rust WebRTC stack to FFmpeg in the
/// same process.
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
            "Starting rsmpeg WHEP probe (FFI encoded-frame path)"
        );

        let ct = CancellationToken::new();
        let (state_tx, mut state_rx) = tokio::sync::watch::channel(RTCPeerConnectionState::New);
        // Use bounded channels so slow decoders cannot cause unbounded memory
        // growth in the WebRTC track readers.
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
        let (peer, _answer, _stats, _dc_recv_rx, _dc_send_tx) = match crate::whep::setup_whep_peer(
            ct.clone(),
            &mut client,
            video_tx,
            audio_tx,
            codec_info.clone(),
            crate::whep::WhepPeerOptions {
                ice_servers: config.ice_servers.clone(),
                ..Default::default()
            },
            Some(state_tx),
            Some(video_mime_tx),
        )
        .await
        {
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
        // connection, codec detection, and decoding.
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
        // Keep a small margin for the decoder thread to shut down cleanly.
        let decode_duration = self
            .decode_duration
            .min(Duration::from_secs(10))
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

        let sprop_params = config.sprop_params.clone();
        let (packet_tx, packet_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_clone = cancelled.clone();

        let video_bytes_received = Arc::new(AtomicU64::new(0));
        let forward_handle = tokio::spawn(forward_video_packets(
            video_rx,
            packet_tx,
            video_bytes_received.clone(),
            ct.clone(),
            decode_duration,
        ));

        let audio_handle = tokio::spawn(run_audio_decoder(
            audio_rx,
            codec_info.clone(),
            audio_bytes_received.clone(),
            cancelled.clone(),
            ct.clone(),
            decode_duration,
        ));

        let decoder_timeout = deadline.saturating_duration_since(tokio::time::Instant::now());
        let decoder_handle = tokio::task::spawn_blocking(move || {
            crate::probe::decoder::run_ffi_tokio_decoder(
                mime_type,
                sprop_params.as_deref(),
                packet_rx,
                cancelled_clone,
                decode_duration + DECODER_JOIN_TIMEOUT,
            )
        });
        let decode_result = join_task(decoder_handle, decoder_timeout).await;

        cancelled.store(true, Ordering::Relaxed);
        ct.cancel();
        match join_task(forward_handle, DECODER_JOIN_TIMEOUT).await {
            Some(Ok(())) => {}
            Some(Err(e)) => result.error = Some(format!("video forward task panicked: {e:?}")),
            None => result.error = Some("video forward task timed out".to_string()),
        }
        let audio_decode_result = join_task(audio_handle, DECODER_JOIN_TIMEOUT).await;

        let mut video_success = false;
        match decode_result {
            Some(Ok(Ok((width, height, frame_count)))) => {
                result.width = width;
                result.height = height;
                result.frame_count = frame_count;
                result.video_tracks = if frame_count > 0 { 1 } else { 0 };
                video_success = frame_count > 0 && width > 0 && height > 0;
            }
            Some(Ok(Err(e))) => {
                result.error = Some(format!("video decoder error: {e:?}"));
            }
            Some(Err(e)) => {
                result.error = Some(format!("video decode task panicked: {e:?}"));
            }
            None => {
                result.error = Some("video decoder timed out".to_string());
            }
        }

        result.video_bytes_received = video_bytes_received.load(Ordering::Relaxed);
        result.audio_bytes_received = audio_bytes_received.load(Ordering::Relaxed);
        result.audio_tracks = u32::from(result.audio_bytes_received > 0);
        result.audio_codec = negotiated_audio_codec(&codec_info).await;

        match audio_decode_result {
            Some(Ok(Some(Ok(decoded)))) => {
                result.audio_frame_count = decoded.frame_count;
                result.audio_sample_rate = decoded.sample_rate;
                result.audio_channels = decoded.channels;
            }
            Some(Ok(Some(Err(e)))) => {
                result.error = Some(format!("audio decoder error: {e:?}"));
            }
            Some(Ok(None)) => {}
            Some(Err(e)) => {
                result.error = Some(format!("audio decode task panicked: {e:?}"));
            }
            None => result.error = Some("audio decode task timed out".to_string()),
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

async fn forward_video_packets(
    mut video_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    bytes_received: Arc<AtomicU64>,
    ct: CancellationToken,
    decode_duration: Duration,
) {
    let deadline = tokio::time::sleep(decode_duration);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            packet = video_rx.recv() => {
                let Some(packet) = packet else {
                    break;
                };
                bytes_received.fetch_add(packet.len() as u64, Ordering::Relaxed);
                tokio::select! {
                    result = packet_tx.send(packet) => {
                        if result.is_err() {
                            break;
                        }
                    }
                    _ = ct.cancelled() => break,
                    _ = &mut deadline => break,
                }
            }
            _ = ct.cancelled() => break,
            _ = &mut deadline => break,
        }
    }
}

async fn run_audio_decoder(
    mut audio_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    codec_info: Arc<Mutex<rtsp::CodecInfo>>,
    bytes_received: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
    ct: CancellationToken,
    decode_duration: Duration,
) -> Option<Result<crate::probe::decoder::DecodedAudio>> {
    let (packet_tx, packet_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
    let mut packet_rx = Some(packet_rx);
    let mut decoder_handle = None;
    let deadline = tokio::time::sleep(decode_duration);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            packet = audio_rx.recv() => {
                let Some(packet) = packet else {
                    break;
                };
                bytes_received.fetch_add(packet.len() as u64, Ordering::Relaxed);
                if decoder_handle.is_none() {
                    let Some(mime_type) = negotiated_audio_mime(&codec_info).await else {
                        return Some(Err(anyhow!("audio RTP arrived without codec information")));
                    };
                    let cancelled = cancelled.clone();
                    let packet_rx = packet_rx.take().expect("audio receiver is taken once");
                    decoder_handle = Some(tokio::task::spawn_blocking(move || {
                        crate::probe::decoder::run_ffi_audio_decoder(
                            mime_type,
                            packet_rx,
                            cancelled,
                            decode_duration + DECODER_JOIN_TIMEOUT,
                        )
                    }));
                }
                tokio::select! {
                    result = packet_tx.send(packet) => {
                        if result.is_err() {
                            break;
                        }
                    }
                    _ = ct.cancelled() => break,
                    _ = &mut deadline => break,
                }
            }
            _ = ct.cancelled() => break,
            _ = &mut deadline => break,
        }
    }

    drop(packet_tx);
    match decoder_handle {
        Some(handle) => match join_task(handle, DECODER_JOIN_TIMEOUT).await {
            Some(Ok(result)) => Some(result),
            Some(Err(e)) => Some(Err(anyhow!("audio decoder task failed: {e}"))),
            None => Some(Err(anyhow!("audio decoder timed out"))),
        },
        None => None,
    }
}

async fn join_task<T>(
    task: tokio::task::JoinHandle<T>,
    timeout: Duration,
) -> Option<Result<T, tokio::task::JoinError>> {
    let abort = task.abort_handle();
    match tokio::time::timeout(timeout, task).await {
        Ok(result) => Some(result),
        Err(_) => {
            abort.abort();
            None
        }
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use super::{forward_video_packets, join_task};

    type BlockedForward = (
        tokio::sync::mpsc::Sender<Vec<u8>>,
        tokio::sync::mpsc::Sender<Vec<u8>>,
        tokio::sync::mpsc::Receiver<Vec<u8>>,
        tokio::task::JoinHandle<()>,
    );

    fn blocked_forward(ct: CancellationToken, decode_duration: Duration) -> BlockedForward {
        let (source_tx, source_rx) = tokio::sync::mpsc::channel(1);
        let (decoder_tx, decoder_rx) = tokio::sync::mpsc::channel(1);
        decoder_tx.try_send(vec![0]).unwrap();
        let forward = tokio::spawn(forward_video_packets(
            source_rx,
            decoder_tx.clone(),
            Arc::new(AtomicU64::new(0)),
            ct,
            decode_duration,
        ));
        (source_tx, decoder_tx, decoder_rx, forward)
    }

    #[tokio::test]
    async fn cancel_interrupts_forward_when_decoder_stops_consuming() {
        let ct = CancellationToken::new();
        let (source_tx, _decoder_tx, _decoder_rx, forward) =
            blocked_forward(ct.clone(), Duration::from_secs(30));
        source_tx.send(vec![1]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!forward.is_finished());

        ct.cancel();
        let result = join_task(forward, Duration::from_millis(200)).await;
        assert!(matches!(result, Some(Ok(()))));
    }

    #[tokio::test]
    async fn deadline_interrupts_forward_when_decoder_stops_consuming() {
        let ct = CancellationToken::new();
        let (source_tx, _decoder_tx, _decoder_rx, forward) =
            blocked_forward(ct, Duration::from_millis(20));
        source_tx.send(vec![1]).await.unwrap();
        let result = join_task(forward, Duration::from_millis(200)).await;
        assert!(matches!(result, Some(Ok(()))));
    }

    #[tokio::test]
    async fn dropping_decoder_receiver_interrupts_forward() {
        let ct = CancellationToken::new();
        let (source_tx, _decoder_tx, decoder_rx, forward) =
            blocked_forward(ct, Duration::from_secs(30));
        source_tx.send(vec![1]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!forward.is_finished());

        drop(decoder_rx);
        let result = join_task(forward, Duration::from_millis(200)).await;
        assert!(matches!(result, Some(Ok(()))));
    }

    #[tokio::test]
    async fn slow_decoder_join_has_a_hard_timeout() {
        let (source_tx, source_rx) = tokio::sync::mpsc::channel(1);
        let (decoder_tx, mut decoder_rx) = tokio::sync::mpsc::channel(1);
        let decoding = Arc::new(AtomicBool::new(false));
        let decoding_clone = decoding.clone();
        let decoder = tokio::task::spawn_blocking(move || {
            decoder_rx.blocking_recv().unwrap();
            decoding_clone.store(true, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(250));
        });
        let ct = CancellationToken::new();
        let forward = tokio::spawn(forward_video_packets(
            source_rx,
            decoder_tx,
            Arc::new(AtomicU64::new(0)),
            ct.clone(),
            Duration::from_secs(30),
        ));
        source_tx.send(vec![1]).await.unwrap();
        for _ in 0..100 {
            if decoding.load(Ordering::Relaxed) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert!(decoding.load(Ordering::Relaxed));

        ct.cancel();
        let start = std::time::Instant::now();
        assert!(
            join_task(decoder, Duration::from_millis(20))
                .await
                .is_none()
        );
        assert!(start.elapsed() < Duration::from_millis(200));
        assert!(matches!(
            join_task(forward, Duration::from_millis(200)).await,
            Some(Ok(()))
        ));
    }
}
