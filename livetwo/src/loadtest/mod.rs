//! Generic load-test orchestration: spawn N protocol-conversion sessions with
//! a ramp-up interval, optionally bounded by a duration, and aggregate their
//! metrics. Used by the `loadtest` binary; not part of the test suite.

pub mod whep;
#[cfg(feature = "rsmpeg")]
pub mod whip;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Orchestration parameters for a loadtest run.
#[derive(Debug, Clone)]
pub struct LoadtestConfig {
    /// Number of concurrent sessions.
    pub session_count: usize,
    /// Delay between spawning each session (ramp-up).
    pub spawn_interval: Duration,
    /// Overall run duration; sessions are cancelled after it elapses.
    /// `None` runs until every session completes or the token is cancelled.
    pub duration: Option<Duration>,
}

/// Per-session metrics, kind-agnostic. `nack_count`/`pli_count` are only
/// meaningful on the publish (whip) side.
#[derive(Debug, Default, Clone)]
pub struct SessionMetrics {
    pub packets: u64,
    pub bytes: u64,
    pub errors: u64,
    pub nack_count: u64,
    pub pli_count: u64,
    pub connected_duration: Duration,
}

/// Aggregate result of a loadtest run.
#[derive(Debug, Default, Clone)]
pub struct LoadtestStats {
    pub sessions_total: usize,
    pub sessions_connected: u64,
    pub sessions_failed: u64,
    pub total_packets: u64,
    pub total_bytes: u64,
    pub total_errors: u64,
    pub total_nack_count: u64,
    pub total_pli_count: u64,
    pub total_connected_duration: Duration,
}

/// Spawn `config.session_count` sessions with a ramp-up interval between
/// spawns. When `config.duration` is set, the run is cancelled after it
/// elapses (sessions are expected to honor the cancellation token and return
/// their metrics); otherwise it runs until every session completes or the
/// token is cancelled.
pub async fn run_sessions<F, Fut>(
    config: &LoadtestConfig,
    ct: CancellationToken,
    make_session: F,
) -> Result<LoadtestStats>
where
    F: Fn(usize) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<SessionMetrics>> + Send + 'static,
{
    let connected = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let metrics = Arc::new(tokio::sync::Mutex::new(SessionMetrics::default()));

    let mut join_set = JoinSet::new();
    let mut spawned = 0usize;

    for i in 0..config.session_count {
        if ct.is_cancelled() {
            break;
        }

        let session_connected = connected.clone();
        let session_failed = failed.clone();
        let session_metrics = metrics.clone();
        let future = make_session(i);

        join_set.spawn(async move {
            match future.await {
                Ok(m) => {
                    session_connected.fetch_add(1, Ordering::Relaxed);
                    let mut stats = session_metrics.lock().await;
                    stats.packets += m.packets;
                    stats.bytes += m.bytes;
                    stats.errors += m.errors;
                    stats.nack_count += m.nack_count;
                    stats.pli_count += m.pli_count;
                    stats.connected_duration += m.connected_duration;
                }
                Err(e) => {
                    warn!(session = i, error = ?e, "loadtest session failed");
                    session_failed.fetch_add(1, Ordering::Relaxed);
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

    if let Some(duration) = config.duration {
        tokio::select! {
            _ = ct.cancelled() => {}
            _ = tokio::time::sleep(duration) => {
                info!(?duration, "loadtest duration reached, cancelling sessions");
                ct.cancel();
            }
        }
    }

    while let Some(result) = join_set.join_next().await {
        if let Err(e) = result {
            warn!(error = ?e, "loadtest session task panicked or was cancelled");
        }
    }

    // A single lock guard for the whole struct literal: temporaries of field
    // expressions live until the end of the statement, so per-field
    // `metrics.lock().await` would deadlock on the second lock.
    let m = metrics.lock().await;
    let stats = LoadtestStats {
        sessions_total: spawned,
        sessions_connected: connected.load(Ordering::Relaxed),
        sessions_failed: failed.load(Ordering::Relaxed),
        total_packets: m.packets,
        total_bytes: m.bytes,
        total_errors: m.errors,
        total_nack_count: m.nack_count,
        total_pli_count: m.pli_count,
        total_connected_duration: m.connected_duration,
    };
    drop(m);

    info!(
        sessions_total = stats.sessions_total,
        sessions_connected = stats.sessions_connected,
        sessions_failed = stats.sessions_failed,
        total_packets = stats.total_packets,
        total_bytes = stats.total_bytes,
        "Loadtest completed"
    );

    if stats.sessions_connected == 0 && spawned > 0 {
        anyhow::bail!("all {} loadtest session(s) failed", spawned);
    }

    Ok(stats)
}
