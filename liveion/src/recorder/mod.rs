use glob::Pattern;
use once_cell::sync::Lazy;
use opendal::services::{Fs, S3};
use opendal::Operator;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;

use crate::hook::{Event, StreamEventType};
use crate::stream::manager::Manager;

#[cfg(feature = "recorder")]
use crate::config::RecorderConfig;

mod segmenter;
mod task;
use task::RecordingTask;
pub mod codec;
mod fmp4;

static TASKS: Lazy<RwLock<HashMap<String, RecordingTask>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

static STORAGE: Lazy<RwLock<Option<Operator>>> = Lazy::new(|| RwLock::new(None));

/// Create storage operator based on URI scheme
pub fn create_storage_operator(uri: &str) -> anyhow::Result<Operator> {
    let url = Url::parse(uri)?;

    match url.scheme() {
        "file" => {
            let root_path = if uri.starts_with("file://./") {
                // Relative path: file://./path -> ./path
                &uri[9..]
            } else if uri.starts_with("file:///") {
                // Absolute path: file:///path -> /path
                url.path()
            } else if uri.starts_with("file://") {
                // Other cases, strip prefix: file://path -> path
                &uri[7..]
            } else {
                url.path()
            };
            let builder = Fs::default().root(root_path);
            Ok(Operator::new(builder)?.finish())
        }
        "s3" => {
            let bucket = url
                .host_str()
                .ok_or_else(|| anyhow::anyhow!("S3 URI must contain bucket name"))?;

            let mut builder = S3::default()
                .bucket(bucket)
                .root(url.path().trim_start_matches('/'));

            // Parse query parameters for S3 configuration
            for (key, value) in url.query_pairs() {
                match key.as_ref() {
                    "region" => {
                        builder = builder.region(&value);
                    }
                    "access_key_id" => {
                        builder = builder.access_key_id(&value);
                    }
                    "secret_access_key" => {
                        builder = builder.secret_access_key(&value);
                    }
                    "endpoint" => {
                        builder = builder.endpoint(&value);
                    }
                    "disable_config_load" => {
                        if value == "true" {
                            builder = builder.disable_config_load();
                        }
                    }
                    _ => {
                        tracing::warn!("[recorder] unknown S3 parameter: {}", key);
                    }
                }
            }

            Ok(Operator::new(builder)?.finish())
        }
        scheme => Err(anyhow::anyhow!("Unsupported storage scheme: {}", scheme)),
    }
}

/// Initialize recorder event listener.
#[cfg(feature = "recorder")]
pub async fn init(manager: Arc<Manager>, cfg: RecorderConfig) {
    let manager_clone = manager.clone();

    // Initialize storage Operator
    {
        let mut storage_writer = STORAGE.write().await;
        if storage_writer.is_none() {
            match create_storage_operator(&cfg.root) {
                Ok(op) => {
                    *storage_writer = Some(op);
                    tracing::info!("[recorder] initialized storage with URI: {}", cfg.root);
                }
                Err(e) => {
                    tracing::error!(
                        "[recorder] failed to initialize storage with URI {}: {}",
                        cfg.root,
                        e
                    );
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
