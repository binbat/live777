use std::collections::HashSet;
use std::net::SocketAddr;

use anyhow::Result;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::Query;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

mod log;
mod utils;

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
struct Config {
    #[serde(default)]
    http: Http,
    #[serde(default)]
    log: Log,
    #[serde(default)]
    playback: Playback,
    #[serde(default = "default_index_path")]
    index_path: String,
    #[serde(default)]
    storage: storage::StorageConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Http {
    #[serde(default = "default_http_listen")]
    listen: SocketAddr,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            listen: default_http_listen(),
        }
    }
}

fn default_http_listen() -> SocketAddr {
    "0.0.0.0:8899".parse().expect("invalid listen address")
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Log {
    #[serde(default = "default_log_level")]
    level: String,
}

impl Default for Log {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
}

fn default_log_level() -> String {
    std::env::var("LOG_LEVEL").unwrap_or_else(|_| {
        if cfg!(debug_assertions) {
            "debug".to_string()
        } else {
            "info".to_string()
        }
    })
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Playback {
    #[serde(default)]
    signed_redirect: bool,
    #[serde(default = "default_signed_ttl_seconds")]
    signed_ttl_seconds: u64,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            signed_redirect: false,
            signed_ttl_seconds: default_signed_ttl_seconds(),
        }
    }
}

fn default_signed_ttl_seconds() -> u64 {
    60
}

fn default_index_path() -> String {
    "./recordings/index.json".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RecordingIndexEntry {
    record: String,
    stream: String,
    record_dir: String,
    mpd_path: String,
    start_ts: i64,
    end_ts: Option<i64>,
    duration_ms: Option<i32>,
    status: api::recorder::RecordingStatus,
    node_alias: Option<String>,
    updated_at: i64,
}

#[derive(Clone)]
struct AppState {
    config: Config,
    operator: opendal::Operator,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::path::Path::new("livevod.toml");
    let cfg: Config = if path.try_exists()? {
        toml::from_str(std::fs::read_to_string(path)?.as_str())?
    } else {
        eprintln!("=== No any config file, use default config ===");
        Default::default()
    };

    log::set(format!("livevod={}", cfg.log.level));
    warn!("set log level : {}", cfg.log.level);
    debug!("config : {:?}", cfg);

    let operator = storage::init_operator(&cfg.storage)
        .await
        .expect("failed to init storage operator");

    let state = AppState {
        config: cfg.clone(),
        operator,
    };

    let app = Router::new()
        .route("/api/playback", get(list_streams))
        .route("/api/playback/{stream}", get(list_records))
        .route("/api/playback/{stream}/at", get(find_record_at))
        .route("/api/record/object/{*path}", get(get_object))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cfg.http.listen).await?;
    let addr = listener.local_addr()?;
    info!("LiveVOD listening on {}", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(utils::shutdown_signal())
        .await?;

    Ok(())
}

async fn list_streams(State(state): State<AppState>) -> Result<Json<Vec<String>>, Response> {
    let entries = load_index(&state.config.index_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to load index: {e}"),
        )
            .into_response()
    })?;
    let mut streams = HashSet::new();
    for entry in entries {
        streams.insert(entry.stream);
    }
    let mut list: Vec<String> = streams.into_iter().collect();
    list.sort();
    Ok(Json(list))
}

async fn list_records(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> Result<Json<Vec<RecordingIndexEntry>>, Response> {
    let entries = load_index(&state.config.index_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to load index: {e}"),
        )
            .into_response()
    })?;
    let mut records: Vec<RecordingIndexEntry> = entries
        .into_iter()
        .filter(|entry| entry.stream == stream)
        .collect();
    records.sort_by(|a, b| a.record.cmp(&b.record));
    Ok(Json(records))
}

#[derive(Deserialize)]
struct TimeQuery {
    ts: i64,
}

async fn find_record_at(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Query(query): Query<TimeQuery>,
) -> Result<Json<RecordingIndexEntry>, Response> {
    let ts_micros = normalize_ts_to_micros(query.ts);
    let entries = load_index(&state.config.index_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to load index: {e}"),
        )
            .into_response()
    })?;

    let record = entries.into_iter().find(|entry| {
        if entry.stream != stream {
            return false;
        }
        let start = entry.start_ts;
        let end = entry
            .end_ts
            .or_else(|| entry.duration_ms.map(|d| start + (d as i64) * 1000));
        match end {
            Some(end) => ts_micros >= start && ts_micros <= end,
            None => ts_micros >= start,
        }
    });

    match record {
        Some(record) => Ok(Json(record)),
        None => Err((StatusCode::NOT_FOUND, "record not found").into_response()),
    }
}

async fn get_object(
    State(state): State<AppState>,
    Path(path): Path<String>,
) -> Result<Response, Response> {
    let is_mpd = path.ends_with(".mpd");

    if !is_mpd && state.config.playback.signed_redirect {
        let ttl = std::time::Duration::from_secs(state.config.playback.signed_ttl_seconds.max(1));
        match state.operator.presign_read(&path, ttl).await {
            Ok(req) => {
                let uri = req.uri().to_string();
                return Ok(
                    (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, uri)]).into_response()
                );
            }
            Err(e) => {
                tracing::error!("presign read failed for '{}': {}", path, e);
            }
        }
    }

    match state.operator.read(&path).await {
        Ok(bytes) => {
            let content_type = if path.ends_with(".mpd") {
                "application/dash+xml"
            } else if path.ends_with(".m4s") || path.ends_with(".mp4") {
                if path.contains("audio_") {
                    "audio/mp4"
                } else {
                    "video/mp4"
                }
            } else {
                "application/octet-stream"
            };
            Ok((
                StatusCode::OK,
                [("content-type", content_type)],
                bytes.to_vec(),
            )
                .into_response())
        }
        Err(e) => {
            tracing::error!("failed to read object '{}': {}", path, e);
            Err((StatusCode::NOT_FOUND, "object not found").into_response())
        }
    }
}

async fn load_index(path: &str) -> Result<Vec<RecordingIndexEntry>> {
    let content = tokio::fs::read_to_string(path).await.unwrap_or_default();
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if trimmed.starts_with('[') {
        let entries: Vec<RecordingIndexEntry> = serde_json::from_str(trimmed)?;
        return Ok(entries);
    }

    let mut entries = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: RecordingIndexEntry = serde_json::from_str(line)?;
        entries.push(entry);
    }
    Ok(entries)
}

fn normalize_ts_to_micros(ts: i64) -> i64 {
    if ts > 1_000_000_000_000_000 {
        ts
    } else if ts > 1_000_000_000_000 {
        ts * 1000
    } else {
        ts * 1_000_000
    }
}
