use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use api::recorder::RecordingStatus;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, RwLock};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordingIndexEntry {
    pub record: String,
    pub stream: String,
    pub record_dir: String,
    pub mpd_path: String,
    pub start_ts: i64,
    pub end_ts: Option<i64>,
    pub duration_ms: Option<i32>,
    pub status: RecordingStatus,
    pub node_alias: Option<String>,
    pub updated_at: i64,
}

impl RecordingIndexEntry {
    pub fn key(&self) -> String {
        format!("{}/{}", self.stream, self.record)
    }
}

pub struct RecordingsIndex {
    path: PathBuf,
    entries: RwLock<HashMap<String, RecordingIndexEntry>>,
    write_lock: Mutex<()>,
}

impl RecordingsIndex {
    pub async fn load(path: PathBuf) -> Result<Self> {
        let mut entries = HashMap::new();
        if let Ok(content) = tokio::fs::read_to_string(&path).await
            && !content.trim().is_empty()
        {
            let parsed: Vec<RecordingIndexEntry> = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse index file: {}", path.display()))?;
            for entry in parsed {
                entries.insert(entry.key(), entry);
            }
        }

        Ok(Self {
            path,
            entries: RwLock::new(entries),
            write_lock: Mutex::new(()),
        })
    }

    pub async fn upsert(&self, entry: RecordingIndexEntry) -> Result<()> {
        {
            let mut map = self.entries.write().await;
            map.insert(entry.key(), entry);
        }
        self.persist().await
    }

    pub async fn update_status(
        &self,
        stream: &str,
        record: &str,
        status: RecordingStatus,
        end_ts: Option<i64>,
        duration_ms: Option<i32>,
    ) -> Result<()> {
        {
            let mut map = self.entries.write().await;
            let key = format!("{}/{}", stream, record);
            if let Some(entry) = map.get_mut(&key) {
                entry.status = status;
                entry.end_ts = end_ts;
                entry.duration_ms = duration_ms;
                entry.updated_at = Utc::now().timestamp_micros();
            }
        }
        self.persist().await
    }

    pub async fn persist(&self) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let entries = {
            let map = self.entries.read().await;
            let mut values: Vec<RecordingIndexEntry> = map.values().cloned().collect();
            values.sort_by(|a, b| a.stream.cmp(&b.stream).then(a.record.cmp(&b.record)));
            values
        };

        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let tmp_path = tmp_path_for(&self.path);
        let json = serde_json::to_string_pretty(&entries)?;
        tokio::fs::write(&tmp_path, json).await?;

        if tokio::fs::metadata(&self.path).await.is_ok() {
            let _ = tokio::fs::remove_file(&self.path).await;
        }
        tokio::fs::rename(&tmp_path, &self.path)
            .await
            .with_context(|| format!("Failed to replace index file {}", self.path.display()))?;
        Ok(())
    }
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    if let Some(ext) = path.extension() {
        let mut ext = ext.to_os_string();
        ext.push(".tmp");
        tmp.set_extension(ext);
    } else {
        tmp.set_extension("tmp");
    }
    tmp
}
