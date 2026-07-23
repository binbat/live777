//! Stream-lifecycle hook scripts.
//!
//! A consumer of the manager's event bus (see [`crate::event`]) that runs
//! user-configured scripts on stream lifecycle transitions. Typical use:
//! start a capture device / hardware encoder when media actually starts
//! flowing on a stream and stop it when it ends to save resources — for
//! on-demand/provisioned streams the publish events (not `StreamCreated`,
//! which fires at startup) carry that signal.
//!
//! Execution model:
//!
//! - A dispatcher task forwards `StreamCreated`/`StreamDeleted`/
//!   `PublishStarted`/`PublishStopped` events into an internal unbounded
//!   queue. It never awaits a script, so slow hooks can never overrun the
//!   broadcast buffer and lose events.
//! - A single executor task drains the queue FIFO and runs each event's
//!   scripts sequentially: global `[hooks]` first, then the per-stream
//!   `[stream.<name>.hooks]`, each in configured order. All hooks of an
//!   earlier event finish before any hook of a later event starts, which
//!   yields a global ordering guarantee (stronger than the per-stream
//!   ordering the events actually require).

use std::process::Stdio;
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tracing::{debug, warn};

use crate::config::{HookConfig, HooksConfig, OnError, StreamConfig};
use crate::event::{Event, SessionStopReason, StreamDeleteReason};
use crate::stream::manager::Manager;

/// One lifecycle event's worth of hook work.
struct HookJob {
    /// `"stream-created"`, `"stream-deleted"`, `"publish-started"` or
    /// `"publish-stopped"` — passed to scripts as argv[1] and
    /// `LIVE777_EVENT`.
    event: &'static str,
    stream: String,
    /// Stop reason for `stream-deleted` / `publish-stopped` — argv[3] and
    /// `LIVE777_REASON`.
    reason: Option<&'static str>,
    /// Publisher session id for publish events — `LIVE777_SESSION`.
    session: Option<String>,
    /// Global scripts followed by per-stream scripts, in configured order.
    scripts: Vec<String>,
}

fn reason_str(reason: StreamDeleteReason) -> &'static str {
    match reason {
        StreamDeleteReason::ApiDeleted => "api-deleted",
        StreamDeleteReason::PublishLeaveTimeout => "publish-leave-timeout",
        StreamDeleteReason::SubscribeLeaveTimeout => "subscribe-leave-timeout",
        StreamDeleteReason::Orphaned => "orphaned",
        StreamDeleteReason::Reset => "reset",
    }
}

fn session_reason_str(reason: SessionStopReason) -> &'static str {
    match reason {
        SessionStopReason::PeerClosed => "peer-closed",
        SessionStopReason::ApiDeleted => "api-deleted",
        SessionStopReason::IdleTimeout => "idle-timeout",
    }
}

/// Effective hook list for one event: global scripts first, then the
/// per-stream ones, each in configured order (mirrors `Strategy::effective`).
fn effective_scripts(global: &[String], per_stream: Option<&[String]>) -> Vec<String> {
    let mut scripts = global.to_vec();
    if let Some(per_stream) = per_stream {
        scripts.extend(per_stream.iter().cloned());
    }
    scripts
}

/// Spawn the hook dispatcher and executor. Both tasks are skipped entirely
/// when no hooks are configured anywhere (the common case).
///
/// Must be called before any source auto-start so streams created at
/// startup are not missed: the broadcast bus does not replay events sent
/// before the subscription.
pub fn init(manager: &Manager, hooks: HooksConfig, stream_cfg: StreamConfig) {
    let global_empty = hooks.hooks.on_stream_created.is_empty()
        && hooks.hooks.on_stream_deleted.is_empty()
        && hooks.hooks.on_publish_started.is_empty()
        && hooks.hooks.on_publish_stopped.is_empty();
    let per_stream_empty = stream_cfg.streams.values().all(|e| {
        e.hooks.on_stream_created.is_empty()
            && e.hooks.on_stream_deleted.is_empty()
            && e.hooks.on_publish_started.is_empty()
            && e.hooks.on_publish_stopped.is_empty()
    });
    if global_empty && per_stream_empty {
        debug!("no stream hooks configured, hook executor disabled");
        return;
    }

    // Catch path typos early; a missing script is not fatal — spawn errors
    // are reported (and counted by on_error) at run time like any failure.
    let all_scripts = hooks
        .hooks
        .on_stream_created
        .iter()
        .chain(&hooks.hooks.on_stream_deleted)
        .chain(&hooks.hooks.on_publish_started)
        .chain(&hooks.hooks.on_publish_stopped)
        .chain(stream_cfg.streams.values().flat_map(|e| {
            e.hooks
                .on_stream_created
                .iter()
                .chain(&e.hooks.on_stream_deleted)
                .chain(&e.hooks.on_publish_started)
                .chain(&e.hooks.on_publish_stopped)
        }));
    for script in all_scripts {
        if !std::path::Path::new(script).exists() {
            warn!("hook script does not exist : {}", script);
        }
    }

    let (tx, rx) = mpsc::unbounded_channel::<HookJob>();

    let timeout = (hooks.timeout_ms > 0).then(|| Duration::from_millis(hooks.timeout_ms));
    tokio::spawn(executor(rx, timeout, hooks.on_error));

    spawn_dispatcher(manager.subscribe_event(), hooks.hooks, stream_cfg, tx);
}

/// Forward matching bus events into the executor queue. Never awaits a
/// script, so a slow hook can never overrun the broadcast buffer.
fn spawn_dispatcher(
    mut events: broadcast::Receiver<Event>,
    global: HookConfig,
    stream_cfg: StreamConfig,
    tx: mpsc::UnboundedSender<HookJob>,
) {
    tokio::spawn(async move {
        loop {
            let job = match events.recv().await {
                Ok(Event::StreamCreated { stream }) => HookJob {
                    event: "stream-created",
                    scripts: effective_scripts(
                        &global.on_stream_created,
                        stream_cfg
                            .streams
                            .get(&stream)
                            .map(|e| e.hooks.on_stream_created.as_slice()),
                    ),
                    stream,
                    reason: None,
                    session: None,
                },
                Ok(Event::StreamDeleted { stream, reason }) => HookJob {
                    event: "stream-deleted",
                    scripts: effective_scripts(
                        &global.on_stream_deleted,
                        stream_cfg
                            .streams
                            .get(&stream)
                            .map(|e| e.hooks.on_stream_deleted.as_slice()),
                    ),
                    stream,
                    reason: Some(reason_str(reason)),
                    session: None,
                },
                Ok(Event::PublishStarted { stream, session }) => HookJob {
                    event: "publish-started",
                    scripts: effective_scripts(
                        &global.on_publish_started,
                        stream_cfg
                            .streams
                            .get(&stream)
                            .map(|e| e.hooks.on_publish_started.as_slice()),
                    ),
                    stream,
                    reason: None,
                    session: Some(session),
                },
                Ok(Event::PublishStopped {
                    stream,
                    session,
                    reason,
                }) => HookJob {
                    event: "publish-stopped",
                    scripts: effective_scripts(
                        &global.on_publish_stopped,
                        stream_cfg
                            .streams
                            .get(&stream)
                            .map(|e| e.hooks.on_publish_stopped.as_slice()),
                    ),
                    stream,
                    reason: Some(session_reason_str(reason)),
                    session: Some(session),
                },
                Ok(_) => continue,
                // Unlike snapshot consumers, hooks cannot reconcile after the
                // fact — a missed "stream-created" script cannot be rerun — so the
                // loss is surfaced loudly instead of being smoothed over.
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("hook dispatcher dropped {} stream events due to lag", n);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            if job.scripts.is_empty() {
                continue;
            }
            // The executor is gone only if this task is shutting down too.
            if tx.send(job).is_err() {
                break;
            }
        }
    });
}

/// Drain the queue strictly in order: every script of an event is awaited
/// before the next event's first script starts.
async fn executor(
    mut rx: mpsc::UnboundedReceiver<HookJob>,
    timeout: Option<Duration>,
    on_error: OnError,
) {
    while let Some(job) = rx.recv().await {
        for script in &job.scripts {
            if let Err(e) = run_script(script, &job, timeout).await {
                warn!(
                    "hook script failed, script : {}, event : {}, stream : {}, error : {}",
                    script, job.event, job.stream, e
                );
                if on_error == OnError::Stop {
                    break;
                }
            }
        }
    }
}

/// Run one hook script and wait for it to exit.
///
/// Contract: argv is `<event> <stream> [reason]`; the same values are also
/// exported as `LIVE777_EVENT` / `LIVE777_STREAM` / `LIVE777_REASON`, and
/// publish events additionally export `LIVE777_SESSION`. Scripts should
/// return quickly after initiating their work (e.g. launch an encoder in
/// the background) — a blocked script blocks the whole hook queue.
/// Non-zero exit, spawn failure, and timeout kill all count as failure and
/// are handled per `on_error`.
async fn run_script(script: &str, job: &HookJob, timeout: Option<Duration>) -> anyhow::Result<()> {
    let mut cmd = tokio::process::Command::new(script);
    cmd.arg(job.event).arg(&job.stream);
    cmd.env("LIVE777_EVENT", job.event)
        .env("LIVE777_STREAM", &job.stream);
    if let Some(reason) = job.reason {
        cmd.arg(reason);
        cmd.env("LIVE777_REASON", reason);
    }
    if let Some(session) = &job.session {
        cmd.env("LIVE777_SESSION", session);
    }
    // Dropping the expired timeout future drops the Child, which kills it.
    cmd.kill_on_drop(true);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    debug!(
        "running hook script, script : {}, event : {}, stream : {}",
        script, job.event, job.stream
    );
    let child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("spawn failed : {}", e))?;
    let output = match timeout {
        Some(t) => match tokio::time::timeout(t, child.wait_with_output()).await {
            Ok(output) => output.map_err(|e| anyhow::anyhow!("wait failed : {}", e))?,
            Err(_) => anyhow::bail!("timed out after {} ms", t.as_millis()),
        },
        None => child
            .wait_with_output()
            .await
            .map_err(|e| anyhow::anyhow!("wait failed : {}", e))?,
    };
    if !output.stdout.is_empty() {
        debug!(
            "hook script stdout, script : {}, stream : {}, stdout : {}",
            script,
            job.stream,
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    if output.status.success() {
        if !output.stderr.is_empty() {
            debug!(
                "hook script stderr, script : {}, stream : {}, stderr : {}",
                script,
                job.stream,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        return Ok(());
    }
    // stderr is how scripts explain failures — attach it to the error so it
    // shows up in the warn log, not just at debug level.
    anyhow::bail!(
        "exit status : {}{}",
        output.status,
        if output.stderr.is_empty() {
            String::new()
        } else {
            format!(
                ", stderr : {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_scripts_merge_global_then_per_stream() {
        let global = vec!["/g1.sh".to_string(), "/g2.sh".to_string()];
        let per_stream = vec!["/p1.sh".to_string()];

        assert_eq!(
            effective_scripts(&global, Some(&per_stream)),
            ["/g1.sh", "/g2.sh", "/p1.sh"]
        );
        assert_eq!(effective_scripts(&global, None), ["/g1.sh", "/g2.sh"]);
        assert!(effective_scripts(&[], None).is_empty());
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use crate::config::{Config, HookConfig, StreamConfig, StreamEntry};
        use std::path::{Path, PathBuf};
        use tokio_util::sync::CancellationToken;

        fn write_script(dir: &Path, name: &str, body: &str) -> String {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.to_str().unwrap().to_string()
        }

        /// Append-to-log script body; each invocation appends one line.
        /// Double quotes keep `$VAR`/`$N` expansion working.
        fn log_line_script(dir: &Path, name: &str, log: &Path, line: &str) -> String {
            write_script(
                dir,
                name,
                &format!("#!/bin/sh\necho \"{}\" >> {}\n", line, log.display()),
            )
        }

        async fn wait_for_lines(path: &PathBuf, n: usize) -> String {
            for _ in 0..100 {
                if let Ok(content) = std::fs::read_to_string(path)
                    && content.lines().count() >= n
                {
                    return content;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            panic!("timed out waiting for {} lines in {}", n, path.display());
        }

        fn job(event: &'static str, scripts: Vec<String>) -> HookJob {
            HookJob {
                event,
                stream: "cam1".to_string(),
                reason: None,
                session: None,
                scripts,
            }
        }

        #[tokio::test]
        async fn on_error_stop_skips_remaining_hooks() {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().join("order.log");
            let fail = write_script(dir.path(), "fail.sh", "#!/bin/sh\nexit 1\n");
            let marker = log_line_script(dir.path(), "marker.sh", &log, "marker");

            let (tx, rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(executor(rx, None, OnError::Stop));
            tx.send(job("stream-created", vec![fail, marker])).unwrap();
            drop(tx);
            handle.await.unwrap();

            assert!(!log.exists());
        }

        #[tokio::test]
        async fn on_error_continue_runs_remaining_hooks() {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().join("order.log");
            let fail = write_script(dir.path(), "fail.sh", "#!/bin/sh\nexit 1\n");
            let marker = log_line_script(dir.path(), "marker.sh", &log, "marker");

            let (tx, rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(executor(rx, None, OnError::Continue));
            tx.send(job("stream-created", vec![fail, marker])).unwrap();
            drop(tx);
            handle.await.unwrap();

            assert_eq!(std::fs::read_to_string(&log).unwrap(), "marker\n");
        }

        #[tokio::test]
        async fn timeout_kills_slow_script_and_executor_continues() {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().join("order.log");
            let slow = write_script(dir.path(), "slow.sh", "#!/bin/sh\nsleep 30\n");
            let marker = log_line_script(dir.path(), "marker.sh", &log, "marker");

            let (tx, rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(executor(
                rx,
                Some(Duration::from_millis(100)),
                OnError::Stop,
            ));
            // The timed-out job stops; the *next* event must still run.
            tx.send(job("stream-created", vec![slow])).unwrap();
            tx.send(job("stream-deleted", vec![marker])).unwrap();
            drop(tx);

            let start = std::time::Instant::now();
            handle.await.unwrap();
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "executor stayed stuck behind the slow script"
            );
            assert_eq!(std::fs::read_to_string(&log).unwrap(), "marker\n");
        }

        #[tokio::test]
        async fn hooks_run_in_event_order_with_metadata() {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().join("order.log");
            // argv contract: $1 = event, $3 = reason; env: LIVE777_STREAM.
            let created_global =
                log_line_script(dir.path(), "created1.sh", &log, "created-1 $LIVE777_STREAM");
            let created_stream = log_line_script(dir.path(), "created2.sh", &log, "created-2 $1");
            let deleted_global = log_line_script(dir.path(), "deleted1.sh", &log, "deleted-1 $3");

            let hooks = HooksConfig {
                hooks: HookConfig {
                    on_stream_created: vec![created_global],
                    on_stream_deleted: vec![deleted_global],
                    ..Default::default()
                },
                timeout_ms: 5_000,
                on_error: OnError::Stop,
            };
            let mut stream_cfg = StreamConfig::default();
            stream_cfg.streams.insert(
                "cam1".to_string(),
                StreamEntry {
                    hooks: HookConfig {
                        on_stream_created: vec![created_stream],
                        ..Default::default()
                    },
                    ..Default::default()
                },
            );

            let cancel = CancellationToken::new();
            let manager = Manager::new(Config::default(), cancel.clone()).await;
            init(&manager, hooks, stream_cfg);

            manager.stream_create("cam1".to_string()).await.unwrap();
            manager.stream_delete("cam1".to_string()).await.unwrap();

            let content = wait_for_lines(&log, 3).await;
            assert_eq!(
                content.lines().collect::<Vec<_>>(),
                [
                    "created-1 cam1",
                    "created-2 stream-created",
                    "deleted-1 api-deleted"
                ]
            );
            cancel.cancel();
        }

        #[tokio::test]
        async fn publish_hooks_carry_event_session_and_reason() {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().join("publish.log");
            // argv contract: $1 = event, $3 = reason; env: LIVE777_SESSION.
            let started = log_line_script(
                dir.path(),
                "started.sh",
                &log,
                "started $1 $LIVE777_SESSION",
            );
            let stopped = log_line_script(
                dir.path(),
                "stopped.sh",
                &log,
                "stopped $3 $LIVE777_SESSION",
            );

            let (tx, rx) = mpsc::unbounded_channel();
            let handle = tokio::spawn(executor(rx, None, OnError::Stop));

            let (event_tx, event_rx) = broadcast::channel(16);
            spawn_dispatcher(
                event_rx,
                HookConfig {
                    on_publish_started: vec![started],
                    on_publish_stopped: vec![stopped],
                    ..Default::default()
                },
                StreamConfig::default(),
                tx,
            );

            event_tx
                .send(Event::PublishStarted {
                    stream: "cam1".to_string(),
                    session: "virtual-source".to_string(),
                })
                .unwrap();
            event_tx
                .send(Event::PublishStopped {
                    stream: "cam1".to_string(),
                    session: "virtual-source".to_string(),
                    reason: SessionStopReason::IdleTimeout,
                })
                .unwrap();

            let content = wait_for_lines(&log, 2).await;
            assert_eq!(
                content.lines().collect::<Vec<_>>(),
                [
                    "started publish-started virtual-source",
                    "stopped idle-timeout virtual-source"
                ]
            );

            // Closing the bus ends the dispatcher, which drops the queue
            // sender and lets the executor finish.
            drop(event_tx);
            handle.await.unwrap();
        }
    }
}
