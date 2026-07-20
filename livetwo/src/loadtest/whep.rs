//! WHEP subscribe loadtest: N concurrent subscribers on one stream, exercising
//! the SFU's fan-out path. Each session drains the received WebRTC RTP tracks
//! internally and counts packets/bytes; it does not decode or forward media to
//! UDP, so the per-session overhead stays low.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use libwish::Client;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::info;
use webrtc::peer_connection::RTCPeerConnectionState;

use super::{LoadtestConfig, LoadtestStats, SessionMetrics, run_sessions};
use crate::utils::shutdown::graceful_shutdown;

/// Grace period for a transient `Disconnected` state: ICE connectivity loss
/// may recover on its own, so only a disconnection that outlives the grace
/// period (or a `Failed`/`Closed` state) ends the session.
const DISCONNECTED_GRACE: Duration = Duration::from_secs(5);

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

async fn run_one(params: WhepLoadParams, ct: CancellationToken) -> (SessionMetrics, Result<()>) {
    let start = std::time::Instant::now();
    let packets = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));

    let result = run_session(&params, ct, packets.clone(), bytes.clone()).await;

    // Metrics are returned even on failure so traffic from sessions that die
    // mid-run still counts towards the aggregate.
    let metrics = SessionMetrics {
        packets: packets.load(Ordering::Relaxed),
        bytes: bytes.load(Ordering::Relaxed),
        connected_duration: start.elapsed(),
        ..Default::default()
    };
    (metrics, result)
}

async fn run_session(
    params: &WhepLoadParams,
    ct: CancellationToken,
    packets: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
) -> Result<()> {
    const MEDIA_CHANNEL_CAPACITY: usize = 512;

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

    Ok(())
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
