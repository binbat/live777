//! WHEP subscribe loadtest: N concurrent subscribers on one stream, exercising
//! the SFU's fan-out path. Each session drains the received WebRTC RTP tracks
//! internally and counts packets/bytes; it does not decode or forward media to
//! UDP, so the per-session overhead stays low.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, anyhow};
use libwish::Client;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

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

    let mut client = Client::new(
        params.whep_url.clone(),
        Client::get_auth_header_map(params.token.clone()),
    );

    let (peer, _answer, _stats, _dc_recv_rx, _dc_send_tx) = crate::whep::setup_whep_peer(
        ct.clone(),
        &mut client,
        video_tx,
        audio_tx,
        codec_info,
        None,
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

    ct.cancelled().await;
    graceful_shutdown("WHEP loadtest", &mut client, peer).await;
    let _ = tokio::join!(video_handle, audio_handle);

    let packets = packets.load(Ordering::Relaxed);
    let bytes = bytes.load(Ordering::Relaxed);
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
