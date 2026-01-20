use glob::Pattern;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::{self, MissedTickBehavior};

use opendal::Operator;
#[cfg(feature = "recorder")]
use storage::init_operator;

use crate::hook::{Event, StreamEventType};
use crate::stream::manager::Manager;
use api::recorder::{
    AckRecordingsRequest, AckRecordingsResponse, PullRecordingsRequest, PullRecordingsResponse,
    RecordingStatus,
};
use chrono::Utc;

#[cfg(feature = "recorder")]
use crate::config::RecorderConfig;

mod index;
mod pli_backoff;
mod segmenter;
mod task;
use task::RecordingTask;
pub mod codec;
mod fmp4;
use index::{RecordingIndexEntry, RecordingsIndex};

static TASKS: Lazy<RwLock<HashMap<String, RecordingTask>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

static STORAGE: Lazy<RwLock<Option<Operator>>> = Lazy::new(|| RwLock::new(None));
static INDEX: Lazy<RwLock<Option<Arc<RecordingsIndex>>>> = Lazy::new(|| RwLock::new(None));
static NODE_ALIAS: Lazy<RwLock<Option<String>>> = Lazy::new(|| RwLock::new(None));

#[derive(Clone, Debug)]
pub struct RecordingInfo {
    pub record_dir: String,
    pub record_id: i64,
    pub start_ts_micros: i64,
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

    {
        let mut alias = NODE_ALIAS.write().await;
        *alias = cfg.node_alias.clone();
    }

    if let Some(index_path) = resolve_index_path(&cfg) {
        let mut index_writer = INDEX.write().await;
        if index_writer.is_none() {
            match RecordingsIndex::load(index_path).await {
                Ok(idx) => {
                    *index_writer = Some(Arc::new(idx));
                    tracing::info!("[recorder] index.json initialized");
                }
                Err(e) => {
                    tracing::error!("[recorder] failed to load index.json: {}", e);
                }
            }
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
                            let info = task.info.clone();
                            let outcome = task.stop().await;
                            update_index_on_stop(&stream_name, &info, outcome).await;
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
    update_index_on_start(&stream, &info).await;
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
        let info = task.info.clone();
        let outcome = task.stop().await;
        update_index_on_stop(&stream, &info, outcome).await;
        tracing::info!("[recorder] stopped recording task for {}", stream);
    } else {
        tracing::info!("[recorder] no recording task found for {}", stream);
    }
    Ok(())
}

async fn update_index_on_start(stream: &str, info: &RecordingInfo) {
    let index_opt = get_index().await;
    if index_opt.is_none() {
        return;
    }

    let record = record_key(info);
    let mpd_path = format!("{}/manifest.mpd", info.record_dir);
    let entry = RecordingIndexEntry {
        record,
        stream: stream.to_string(),
        record_dir: info.record_dir.clone(),
        mpd_path,
        start_ts: info.start_ts_micros,
        end_ts: None,
        duration_ms: None,
        status: RecordingStatus::Active,
        node_alias: NODE_ALIAS.read().await.clone(),
        updated_at: Utc::now().timestamp_micros(),
    };

    if let Some(index) = index_opt
        && let Err(e) = index.upsert(entry).await
    {
        tracing::error!("[recorder] index.json upsert failed: {}", e);
    }
}

async fn update_index_on_stop(
    stream: &str,
    info: &RecordingInfo,
    outcome: task::RecordingStopOutcome,
) {
    if let Some(index) = get_index().await {
        let record = record_key(info);
        if let Err(e) = index
            .update_status(
                stream,
                &record,
                outcome.status,
                Some(outcome.end_ts),
                Some(outcome.duration_ms),
            )
            .await
        {
            tracing::error!("[recorder] index.json update failed: {}", e);
        }
    }
}

async fn get_index() -> Option<Arc<RecordingsIndex>> {
    let index = INDEX.read().await;
    index.clone()
}

pub async fn pull_recordings(req: PullRecordingsRequest) -> anyhow::Result<PullRecordingsResponse> {
    let Some(index) = get_index().await else {
        return Ok(PullRecordingsResponse {
            sessions: Vec::new(),
            last_ts: None,
        });
    };

    let (sessions, last_ts) = index
        .list_sessions(req.stream, req.since_ts, req.limit)
        .await;

    Ok(PullRecordingsResponse { sessions, last_ts })
}

pub async fn ack_recordings(req: AckRecordingsRequest) -> anyhow::Result<AckRecordingsResponse> {
    let Some(index) = get_index().await else {
        return Ok(AckRecordingsResponse { deleted: 0 });
    };

    let deleted = index.ack(req).await?;
    Ok(AckRecordingsResponse { deleted })
}

fn record_key(info: &RecordingInfo) -> String {
    if info.record_id > 0 {
        return info.record_id.to_string();
    }

    let ts = (info.start_ts_micros / 1_000_000).max(0);
    ts.to_string()
}

fn resolve_index_path(cfg: &RecorderConfig) -> Option<PathBuf> {
    if let Some(path) = cfg.index_path.as_ref()
        && !path.trim().is_empty()
    {
        return Some(PathBuf::from(path));
    }

    match &cfg.storage {
        storage::StorageConfig::Fs { root } => Some(PathBuf::from(root).join("index.json")),
        _ => Some(PathBuf::from("./recordings/index.json")),
    }
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
    base.clamp(1, 300)
}
