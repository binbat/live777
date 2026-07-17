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
use tracing::{debug, info};
use webrtc::peer_connection::RTCPeerConnectionState;

use crate::probe::{ProbeBackend, ProbeConfig, ProbeResult};
use crate::utils::shutdown::graceful_shutdown;

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
            codec = ?config.codec,
            "Starting rsmpeg WHEP probe (FFI encoded-frame path)"
        );

        let ct = CancellationToken::new();
        let (state_tx, mut state_rx) = tokio::sync::watch::channel(RTCPeerConnectionState::New);
        // Use a bounded channel so a slow decoder cannot cause unbounded memory
        // growth. Backpressure will propagate to the WebRTC track reader.
        let (video_tx, mut video_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
        // The rsmpeg probe only decodes video; give audio a small bounded
        // channel and drain it so audio RTP does not accumulate in the WebRTC
        // stack and spam the logs with drop warnings.
        let (audio_tx, mut audio_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let codec_info = Arc::new(Mutex::new(rtsp::CodecInfo::new()));
        let (video_mime_tx, mut video_mime_rx) = tokio::sync::watch::channel(None::<String>);

        let audio_bytes_received = Arc::new(AtomicU64::new(0));
        let drain_ct = ct.clone();
        let drain_bytes = audio_bytes_received.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(packet) = audio_rx.recv() => {
                        drain_bytes.fetch_add(packet.len() as u64, Ordering::Relaxed);
                    }
                    _ = drain_ct.cancelled() => {
                        debug!("rsmpeg probe audio drain task cancelled");
                        break;
                    }
                }
            }
        });

        let mut client = Client::new(
            config.whep_url.clone(),
            Client::get_auth_header_map(config.token.clone()),
        );

        // Create the peer connection in the current task so that the returned
        // `Arc<dyn PeerConnection>` stays alive for the whole probe duration.
        let (peer, _answer, _stats, _dc_recv_rx, _dc_send_tx) = crate::whep::setup_whep_peer(
            ct.clone(),
            &mut client,
            video_tx,
            audio_tx,
            codec_info.clone(),
            Some(state_tx),
            Some(video_mime_tx),
        )
        .await?;

        let mut result = ProbeResult {
            success: false,
            connected: false,
            backend: self.name(),
            codec: config.codec.map(|c| c.as_str().to_string()),
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
                graceful_shutdown("WHEP", &mut client, peer).await;
                result.duration_ms = start.elapsed().as_millis() as u64;
                return Ok(result);
            }
        };
        info!("WHEP peer connected, video mime type: {mime_type}");

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        // Keep a small margin for the decoder thread to shut down cleanly.
        let decode_duration = self
            .decode_duration
            .min(Duration::from_secs(10))
            .min(remaining.saturating_sub(Duration::from_secs(2)));
        if decode_duration.is_zero() {
            ct.cancel();
            result.error = Some("probe timeout expired before decoding could start".to_string());
            graceful_shutdown("WHEP", &mut client, peer).await;
            result.duration_ms = start.elapsed().as_millis() as u64;
            return Ok(result);
        }

        let sprop_params = config.sprop_params.clone();
        let (packet_tx, packet_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_clone = cancelled.clone();

        // Forward RTP packets from the async WebRTC reader to the blocking
        // FFmpeg decoder thread. Stop when cancelled or when the decode window
        // has elapsed so the probe cannot outlive its timeout.
        //
        // Note: if packet_tx.send(packet) blocks because the decoder thread is
        // not consuming, the sleep branch will not fire. The decoder thread uses
        // a short recv_timeout loop and will drop packet_rx on its own timeout,
        // which unblocks the forwarder and lets it exit.
        let forward_ct = ct.clone();
        let video_bytes_received = Arc::new(AtomicU64::new(0));
        let forward_bytes = video_bytes_received.clone();
        let forward_handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some(packet) = video_rx.recv() => {
                        forward_bytes.fetch_add(packet.len() as u64, Ordering::Relaxed);
                        if packet_tx.send(packet).is_err() {
                            break;
                        }
                    }
                    _ = forward_ct.cancelled() => break,
                    _ = tokio::time::sleep(decode_duration) => break,
                }
            }
        });

        let decoder_timeout = deadline.saturating_duration_since(tokio::time::Instant::now());
        let decoder_handle = tokio::task::spawn_blocking(move || {
            crate::probe::decoder::run_ffi_decoder(
                mime_type,
                sprop_params.as_deref(),
                packet_rx,
                cancelled_clone,
                decode_duration + Duration::from_secs(2),
            )
        });

        let abort_handle = decoder_handle.abort_handle();
        let decode_result = {
            let decoder = decoder_handle;
            let abort = abort_handle;
            tokio::select! {
                res = decoder => Some(res),
                _ = tokio::time::sleep(decoder_timeout) => {
                    // Abort the blocking task so the decoder thread does
                    // not keep consuming CPU after the probe has timed out.
                    abort.abort();
                    None
                }
            }
        };

        cancelled.store(true, Ordering::Relaxed);
        let _ = forward_handle.await;

        match decode_result {
            Some(Ok(Ok((width, height, frame_count)))) => {
                result.width = width;
                result.height = height;
                result.frame_count = frame_count;
                result.video_tracks = if frame_count > 0 { 1 } else { 0 };
                result.success = frame_count > 0 && width > 0 && height > 0;
            }
            Some(Ok(Err(e))) => {
                result.error = Some(format!("decoder error: {e:?}"));
            }
            Some(Err(e)) => {
                result.error = Some(format!("decode task panicked: {e:?}"));
            }
            None => {
                result.error = Some("decoder timed out".to_string());
            }
        }

        result.video_bytes_received = video_bytes_received.load(Ordering::Relaxed);
        result.audio_bytes_received = audio_bytes_received.load(Ordering::Relaxed);

        ct.cancel();
        graceful_shutdown("WHEP", &mut client, peer).await;
        result.duration_ms = start.elapsed().as_millis() as u64;
        Ok(result)
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
