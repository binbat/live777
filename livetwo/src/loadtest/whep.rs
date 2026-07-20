//! WHEP subscribe loadtest: N concurrent subscribers on one stream, exercising
//! the SFU's fan-out path. Each session drains the received WebRTC RTP tracks
//! internally and counts packets/bytes; it does not decode or forward media to
//! UDP, so the per-session overhead stays low.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, anyhow};
use libwish::Client;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::info;
use webrtc::peer_connection::RTCPeerConnectionState;

use super::{LoadtestConfig, LoadtestStats, SessionMetrics, run_sessions};
use crate::utils::shutdown::graceful_shutdown;

/// Parameters shared by all subscribe sessions.
#[derive(Debug, Clone)]
pub struct WhepLoadParams {
    /// WHEP endpoint of the published stream, e.g. `http://localhost:7777/whep/live`.
    /// A publisher must be running on that stream (e.g. `loadtest whip`).
    pub whep_url: String,
    pub token: Option<String>,
}

/// Run `config.session_count` WHEP subscribers against `params.whep_url`.
pub async fn run(
    config: &LoadtestConfig,
    params: WhepLoadParams,
    ct: CancellationToken,
) -> Result<LoadtestStats> {
    let session_ct = ct.child_token();
    run_sessions(config, session_ct.clone(), move |_| {
        let params = params.clone();
        let run_ct = session_ct.child_token();
        async move { run_one(params, run_ct).await }
    })
    .await
}

async fn run_one(params: WhepLoadParams, ct: CancellationToken) -> Result<SessionMetrics> {
    const MEDIA_CHANNEL_CAPACITY: usize = 512;

    let start = std::time::Instant::now();
    let packets = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));

    let (video_tx, video_rx) = mpsc::channel::<Vec<u8>>(MEDIA_CHANNEL_CAPACITY);
    let (audio_tx, audio_rx) = mpsc::channel::<Vec<u8>>(MEDIA_CHANNEL_CAPACITY);
    let codec_info = Arc::new(tokio::sync::Mutex::new(rtsp::CodecInfo::new()));
    let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);

    let mut client = Client::new(
        params.whep_url.clone(),
        Client::get_auth_header_map(params.token.clone()),
    );
    let peer_ct = CancellationToken::new();

    let (peer, _answer, _stats, _dc_recv_rx, _dc_send_tx) = crate::whep::setup_whep_peer(
        peer_ct.clone(),
        &mut client,
        video_tx,
        audio_tx,
        codec_info,
        Some(state_tx),
        None,
    )
    .await?;

    let video_handle = tokio::spawn(drain_media(
        "video",
        video_rx,
        packets.clone(),
        bytes.clone(),
    ));
    let audio_handle = tokio::spawn(drain_media(
        "audio",
        audio_rx,
        packets.clone(),
        bytes.clone(),
    ));

    let stop_reason = wait_for_stop(ct.clone(), peer_ct.clone(), state_rx).await;
    peer_ct.cancel();
    graceful_shutdown("WHEP loadtest", &mut client, peer).await;
    let _ = tokio::join!(video_handle, audio_handle);

    let packets = packets.load(Ordering::Relaxed);
    let bytes = bytes.load(Ordering::Relaxed);
    if let StopReason::PeerEnded(state) = stop_reason {
        return Err(anyhow!(
            "WHEP subscriber peer ended unexpectedly: state={state}, packets={packets}, bytes={bytes}, url={}",
            params.whep_url
        ));
    }
    if packets == 0 {
        return Err(anyhow!(
            "WHEP subscriber received no media packets from {}",
            params.whep_url
        ));
    }

    Ok(SessionMetrics {
        packets,
        bytes,
        connected_duration: start.elapsed(),
        ..Default::default()
    })
}

async fn drain_media(
    kind: &'static str,
    mut rx: mpsc::Receiver<Vec<u8>>,
    packets: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
) {
    while let Some(packet) = rx.recv().await {
        packets.fetch_add(1, Ordering::Relaxed);
        bytes.fetch_add(packet.len() as u64, Ordering::Relaxed);
    }

    info!(kind, "WHEP loadtest media drain stopped");
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
        tokio::select! {
            _ = ct.cancelled() => return StopReason::Requested,
            _ = peer_ct.cancelled() => {
                return StopReason::PeerEnded(*state_rx.borrow());
            }
            changed = state_rx.changed() => {
                if changed.is_err() {
                    return StopReason::PeerEnded(*state_rx.borrow());
                }
                let state = *state_rx.borrow_and_update();
                if matches!(
                    state,
                    RTCPeerConnectionState::Failed
                        | RTCPeerConnectionState::Closed
                        | RTCPeerConnectionState::Disconnected
                ) {
                    return StopReason::PeerEnded(state);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
