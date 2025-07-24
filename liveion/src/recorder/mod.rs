use glob::Pattern;
use once_cell::sync::Lazy;
use opendal::services::{Fs, S3};
use opendal::Operator;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::hook::{Event, StreamEventType};
use crate::stream::manager::Manager;

#[cfg(feature = "recorder")]
use crate::config::{RecorderConfig, StorageConfig};

mod segmenter;
mod task;
use task::RecordingTask;
pub mod codec;
mod fmp4;

static TASKS: Lazy<RwLock<HashMap<String, RecordingTask>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

static STORAGE: Lazy<RwLock<Option<Operator>>> = Lazy::new(|| RwLock::new(None));

/// Create storage operator based on storage configuration
pub fn create_storage_operator(config: &StorageConfig) -> anyhow::Result<Operator> {
    tracing::debug!(
        "[recorder] creating storage operator for config: {:?}",
        config
    );

    match config {
        StorageConfig::Fs { root } => {
            tracing::info!(
                "[recorder] configuring filesystem storage with root: {}",
                root
            );
            let builder = Fs::default().root(root);
            let op = Operator::new(builder)?.finish();
            tracing::debug!("[recorder] filesystem storage operator created successfully");
            Ok(op)
        }
        StorageConfig::S3 {
            bucket,
            root,
            region,
            endpoint,
            access_key_id,
            secret_access_key,
            session_token,
            disable_config_load,
            enable_virtual_host_style,
        } => {
            tracing::info!(
                "[recorder] configuring S3 storage with bucket: {}, region: {:?}",
                bucket,
                region
            );

            let mut builder = S3::default()
                .bucket(bucket)
                .root(root.trim_start_matches('/'));

            if let Some(region) = region {
                builder = builder.region(region);
                tracing::debug!("[recorder] S3 region set to: {}", region);
            }

            if let Some(endpoint) = endpoint {
                builder = builder.endpoint(endpoint);
                tracing::debug!("[recorder] S3 endpoint set to: {}", endpoint);
            }

            if let Some(access_key_id) = access_key_id {
                builder = builder.access_key_id(access_key_id);
                tracing::debug!("[recorder] S3 access key configured");
            }

            if let Some(secret_access_key) = secret_access_key {
                builder = builder.secret_access_key(secret_access_key);
                tracing::debug!("[recorder] S3 secret key configured");
            }

            if let Some(session_token) = session_token {
                builder = builder.session_token(session_token);
                tracing::debug!("[recorder] S3 session token configured");
            }

            if *disable_config_load {
                builder = builder.disable_config_load();
                tracing::debug!("[recorder] S3 config load disabled");
            }

            if *enable_virtual_host_style {
                builder = builder.enable_virtual_host_style();
                tracing::debug!("[recorder] S3 virtual host style enabled");
            }

            let op = Operator::new(builder)?.finish();
            tracing::debug!("[recorder] S3 storage operator created successfully");
            Ok(op)
        }
        StorageConfig::Oss {
            bucket,
            root,
            region,
            endpoint,
            access_key_id,
            access_key_secret,
            security_token,
        } => {
            tracing::info!(
                "[recorder] configuring OSS storage with bucket: {}, region: {}",
                bucket,
                region
            );

            // Use S3 service for OSS compatibility
            let mut builder = S3::default()
                .bucket(bucket)
                .root(root.trim_start_matches('/'))
                .region(region)
                .endpoint(endpoint)
                .enable_virtual_host_style();

            if let Some(access_key_id) = access_key_id {
                builder = builder.access_key_id(access_key_id);
                tracing::debug!("[recorder] OSS access key configured");
            }

            if let Some(access_key_secret) = access_key_secret {
                builder = builder.secret_access_key(access_key_secret);
                tracing::debug!("[recorder] OSS secret key configured");
            }

            if let Some(security_token) = security_token {
                builder = builder.session_token(security_token);
                tracing::debug!("[recorder] OSS security token configured");
            }

            let op = Operator::new(builder)?.finish();
            tracing::debug!("[recorder] OSS storage operator created successfully");
            Ok(op)
        }
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
            tracing::info!(
                "[recorder] initializing storage operator with config: {:?}",
                cfg.storage
            );
            match create_storage_operator(&cfg.storage) {
                Ok(op) => {
                    // Test the storage connection with a simple operation
                    match op.check().await {
                        Ok(_) => {
                            *storage_writer = Some(op);
                            tracing::info!(
                                "[recorder] storage backend initialized and verified: {:?}",
                                cfg.storage
                            );
                        }
                        Err(e) => {
                            tracing::warn!("[recorder] storage backend initialized but connection test failed: {}, continuing anyway", e);
                            *storage_writer = Some(op);
                        }
                    }
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
