//! Generic load-test orchestration: spawn N protocol-conversion sessions with
//! a ramp-up interval, optionally bounded by a duration, and aggregate their
//! metrics. Used by the `livewrk` binary; not part of the test suite.

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

/// Grace period for sessions to drain after cancellation. Anything still
/// running afterwards is aborted so a stuck session cannot hang the run; an
/// aborted session's metrics and result are lost with the dropped future, so
/// it is only visible via `LoadtestStats::sessions_aborted`. The connect
/// phase honors the cancellation token, so the worst-case drain is the
/// session graceful-shutdown budget (~5 s).
const DRAIN_GRACE: Duration = Duration::from_secs(10);

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

/// How a session ended. `Connected` = established its connection and ran
/// until stopped; `Cancelled` = was cancelled before establishing a
/// connection (never became healthy, but nothing failed either).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOutcome {
    Connected,
    Cancelled,
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
    /// Sessions cancelled before they established a connection.
    pub sessions_cancelled: u64,
    /// Sessions still running after the drain grace; their tasks were aborted
    /// and their metrics are unrecoverable.
    pub sessions_aborted: u64,
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
///
/// Side effect: the passed-in token is cancelled by this function when
/// `config.duration` elapses; pass a child token if the caller's token must
/// stay usable afterwards.
///
/// The session future returns its metrics together with the final outcome
/// (`Err` = failed). Metrics are aggregated even when the outcome is an
/// error, so traffic from sessions that fail mid-run is not lost from the
/// totals; only `SessionOutcome::Connected` sessions contribute
/// `connected_duration`, so the per-connected-session average is not skewed.
pub async fn run_sessions<F, Fut>(
    config: &LoadtestConfig,
    ct: CancellationToken,
    make_session: F,
) -> Result<LoadtestStats>
where
    F: Fn(usize) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = (SessionMetrics, Result<SessionOutcome>)> + Send + 'static,
{
    let connected = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let cancelled = Arc::new(AtomicU64::new(0));
    let metrics = Arc::new(tokio::sync::Mutex::new(SessionMetrics::default()));

    let mut join_set = JoinSet::new();
    let mut spawned = 0usize;
    let mut aborted = 0u64;
    // A pathologically large duration would overflow `Instant + Duration`
    // and panic; fall back to no deadline (run until externally cancelled).
    let mut deadline = config.duration.and_then(|d| {
        tokio::time::Instant::now().checked_add(d).or_else(|| {
            warn!(
                duration = ?d,
                "loadtest duration overflows the instant clock, running without a deadline"
            );
            None
        })
    });

    for i in 0..config.session_count {
        if ct.is_cancelled() {
            break;
        }
        if let Some(d) = deadline
            && tokio::time::Instant::now() >= d
        {
            info!(
                duration = ?config.duration,
                "loadtest duration reached during ramp-up, cancelling sessions"
            );
            ct.cancel();
            deadline = None;
            break;
        }

        let session_connected = connected.clone();
        let session_failed = failed.clone();
        let session_cancelled = cancelled.clone();
        let session_metrics = metrics.clone();
        let future = make_session(i);

        join_set.spawn(async move {
            let (m, result) = future.await;
            let mut stats = session_metrics.lock().await;
            stats.packets += m.packets;
            stats.bytes += m.bytes;
            stats.errors += m.errors;
            stats.nack_count += m.nack_count;
            stats.pli_count += m.pli_count;
            // Connected duration only merges from sessions that actually
            // connected; failed or cancelled sessions would skew the average
            // computed over the connected count.
            if matches!(result, Ok(SessionOutcome::Connected)) {
                stats.connected_duration += m.connected_duration;
            }
            drop(stats);
            match result {
                Ok(SessionOutcome::Connected) => {
                    session_connected.fetch_add(1, Ordering::Relaxed);
                }
                Ok(SessionOutcome::Cancelled) => {
                    session_cancelled.fetch_add(1, Ordering::Relaxed);
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
                _ = sleep_until_opt(deadline) => {
                    info!(
                        duration = ?config.duration,
                        "loadtest duration reached during ramp-up, cancelling sessions"
                    );
                    ct.cancel();
                    deadline = None;
                    break;
                }
            }
        }
    }

    // Join sessions as they finish, racing against the overall duration.
    // Cancellation — external or duration — propagates to the sessions
    // through their child tokens; anything still running after DRAIN_GRACE is
    // aborted so a stuck session cannot hang the run. When every session ends
    // early (e.g. all failed to connect), this returns immediately instead of
    // waiting out the full duration.
    let mut drain_deadline = None;
    while !join_set.is_empty() {
        tokio::select! {
            _ = sleep_until_opt(deadline) => {
                info!(duration = ?config.duration, "loadtest duration reached, cancelling sessions");
                ct.cancel();
                deadline = None;
            }
            _ = ct.cancelled(), if drain_deadline.is_none() => {
                drain_deadline = Some(tokio::time::Instant::now() + DRAIN_GRACE);
            }
            _ = sleep_until_opt(drain_deadline) => {
                warn!(
                    grace = ?DRAIN_GRACE,
                    "loadtest sessions did not stop after cancellation, aborting remaining tasks"
                );
                join_set.abort_all();
                break;
            }
            result = join_set.join_next() => {
                if let Some(Err(e)) = result {
                    if e.is_cancelled() {
                        aborted += 1;
                    } else {
                        warn!(error = ?e, "loadtest session task panicked");
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }
    while let Some(result) = join_set.join_next().await {
        if let Err(e) = result {
            if e.is_cancelled() {
                aborted += 1;
            } else {
                warn!(error = ?e, "loadtest session task panicked");
                failed.fetch_add(1, Ordering::Relaxed);
            }
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
        sessions_cancelled: cancelled.load(Ordering::Relaxed),
        sessions_aborted: aborted,
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
        sessions_cancelled = stats.sessions_cancelled,
        sessions_aborted = stats.sessions_aborted,
        total_packets = stats.total_packets,
        total_bytes = stats.total_bytes,
        "Loadtest completed"
    );

    // The run is only genuinely broken when nothing connected AND not every
    // session was a clean cancellation (e.g. the duration fired during
    // ramp-up): failed or aborted sessions mean something went wrong.
    if stats.sessions_connected == 0 && spawned > 0 && stats.sessions_cancelled < spawned as u64 {
        anyhow::bail!("all {} loadtest session(s) failed", spawned);
    }

    // The all-cancelled case is a valid run (e.g. the duration fired during
    // ramp-up), but it means zero traffic ever flowed; say so loudly instead
    // of letting a no-op loadtest pass silently.
    if stats.sessions_connected == 0 && spawned > 0 && stats.sessions_cancelled == spawned as u64 {
        warn!("all sessions were cancelled before connecting; no traffic flowed");
    }

    Ok(stats)
}

/// Sleep until `deadline` when set; pend forever otherwise (for `select!`).
async fn sleep_until_opt(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn duration_limits_ramp_up() {
        let config = LoadtestConfig {
            session_count: 5,
            spawn_interval: Duration::from_millis(50),
            duration: Some(Duration::from_millis(70)),
        };
        let ct = CancellationToken::new();
        let session_ct = ct.clone();

        let stats = run_sessions(&config, ct, move |_| {
            let session_ct = session_ct.clone();
            async move {
                session_ct.cancelled().await;
                (
                    SessionMetrics {
                        packets: 1,
                        bytes: 1,
                        ..Default::default()
                    },
                    Ok(SessionOutcome::Connected),
                )
            }
        })
        .await
        .unwrap();

        assert!(stats.sessions_total > 0);
        assert!(
            stats.sessions_total < config.session_count,
            "duration should stop ramp-up before all sessions spawn: {stats:?}"
        );
    }

    #[tokio::test]
    async fn failed_sessions_still_contribute_metrics() {
        let config = LoadtestConfig {
            session_count: 2,
            spawn_interval: Duration::ZERO,
            duration: None,
        };
        let ct = CancellationToken::new();

        let stats = run_sessions(&config, ct, |i| async move {
            let metrics = SessionMetrics {
                packets: 10,
                bytes: 100,
                connected_duration: Duration::from_secs(if i == 0 { 1 } else { 5 }),
                ..Default::default()
            };
            if i == 0 {
                (metrics, Ok(SessionOutcome::Connected))
            } else {
                (metrics, Err(anyhow::anyhow!("boom")))
            }
        })
        .await
        .unwrap();

        assert_eq!(stats.sessions_connected, 1);
        assert_eq!(stats.sessions_failed, 1);
        assert_eq!(stats.total_packets, 20);
        assert_eq!(stats.total_bytes, 200);
        // Only connected sessions contribute connected_duration.
        assert_eq!(stats.total_connected_duration, Duration::from_secs(1));
    }

    #[tokio::test]
    async fn cancelled_sessions_count_and_do_not_fail_the_run() {
        let config = LoadtestConfig {
            session_count: 2,
            spawn_interval: Duration::ZERO,
            duration: Some(Duration::from_millis(50)),
        };
        let ct = CancellationToken::new();
        let session_ct = ct.clone();

        let stats = run_sessions(&config, ct, move |_| {
            let session_ct = session_ct.clone();
            async move {
                // Cancelled before connecting: no traffic, no failure.
                session_ct.cancelled().await;
                (SessionMetrics::default(), Ok(SessionOutcome::Cancelled))
            }
        })
        .await
        .unwrap();

        assert_eq!(stats.sessions_connected, 0);
        assert_eq!(stats.sessions_cancelled, 2);
        assert_eq!(stats.sessions_failed, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn aborted_sessions_are_counted() {
        let config = LoadtestConfig {
            session_count: 2,
            spawn_interval: Duration::ZERO,
            duration: Some(Duration::from_secs(1)),
        };
        let ct = CancellationToken::new();
        let session_ct = ct.clone();

        let stats = run_sessions(&config, ct, move |i| {
            let session_ct = session_ct.clone();
            async move {
                if i == 0 {
                    session_ct.cancelled().await;
                } else {
                    // Never finishes and ignores cancellation.
                    std::future::pending::<()>().await;
                }
                (SessionMetrics::default(), Ok(SessionOutcome::Connected))
            }
        })
        .await
        .unwrap();

        assert_eq!(stats.sessions_connected, 1);
        assert_eq!(stats.sessions_aborted, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn stuck_sessions_are_aborted_after_drain_grace() {
        let config = LoadtestConfig {
            session_count: 1,
            spawn_interval: Duration::ZERO,
            duration: Some(Duration::from_secs(1)),
        };
        let ct = CancellationToken::new();

        let error = run_sessions(&config, ct, |_| async move {
            // Never finishes and ignores cancellation.
            std::future::pending::<()>().await;
            (SessionMetrics::default(), Ok(SessionOutcome::Connected))
        })
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("all 1 loadtest session(s) failed")
        );
    }
}
