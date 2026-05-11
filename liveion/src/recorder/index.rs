use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use api::recorder::{
    AckRecordingsRequest, DeleteRecordingsRequest, RecordingKey, RecordingSession, RecordingStatus,
};
use chrono::Utc;
use fs2::FileExt;
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
    write_count: AtomicUsize,
}

impl RecordingsIndex {
    pub async fn load(path: PathBuf) -> Result<Self> {
        let mut entries = HashMap::new();
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                if trimmed.starts_with('[') {
                    let parsed: Vec<RecordingIndexEntry> = serde_json::from_str(trimmed)
                        .with_context(|| {
                            format!("Failed to parse index file: {}", path.display())
                        })?;
                    for entry in parsed {
                        entries.insert(entry.key(), entry);
                    }
                } else {
                    for line in trimmed.lines() {
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        let entry: RecordingIndexEntry =
                            serde_json::from_str(line).with_context(|| {
                                format!("Failed to parse index line in {}", path.display())
                            })?;
                        entries.insert(entry.key(), entry);
                    }
                }
            }
        }

        Ok(Self {
            path,
            entries: RwLock::new(entries),
            write_lock: Mutex::new(()),
            write_count: AtomicUsize::new(0),
        })
    }

    pub async fn upsert(&self, entry: RecordingIndexEntry) -> Result<()> {
        let to_append = entry.clone();
        {
            let mut map = self.entries.write().await;
            map.insert(entry.key(), entry);
        }
        self.append_entries_and_maybe_compact(vec![to_append]).await
    }

    pub async fn update_status(
        &self,
        stream: &str,
        record: &str,
        status: RecordingStatus,
        end_ts: Option<i64>,
        duration_ms: Option<i32>,
    ) -> Result<()> {
        let mut updated: Option<RecordingIndexEntry> = None;
        {
            let mut map = self.entries.write().await;
            let key = format!("{}/{}", stream, record);
            if let Some(entry) = map.get_mut(&key) {
                entry.status = status;
                entry.end_ts = end_ts;
                entry.duration_ms = duration_ms;
                entry.updated_at = Utc::now().timestamp_micros();
                updated = Some(entry.clone());
            }
        }
        if let Some(entry) = updated {
            self.append_entries_and_maybe_compact(vec![entry]).await?;
        }
        Ok(())
    }

    pub async fn list_sessions(
        &self,
        stream: Option<String>,
        since_ts: Option<i64>,
        limit: u32,
    ) -> (Vec<RecordingSession>, Option<i64>) {
        let limit = if limit == 0 { 100 } else { limit } as usize;
        let mut rows: Vec<RecordingIndexEntry> = {
            let map = self.entries.read().await;
            map.values().cloned().collect()
        };

        if let Some(stream) = stream.as_ref() {
            rows.retain(|r| &r.stream == stream);
        }

        if let Some(since) = since_ts {
            rows.retain(|r| r.updated_at > since);
        }

        rows.retain(|r| !matches!(r.status, RecordingStatus::Acked));
        rows.sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
        if rows.len() > limit {
            rows.truncate(limit);
        }

        let last_ts = rows.iter().map(|r| r.updated_at).max();
        let sessions = rows
            .into_iter()
            .map(|r| RecordingSession {
                id: Some(r.record.clone()),
                stream: r.stream,
                start_ts: r.start_ts,
                end_ts: r.end_ts,
                duration_ms: r.duration_ms,
                mpd_path: r.mpd_path,
                status: r.status,
            })
            .collect();

        (sessions, last_ts)
    }

    pub async fn list_streams(&self) -> Vec<String> {
        let mut streams: Vec<String> = {
            let map = self.entries.read().await;
            map.values().map(|entry| entry.stream.clone()).collect()
        };

        streams.sort();
        streams.dedup();
        streams
    }

    pub async fn list_playback_entries(
        &self,
        stream: &str,
    ) -> Vec<super::PlaybackIndexEntry> {
        let mut rows: Vec<RecordingIndexEntry> = {
            let map = self.entries.read().await;
            map.values()
                .filter(|entry| entry.stream == stream)
                .cloned()
                .collect()
        };

        rows.sort_by(|a, b| a.record.cmp(&b.record));
        rows.into_iter()
            .map(|entry| super::PlaybackIndexEntry {
                record: entry.record,
                mpd_path: entry.mpd_path,
            })
            .collect()
    }

    pub async fn ack(&self, req: AckRecordingsRequest) -> Result<usize> {
        let mut acked = 0usize;
        let records = req.records;
        {
            let mut map = self.entries.write().await;
            for RecordingKey { stream, record } in &records {
                let key = format!("{}/{}", stream, record);
                if let Some(entry) = map.get_mut(&key) {
                    entry.status = RecordingStatus::Acked;
                    entry.updated_at = Utc::now().timestamp_micros();
                    acked += 1;
                }
            }
        }

        if acked > 0 {
            let entries = {
                let map = self.entries.read().await;
                records
                    .iter()
                    .filter_map(|key| map.get(&format!("{}/{}", key.stream, key.record)).cloned())
                    .collect::<Vec<_>>()
            };
            if !entries.is_empty() {
                self.append_entries_and_maybe_compact(entries).await?;
            }
        }

        Ok(acked)
    }

    pub async fn delete_acked(&self, req: DeleteRecordingsRequest) -> Result<usize> {
        let mut removed = 0usize;
        {
            let mut map = self.entries.write().await;
            for RecordingKey { stream, record } in req.records {
                let key = format!("{}/{}", stream, record);
                if let Some(entry) = map.get(&key)
                    && matches!(entry.status, RecordingStatus::Acked)
                {
                    map.remove(&key);
                    removed += 1;
                }
            }
        }

        if removed > 0 {
            self.compact().await?;
        }

        Ok(removed)
    }

    async fn append_entries_and_maybe_compact(
        &self,
        entries: Vec<RecordingIndexEntry>,
    ) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let _guard = self.write_lock.lock().await;
        self.append_entries(entries.clone()).await?;

        let count = self.write_count.fetch_add(entries.len(), Ordering::Relaxed) + entries.len();
        if count.is_multiple_of(200) {
            self.compact().await?;
        }
        Ok(())
    }

    async fn append_entries(&self, entries: Vec<RecordingIndexEntry>) -> Result<()> {
        let path = self.path.clone();
        let lines: Vec<String> = entries
            .into_iter()
            .map(|entry| serde_json::to_string(&entry))
            .collect::<Result<Vec<_>, _>>()?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let _lock = lock_file(&path)?;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?;
            for line in lines {
                writeln!(file, "{}", line)?;
            }
            file.sync_data()?;
            sync_parent_dir(&path)?;
            Ok(())
        })
        .await??;
        Ok(())
    }

    async fn compact(&self) -> Result<()> {
        let entries = {
            let map = self.entries.read().await;
            let mut values: Vec<RecordingIndexEntry> = map.values().cloned().collect();
            values.sort_by(|a, b| a.stream.cmp(&b.stream).then(a.record.cmp(&b.record)));
            values
        };
        self.compact_with_entries(entries).await
    }

    async fn compact_with_entries(&self, entries: Vec<RecordingIndexEntry>) -> Result<()> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let _lock = lock_file(&path)?;
            let tmp_path = tmp_path_for(&path);
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&tmp_path)?;
            for entry in entries {
                let line = serde_json::to_string(&entry)?;
                writeln!(file, "{}", line)?;
            }
            file.sync_data()?;
            if std::fs::metadata(&path).is_ok() {
                let _ = std::fs::remove_file(&path);
            }
            std::fs::rename(&tmp_path, &path)
                .with_context(|| format!("Failed to replace index file {}", path.display()))?;
            sync_parent_dir(&path)?;
            Ok(())
        })
        .await??;
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

fn lock_file(path: &Path) -> Result<std::fs::File> {
    let lock_path = lock_path_for(path);
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&lock_path)?;
    file.lock_exclusive()?;
    Ok(file)
}

fn lock_path_for(path: &Path) -> PathBuf {
    let mut lock_path = path.to_path_buf();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("index.json");
    lock_path.set_file_name(format!("{}.lock", name));
    lock_path
}

fn sync_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent()
        && let Ok(dir) = std::fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
    Ok(())
}
