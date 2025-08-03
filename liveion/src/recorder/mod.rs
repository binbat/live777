use glob::Pattern;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use opendal::Operator;
#[cfg(feature = "recorder")]
use storage::init_operator;

use crate::hook::{Event, StreamEventType};
use crate::stream::manager::Manager;

#[cfg(feature = "recorder")]
use crate::config::RecorderConfig;

mod segmenter;
mod task;
use task::RecordingTask;
pub mod codec;
mod fmp4;

// Segment metadata storage for pull API
use api::recorder::SegmentMetadata;
use std::collections::BTreeMap;

static TASKS: Lazy<RwLock<HashMap<String, RecordingTask>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

static STORAGE: Lazy<RwLock<Option<Operator>>> = Lazy::new(|| RwLock::new(None));

// Store segment metadata for pull API - organized by stream
static SEGMENT_METADATA: Lazy<RwLock<HashMap<String, BTreeMap<i64, SegmentMetadata>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

// Node alias for identification
static NODE_ALIAS: Lazy<RwLock<Option<String>>> = Lazy::new(|| RwLock::new(None));

/// Initialize recorder event listener.
#[cfg(feature = "recorder")]
pub async fn init(manager: Arc<Manager>, cfg: RecorderConfig) {
    let manager_clone = manager.clone();

    // Store node alias globally if provided
    if let Some(ref node_alias) = cfg.node_alias {
        let mut node_alias_writer = NODE_ALIAS.write().await;
        *node_alias_writer = Some(node_alias.clone());
        tracing::info!("[recorder] Node alias set to: {}", node_alias);
    }

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
    let mut recv = manager.subscribe_event();
    tokio::spawn(async move {
        while let Ok(event) = recv.recv().await {
            if let Event::Stream(stream_event) = event {
                match stream_event.r#type {
                    StreamEventType::Up => {
                        let stream_name = stream_event.stream.stream;
                        if should_record(&cfg.auto_streams, &stream_name) {
                            if let Err(e) = start(manager_clone.clone(), stream_name.clone()).await
                            {
                                tracing::error!("[recorder] start failed: {}", e);
                            }
                        }
                    }
                    StreamEventType::Down => {
                        let stream_name = stream_event.stream.stream;
                        let mut map = TASKS.write().await;
                        if let Some(task) = map.remove(&stream_name) {
                            task.stop();
                            tracing::info!("[recorder] stop recording task for {}", stream_name);
                        }
                    }
                }
            }
        }
    });
}

/// Entry point for starting recording manually or automatically
pub async fn start(manager: Arc<Manager>, stream: String) -> anyhow::Result<()> {
    let mut map = TASKS.write().await;
    if map.contains_key(&stream) {
        tracing::info!("[recorder] stream {} is already recording", stream);
        return Ok(());
    }

    let task = RecordingTask::spawn(manager, &stream).await?;
    map.insert(stream.clone(), task);
    tracing::info!("[recorder] spawn recording task for {}", stream);
    Ok(())
}

fn should_record(patterns: &[String], stream: &str) -> bool {
    for p in patterns {
        if let Ok(pat) = Pattern::new(p) {
            if pat.matches(stream) {
                return true;
            }
        }
    }
    false
}

/// Add segment metadata for pull API
pub async fn add_segment_metadata(stream: &str, metadata: SegmentMetadata) {
    let mut segments = SEGMENT_METADATA.write().await;
    let stream_segments = segments
        .entry(stream.to_string())
        .or_insert_with(BTreeMap::new);
    stream_segments.insert(metadata.start_ts, metadata);

    if stream_segments.len() > 1000 {
        let keys_to_remove: Vec<_> = stream_segments
            .keys()
            .take(stream_segments.len() - 1000)
            .cloned()
            .collect();
        for key in keys_to_remove {
            stream_segments.remove(&key);
        }
    }
}

/// Pull segments for Liveman
pub async fn pull_segments(
    stream_filter: Option<&str>,
    since_ts: Option<i64>,
    limit: u32,
) -> api::recorder::PullSegmentsResponse {
    let segments = SEGMENT_METADATA.read().await;
    let node_alias = NODE_ALIAS
        .read()
        .await
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let mut all_segments = Vec::new();
    let mut last_ts = None;

    for (stream_name, stream_segments) in segments.iter() {
        // Apply stream filter if provided
        if let Some(filter) = stream_filter {
            if stream_name != filter {
                continue;
            }
        }

        // Filter by timestamp and collect segments
        let filtered_segments: Vec<_> = stream_segments
            .range(since_ts.unwrap_or(0)..)
            .take(limit as usize)
            .map(|(_, segment)| segment.clone())
            .collect();

        if !filtered_segments.is_empty() {
            if let Some(last_segment) = filtered_segments.last() {
                last_ts = Some(last_segment.start_ts.max(last_ts.unwrap_or(0)));
            }
            all_segments.extend(filtered_segments);
        }
    }

    // Sort by timestamp and apply final limit
    all_segments.sort_by_key(|s| s.start_ts);
    if all_segments.len() > limit as usize {
        all_segments.truncate(limit as usize);
    }

    let total_count = all_segments.len() as u32;
    let has_more = total_count == limit;
    let stream_name = stream_filter.unwrap_or("").to_string();

    api::recorder::PullSegmentsResponse {
        node_alias,
        stream: stream_name,
        segments: all_segments,
        last_ts,
        total_count,
        has_more,
    }
}
