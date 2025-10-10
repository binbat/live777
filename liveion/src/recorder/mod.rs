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
mod pli_backoff;
use task::RecordingTask;
pub mod codec;
mod fmp4;

static TASKS: Lazy<RwLock<HashMap<String, RecordingTask>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

static STORAGE: Lazy<RwLock<Option<Operator>>> = Lazy::new(|| RwLock::new(None));

#[cfg(feature = "recorder")]
use chrono::{FixedOffset, TimeZone, Utc};

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

    // Start daily rotation loop if enabled
    if cfg.rotate_daily {
        let manager_for_rotate = manager.clone();
        let cfg_for_rotate = cfg.clone();
        tokio::spawn(async move {
            rotate_daily_loop(manager_for_rotate, cfg_for_rotate).await;
        });
    }
}

/// Entry point for starting recording manually or automatically
pub async fn start(
    manager: Arc<Manager>,
    stream: String,
    base_dir: Option<String>,
) -> anyhow::Result<()> {
    let mut map = TASKS.write().await;
    if map.contains_key(&stream) {
        tracing::info!("[recorder] stream {} is already recording", stream);
        return Ok(());
    }

    let task = RecordingTask::spawn(manager, &stream, base_dir).await?;
    map.insert(stream.clone(), task);

    tracing::info!("[recorder] spawn recording task for {}", stream);
    Ok(())
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
    let mut map = TASKS.write().await;
    if let Some(task) = map.remove(&stream) {
        task.stop();
        tracing::info!("[recorder] stopped recording task for {}", stream);
    } else {
        tracing::info!("[recorder] no recording task found for {}", stream);
    }
    Ok(())
}

#[cfg(feature = "recorder")]
async fn rotate_daily_loop(manager: Arc<Manager>, cfg: Arc<RecorderConfig>) {
    // Prepare timezone offset
    let offset_minutes = cfg.rotate_tz_offset_minutes;
    let tz =
        FixedOffset::east_opt(offset_minutes * 60).unwrap_or(FixedOffset::east_opt(0).unwrap());

    loop {
        // Compute sleep duration until next local midnight
        let now_utc = Utc::now();
        let now_local = now_utc.with_timezone(&tz);
        let next_local_midnight = now_local
            .date_naive()
            .succ_opt()
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let target_local_dt = tz.from_local_datetime(&next_local_midnight).unwrap();
        let wait_duration = (target_local_dt - now_local)
            .to_std()
            .unwrap_or(std::time::Duration::from_secs(60));

        let sleep = tokio::time::sleep(wait_duration);
        tokio::pin!(sleep);
        let _ = sleep.as_mut().await;

        // Perform rotation: stop all current recordings and start new ones under the new date path
        if let Err(e) = perform_daily_rotation(manager.clone(), cfg.clone()).await {
            tracing::error!("[recorder] daily rotation failed: {}", e);
        }
    }
}

#[cfg(feature = "recorder")]
async fn perform_daily_rotation(
    manager: Arc<Manager>,
    cfg: Arc<RecorderConfig>,
) -> anyhow::Result<()> {
    // Collect current recording streams
    let streams: Vec<String> = {
        let map = TASKS.read().await;
        map.keys().cloned().collect()
    };

    if streams.is_empty() {
        return Ok(());
    }

    tracing::info!(
        "[recorder] performing daily rotation for {} streams",
        streams.len()
    );

    // Stop all
    for s in &streams {
        let _ = stop(s.clone()).await;
    }

    // Compute date path according to configured timezone
    let offset_minutes = cfg.rotate_tz_offset_minutes;
    let tz =
        FixedOffset::east_opt(offset_minutes * 60).unwrap_or(FixedOffset::east_opt(0).unwrap());
    let now_local = Utc::now().with_timezone(&tz);
    let date_path = now_local.format("%Y/%m/%d").to_string();

    // Start all again with tz-based date path: <stream>/<YYYY>/<MM>/<DD>
    for s in streams {
        if !should_record(&cfg.auto_streams, &s) {
            // If the stream was manually started and not in auto_streams, still restart to preserve behavior
            // Fall through to restart
        }
        let base_dir = Some(format!("{}/{}", s, date_path));
        if let Err(e) = start(manager.clone(), s.clone(), base_dir).await {
            tracing::error!("[recorder] failed to restart recording for {}: {}", s, e);
        }
    }

    Ok(())
}
