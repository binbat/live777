use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use crate::whipsynth::PublisherConfig;

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
///
/// Each field is an [`AtomicU64`] so concurrent sessions can update counters
/// without taking a lock.
#[derive(Debug, Default)]
pub struct LoadtestStats {
    pub sessions_connected: AtomicU64,
    pub sessions_failed: AtomicU64,
    pub total_packets_sent: AtomicU64,
    pub total_bytes_sent: AtomicU64,
    pub total_nack_count: AtomicU64,
    pub total_pli_count: AtomicU64,
}

/// Snapshot of [`LoadtestStats`] for reporting.
#[derive(Debug, Clone, Default)]
pub struct LoadtestStatsSnapshot {
    pub sessions_total: usize,
    pub sessions_connected: usize,
    pub sessions_failed: usize,
    pub total_packets_sent: u64,
    pub total_bytes_sent: u64,
    pub total_nack_count: u64,
    pub total_pli_count: u64,
}

impl LoadtestStats {
    fn snapshot(&self) -> LoadtestStatsSnapshot {
        LoadtestStatsSnapshot {
            sessions_total: 0, // filled in by caller
            sessions_connected: self.sessions_connected.load(Ordering::Relaxed) as usize,
            sessions_failed: self.sessions_failed.load(Ordering::Relaxed) as usize,
            total_packets_sent: self.total_packets_sent.load(Ordering::Relaxed),
            total_bytes_sent: self.total_bytes_sent.load(Ordering::Relaxed),
            total_nack_count: self.total_nack_count.load(Ordering::Relaxed),
            total_pli_count: self.total_pli_count.load(Ordering::Relaxed),
        }
    }
}

/// Run multiple WHIP publishers concurrently.
///
/// Each publisher gets its own rsmpeg encoder, PeerConnection, and WHIP
/// session. This is the simplest loadtest shape; a future optimization could
/// share a single encoded-bitstream source across sessions.
pub async fn run_loadtest(
    config: LoadtestConfig,
    ct: CancellationToken,
) -> Result<LoadtestStatsSnapshot> {
    let stats = crate::loadtest::whip::run(
        &crate::loadtest::LoadtestConfig {
            session_count: config.session_count,
            spawn_interval: config.spawn_interval,
            duration: None,
        },
        config.publisher_config,
        ct,
    )
    .await?;

    let atomic_stats = LoadtestStats {
        sessions_connected: AtomicU64::new(stats.sessions_connected),
        sessions_failed: AtomicU64::new(stats.sessions_failed),
        total_packets_sent: AtomicU64::new(stats.total_packets),
        total_bytes_sent: AtomicU64::new(stats.total_bytes),
        total_nack_count: AtomicU64::new(stats.total_nack_count),
        total_pli_count: AtomicU64::new(stats.total_pli_count),
    };

    let mut snapshot = atomic_stats.snapshot();
    snapshot.sessions_total = stats.sessions_total;
    Ok(snapshot)
}
