use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::whipsynth::{Publisher, PublisherConfig};

/// Append a session index to the last path segment of a WHIP URL.
///
/// `http://localhost:7777/whip/live` with index `3` becomes
/// `http://localhost:7777/whip/live-3`.
fn session_whip_url(base_url: &str, index: usize) -> Result<String> {
    let mut url = url::Url::parse(base_url)
        .with_context(|| format!("Invalid WHIP URL: {base_url}"))?;

    let mut segments: Vec<String> = url
        .path_segments()
        .map(|it| it.map(|s| s.to_string()).collect())
        .unwrap_or_default();

    if let Some(last) = segments.last_mut() {
        *last = format!("{}-{}", last, index);
    } else {
        segments.push(format!("session-{}", index));
    }

    let new_path = segments.join("/");
    url.set_path(&new_path);
    Ok(url.to_string())
}

/// Configuration for a WHIP loadtest.
#[derive(Debug, Clone)]
pub struct LoadtestConfig {
    /// Base publisher configuration for each session.
    pub publisher_config: PublisherConfig,
    /// Number of concurrent WHIP publishers to spawn.
    pub session_count: usize,
    /// Delay between spawning each publisher.
    pub spawn_interval: Duration,
}

/// Aggregate statistics across all loadtest sessions.
#[derive(Debug, Clone, Default)]
pub struct LoadtestStats {
    pub sessions_total: usize,
    pub sessions_connected: usize,
    pub sessions_failed: usize,
    pub total_packets_sent: u64,
    pub total_bytes_sent: u64,
    pub total_nack_count: u64,
    pub total_pli_count: u64,
}

/// Run multiple WHIP publishers concurrently.
///
/// Each publisher gets its own rsmpeg encoder, PeerConnection, and WHIP
/// session. This is the simplest loadtest shape; a future optimization could
/// share a single encoded-bitstream source across sessions.
pub async fn run_loadtest(config: LoadtestConfig, ct: CancellationToken) -> Result<LoadtestStats> {
    let stats = Arc::new(Mutex::new(LoadtestStats::default()));

    let mut join_set = JoinSet::new();
    let mut spawned = 0usize;

    for i in 0..config.session_count {
        if ct.is_cancelled() {
            break;
        }

        let session_ct = ct.child_token();
        let mut session_config = config.publisher_config.clone();
        session_config.whip_url = session_whip_url(&session_config.whip_url, i)?;
        let session_stats = stats.clone();

        join_set.spawn(async move {
            let publisher = Publisher::new(session_config);
            match publisher.run(session_ct).await {
                Ok(ps) => {
                    if let Ok(mut s) = session_stats.lock() {
                        s.sessions_connected += 1;
                        s.total_packets_sent += ps.packets_sent;
                        s.total_bytes_sent += ps.bytes_sent;
                        s.total_nack_count += ps.nack_count;
                        s.total_pli_count += ps.pli_count;
                    }
                }
                Err(e) => {
                    warn!(session = i, error = ?e, "loadtest session failed");
                    if let Ok(mut s) = session_stats.lock() {
                        s.sessions_failed += 1;
                    }
                }
            }
        });
        spawned += 1;

        if i + 1 < config.session_count {
            tokio::select! {
                _ = ct.cancelled() => break,
                _ = tokio::time::sleep(config.spawn_interval) => {}
            }
        }
    }

    // Wait for all sessions to finish or cancellation.
    while let Some(result) = join_set.join_next().await {
        if let Err(e) = result {
            warn!(error = ?e, "loadtest session task panicked or was cancelled");
        }
    }

    let mut final_stats = stats.lock().map(|s| s.clone()).unwrap_or_default();
    final_stats.sessions_total = spawned;
    info!(
        sessions_total = final_stats.sessions_total,
        sessions_connected = final_stats.sessions_connected,
        sessions_failed = final_stats.sessions_failed,
        total_packets_sent = final_stats.total_packets_sent,
        total_bytes_sent = final_stats.total_bytes_sent,
        "Loadtest completed"
    );

    if final_stats.sessions_connected == 0 && spawned > 0 {
        return Err(anyhow::anyhow!(
            "all {} loadtest session(s) failed",
            spawned
        ));
    }

    Ok(final_stats)
}
