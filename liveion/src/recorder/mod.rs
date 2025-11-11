use glob::Pattern;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::{self, MissedTickBehavior};

use opendal::Operator;
#[cfg(feature = "recorder")]
use storage::init_operator;

use crate::hook::{Event, StreamEventType};
use crate::stream::manager::Manager;

#[cfg(feature = "recorder")]
use crate::config::RecorderConfig;

mod pli_backoff;
mod segmenter;
mod task;
use task::RecordingTask;
pub mod codec;
mod fmp4;

static TASKS: Lazy<RwLock<HashMap<String, RecordingTask>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

static STORAGE: Lazy<RwLock<Option<Operator>>> = Lazy::new(|| RwLock::new(None));

#[derive(Clone, Debug)]
pub struct RecordingInfo {
    pub record_dir: String,
    pub record_id: i64,
}

/// Initialize recorder event listener.
#[cfg(feature = "recorder")]
pub async fn init(manager: Arc<Manager>, cfg: RecorderConfig) {
    let manager_clone = manager.clone();

    // Initialize storage Operator
    {
        let mut storage_writer = STORAGE.write().await;
        if storage_writer.is_none() {
            tracing::info!(
                "[recorder] initializing storage operator with config: {:?}",
                cfg.storage
            );
            match init_operator(&cfg.storage).await {
                Ok(op) => {
                    *storage_writer = Some(op);
                    tracing::info!("[recorder] storage backend initialized successfully");
                }
                Err(e) => {
                    tracing::error!("[recorder] failed to initialize storage backend: {}", e);
                    return;
                }
            }
        } else {
            tracing::debug!("[recorder] storage operator already initialized");
        }
    }

    let cfg = Arc::new(cfg);
    let cfg_for_events = cfg.clone();
    let mut recv = manager.subscribe_event();
    tokio::spawn(async move {
        while let Ok(event) = recv.recv().await {
            if let Event::Stream(stream_event) = event {
                match stream_event.r#type {
                    StreamEventType::Up => {
                        let stream_name = stream_event.stream.stream;
                        if should_record(&cfg_for_events.auto_streams, &stream_name)
                            && let Err(e) =
                                start(manager_clone.clone(), stream_name.clone(), None).await
                        {
                            tracing::error!("[recorder] start failed: {}", e);
                        }
                    }
                    StreamEventType::Down => {
                        let stream_name = stream_event.stream.stream;
                        let task_opt = {
                            let mut map = TASKS.write().await;
                            map.remove(&stream_name)
                        };

                        if let Some(task) = task_opt {
                            task.stop().await;
                            tracing::info!("[recorder] stop recording task for {}", stream_name);
                        }
                    }
                }
            }
        }
    });

    if cfg.max_recording_seconds > 0 {
        let manager_for_rotation = manager.clone();
        let cfg_for_rotation = cfg.clone();
        tokio::spawn(async move {
            rotation_loop(manager_for_rotation, cfg_for_rotation).await;
        });
    } else {
        tracing::info!("[recorder] max_recording_seconds is 0, automatic rotation disabled");
    }
}

/// Entry point for starting recording manually or automatically
pub async fn start(
    manager: Arc<Manager>,
    stream: String,
    base_dir: Option<String>,
) -> anyhow::Result<RecordingInfo> {
    let mut map = TASKS.write().await;
    if let Some(existing) = map.get(&stream) {
        tracing::info!("[recorder] stream {} is already recording", stream);
        return Ok(existing.info.clone());
    }
    let task = RecordingTask::spawn(manager, &stream, base_dir).await?;
    let info = task.info.clone();
    map.insert(stream.clone(), task);

    tracing::info!("[recorder] spawn recording task for {}", stream);
    Ok(info)
}

/// Check whether a stream is currently being recorded on this node
pub async fn is_recording(stream: &str) -> bool {
    let map = TASKS.read().await;
    map.contains_key(stream)
}

// Query by stream id only

fn should_record(patterns: &[String], stream: &str) -> bool {
    for p in patterns {
        if let Ok(pat) = Pattern::new(p)
            && pat.matches(stream)
        {
            return true;
        }
    }
    false
}

/// Stop recording for a given stream if running
pub async fn stop(stream: String) -> anyhow::Result<()> {
    let task_opt = {
        let mut map = TASKS.write().await;
        map.remove(&stream)
    };

    if let Some(task) = task_opt {
        task.stop().await;
        tracing::info!("[recorder] stopped recording task for {}", stream);
    } else {
        tracing::info!("[recorder] no recording task found for {}", stream);
    }
    Ok(())
}

#[cfg(feature = "recorder")]
async fn rotation_loop(manager: Arc<Manager>, cfg: Arc<RecorderConfig>) {
    let max_seconds = cfg.max_recording_seconds;
    if max_seconds == 0 {
        return;
    }

    let interval_secs = rotation_check_interval(max_seconds);
    let mut ticker = time::interval(Duration::from_secs(interval_secs));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;
        if let Err(e) = enforce_max_duration(manager.clone(), max_seconds).await {
            tracing::error!("[recorder] duration rotation failed: {}", e);
        }
    }
}

#[cfg(feature = "recorder")]
async fn enforce_max_duration(manager: Arc<Manager>, max_seconds: u64) -> anyhow::Result<()> {
    let max_duration = Duration::from_secs(max_seconds);
    let candidates: Vec<(String, Option<String>)> = {
        let map = TASKS.read().await;
        map.iter()
            .filter_map(|(stream, task)| {
                if task.has_exceeded(max_duration) {
                    Some((stream.clone(), task.next_rotation_base_dir()))
                } else {
                    None
                }
            })
            .collect()
    };

    if candidates.is_empty() {
        return Ok(());
    }

    tracing::info!(
        "[recorder] rotating {} streams after {} seconds",
        candidates.len(),
        max_seconds
    );

    for (stream, _) in &candidates {
        if let Err(e) = stop(stream.clone()).await {
            tracing::error!(
                "[recorder] failed to stop stream {} during rotation: {}",
                stream,
                e
            );
        }
    }

    for (stream, base_dir) in candidates {
        if let Err(e) = start(manager.clone(), stream.clone(), base_dir).await {
            tracing::error!(
                "[recorder] failed to restart stream {} during rotation: {}",
                stream,
                e
            );
        } else {
            tracing::info!(
                "[recorder] restarted recording for stream {} after reaching max duration",
                stream
            );
        }
    }

    Ok(())
}

#[cfg(feature = "recorder")]
fn rotation_check_interval(max_seconds: u64) -> u64 {
    let quarter = max_seconds / 4;
    let base = if quarter == 0 { 1 } else { quarter };
    std::cmp::min(std::cmp::max(base, 1), 300)
}
