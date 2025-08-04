use glob::Pattern;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::Utc;

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

// Recording session storage for pull API
use api::recorder::{RecordingSession, RecordingStatus, SegmentInfo};
use std::collections::BTreeMap;

static TASKS: Lazy<RwLock<HashMap<String, RecordingTask>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

static STORAGE: Lazy<RwLock<Option<Operator>>> = Lazy::new(|| RwLock::new(None));

// Store recording sessions for pull API - organized by stream
static RECORDING_SESSIONS: Lazy<RwLock<HashMap<String, BTreeMap<i64, RecordingSession>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

// Store segments for pull API - organized by stream  
static SEGMENTS: Lazy<RwLock<HashMap<String, BTreeMap<i64, SegmentInfo>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

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
                            // Mark recording session as completed
                            complete_recording_session(&stream_name).await;
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
    
    // Create new recording session
    start_recording_session(&stream).await;
    
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

/// Start a new recording session
pub async fn start_recording_session(stream: &str) {
    let now = Utc::now();
    let start_ts = now.timestamp_micros();
    
    // Generate MPD path based on stream and date
    let date_path = now.format("%Y/%m/%d").to_string();
    let mpd_path = format!("{stream}/{date_path}/manifest.mpd");
    
    let session = RecordingSession {
        stream: stream.to_string(),
        start_ts,
        end_ts: None,
        duration_ms: None,
        mpd_path,
        status: RecordingStatus::Active,
    };
    
    let mut sessions = RECORDING_SESSIONS.write().await;
    let stream_sessions = sessions
        .entry(stream.to_string())
        .or_insert_with(BTreeMap::new);
    stream_sessions.insert(start_ts, session);
    
    // Keep only last 100 sessions per stream to prevent memory bloat
    if stream_sessions.len() > 100 {
        let keys_to_remove: Vec<_> = stream_sessions
            .keys()
            .take(stream_sessions.len() - 100)
            .cloned()
            .collect();
        for key in keys_to_remove {
            stream_sessions.remove(&key);
        }
    }
    
    tracing::info!("[recorder] Started recording session for stream: {}", stream);
}

/// Complete the most recent recording session for a stream
pub async fn complete_recording_session(stream: &str) {
    let mut sessions = RECORDING_SESSIONS.write().await;
    
    if let Some(stream_sessions) = sessions.get_mut(stream) {
        // Find the most recent active session
        if let Some((_, session)) = stream_sessions
            .iter_mut()
            .rev()
            .find(|(_, s)| matches!(s.status, RecordingStatus::Active))
        {
            let now = Utc::now();
            let end_ts = now.timestamp_micros();
            let duration_ms = ((end_ts - session.start_ts) / 1000) as i32;
            
            session.end_ts = Some(end_ts);
            session.duration_ms = Some(duration_ms);
            session.status = RecordingStatus::Completed;
            
            tracing::info!("[recorder] Completed recording session for stream: {} (duration: {}ms)", stream, duration_ms);
        }
    }
}

/// Pull recording sessions for Liveman
pub async fn pull_recordings(
    stream_filter: Option<&str>,
    since_ts: Option<i64>,
    limit: u32,
) -> api::recorder::PullRecordingsResponse {
    let sessions = RECORDING_SESSIONS.read().await;
    
    let mut all_sessions = Vec::new();
    let mut last_ts = None;
    
    for (stream_name, stream_sessions) in sessions.iter() {
        // Apply stream filter if provided
        if let Some(filter) = stream_filter {
            if stream_name != filter {
                continue;
            }
        }
        
        // Filter by timestamp and collect sessions
        let filtered_sessions: Vec<_> = stream_sessions
            .range(since_ts.unwrap_or(0)..)
            .take(limit as usize)
            .map(|(_, session)| {
                // Update last_ts to the session's last update time
                let session_ts = session.end_ts.unwrap_or(session.start_ts);
                last_ts = Some(session_ts.max(last_ts.unwrap_or(0)));
                session.clone()
            })
            .collect();
        
        all_sessions.extend(filtered_sessions);
    }
    
    // Sort by start timestamp and apply final limit
    all_sessions.sort_by_key(|s| s.start_ts);
    if all_sessions.len() > limit as usize {
        all_sessions.truncate(limit as usize);
    }
    
    api::recorder::PullRecordingsResponse {
        sessions: all_sessions,
        last_ts,
    }
}
