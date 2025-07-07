use glob::Pattern;
use once_cell::sync::Lazy;
use opendal::services::Fs;
use opendal::Operator;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::hook::{Event, StreamEventType};
use crate::stream::manager::Manager;

#[cfg(feature = "recorder")]
use crate::config::RecorderConfig;

mod rtp_h264;
mod rtp_opus;
mod segmenter;
mod task;
use task::RecordingTask;
mod fmp4;

static TASKS: Lazy<RwLock<HashMap<String, RecordingTask>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

static STORAGE: Lazy<RwLock<Option<Operator>>> = Lazy::new(|| RwLock::new(None));

/// Initialize recorder event listener.
#[cfg(feature = "recorder")]
pub async fn init(manager: Arc<Manager>, cfg: RecorderConfig) {
    let manager_clone = manager.clone();

    // Initialize storage Operator
    {
        let mut storage_writer = STORAGE.write().await;
        if storage_writer.is_none() {
            // Currently supports local filesystem only; can extend to other backends based on URI
            let root_path = cfg.root.trim_start_matches("file://");
            let builder = Fs::default().root(root_path);
            match Operator::new(builder) {
                Ok(op) => {
                    *storage_writer = Some(op.finish());
                }
                Err(e) => {
                    tracing::error!("[recorder] init storage error: {}", e);
                }
            }
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
