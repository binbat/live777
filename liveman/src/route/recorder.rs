use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use axum_extra::extract::Query;
use http::header;

use crate::{AppState, result::Result};

pub fn route() -> Router<AppState> {
    Router::new()
        .route("/api/playback", get(list_index_streams))
        .route("/api/playback/{stream}", get(list_index_by_stream))
        .route(
            "/api/record/{stream}",
            post(start_record)
                .get(get_record_status)
                .delete(stop_record),
        )
        .route("/api/record/object/{*path}", get(get_segment))
}

async fn get_segment(State(state): State<AppState>, Path(path): Path<String>) -> Result<Response> {
    #[cfg(feature = "recorder")]
    {
        if let Some(ref operator) = state.file_storage {
            // Always proxy MPD manifest itself to keep relative segment URLs under our domain
            let is_mpd = path.ends_with(".mpd");

            // If enabled, try presigned redirect for non-MPD files (segments, init, etc.)
            if !is_mpd && state.config.playback.signed_redirect {
                use std::time::Duration as StdDuration;

                let ttl = StdDuration::from_secs(state.config.playback.signed_ttl_seconds.max(1));
                match operator.presign_read(&path, ttl).await {
                    Ok(req) => {
                        let uri = req.uri().to_string();
                        return Ok((StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, uri)])
                            .into_response());
                    }
                    Err(e) => {
                        tracing::error!("Presign read failed for '{}': {}", path, e);
                    }
                }
            }

            // Fallback: proxy bytes directly from storage
            match operator.read(&path).await {
                Ok(bytes) => {
                    tracing::info!("Successfully served segment: {}", path);

                    // Determine content type based on file extension
                    let content_type = if path.ends_with(".mpd") {
                        "application/dash+xml"
                    } else if path.ends_with(".m4s") || path.ends_with(".mp4") {
                        // Heuristic: audio segments and init named with "audio_" prefix
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
                    tracing::error!("Failed to read segment file '{}': {}", path, e);
                    Ok((StatusCode::NOT_FOUND, "Segment not found").into_response())
                }
            }
        } else {
            tracing::error!("File storage not configured for segment access");
            Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                "File storage not available",
            )
                .into_response())
        }
    }

    #[cfg(not(feature = "recorder"))]
    {
        // Avoid unused variable warnings
        let _ = state;
        let _ = path;
        Ok((StatusCode::NOT_IMPLEMENTED, "Recorder feature not enabled").into_response())
    }
}

#[derive(serde::Serialize)]
struct RecordingIndexEntry {
    year: i32,
    month: i32,
    day: i32,
    mpd_path: String,
}

async fn list_index_streams(State(state): State<AppState>) -> Result<Json<Vec<String>>> {
    use crate::entity::recordings::{self, Entity as Recordings};
    use sea_orm::{EntityTrait, QuerySelect};
    let db = state.database.get_connection();
    let streams: Vec<String> = Recordings::find()
        .select_only()
        .column(recordings::Column::Stream)
        .distinct()
        .into_tuple()
        .all(db)
        .await?;
    Ok(Json(streams))
}

async fn list_index_by_stream(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> Result<Json<Vec<RecordingIndexEntry>>> {
    use crate::entity::recordings::{self, Entity as Recordings};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let db = state.database.get_connection();
    let rows = Recordings::find()
        .filter(recordings::Column::Stream.eq(stream))
        .all(db)
        .await?;
    let entries = rows
        .into_iter()
        .map(|m| RecordingIndexEntry {
            year: m.year,
            month: m.month,
            day: m.day,
            mpd_path: m.mpd_path,
        })
        .collect();
    Ok(Json(entries))
}

// ---- Manual start & status proxy ----

#[derive(serde::Deserialize, Default)]
struct StartRecordQuery {
    node: Option<String>,
}

#[derive(serde::Serialize)]
struct StartRecordResponse {
    started: bool,
    mpd_path: String,
}

async fn start_record(
    State(mut state): State<AppState>,
    Path(stream): Path<String>,
    Query(q): Query<StartRecordQuery>,
) -> Result<Json<StartRecordResponse>> {
    // Choose target server
    let streams = state.storage.stream_all().await;
    let servers = state.storage.get_cluster();
    let target_server = if let Some(alias) = q.node.clone() {
        servers.into_iter().find(|s| s.alias == alias)
    } else if let Some(nodes) = streams.get(&stream) {
        let alias = nodes.first().cloned();
        alias.and_then(|a| state.storage.get_map_server().get(&a).cloned())
    } else {
        servers.first().cloned()
    };

    let server = target_server.ok_or(crate::error::AppError::NoAvailableNode)?;

    // Build base_dir using configured base_prefix + today
    let date_path = chrono::Utc::now().format("%Y/%m/%d").to_string();
    let base_prefix = state.config.auto_record.base_prefix.clone();
    let base_dir = if base_prefix.is_empty() {
        None
    } else {
        Some(format!("{base_prefix}/{date_path}"))
    };

    let body = api::recorder::StartRecordRequest { base_dir };
    let url = format!("{}{}", server.url, api::path::record(&stream));
    let resp = state
        .client
        .post(url)
        .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(crate::error::AppError::InternalServerError(
            anyhow::anyhow!("record start failed: {}", resp.status()),
        ));
    }

    let mpd_path = match resp.json::<api::recorder::StartRecordResponse>().await {
        Ok(v) => v.mpd_path,
        Err(_) => {
            if let Some(prefix) = &body.base_dir {
                format!("{prefix}/manifest.mpd")
            } else {
                format!("{stream}/{date_path}/manifest.mpd")
            }
        }
    };

    // Parse date from date_path and upsert index
    if let [y, m, d] = date_path.split('/').collect::<Vec<_>>()[..]
        && let (Ok(yy), Ok(mm), Ok(dd)) = (y.parse::<i32>(), m.parse::<i32>(), d.parse::<i32>())
    {
        let _ = crate::service::recordings_index::RecordingsIndexService::upsert(
            state.database.get_connection(),
            &stream,
            yy,
            mm,
            dd,
            &mpd_path,
        )
        .await;
    }

    Ok(Json(StartRecordResponse {
        started: true,
        mpd_path,
    }))
}

#[derive(serde::Serialize)]
struct RecordStatusResponse {
    recording: bool,
}

async fn get_record_status(
    State(mut state): State<AppState>,
    Path(stream): Path<String>,
) -> Result<Json<RecordStatusResponse>> {
    let streams = state.storage.stream_all().await;
    let map_server = state.storage.get_map_server();
    let mut recording = false;
    if let Some(nodes) = streams.get(&stream) {
        for alias in nodes {
            if let Some(server) = map_server.get(alias) {
                let url = format!("{}{}", server.url, api::path::record(&stream));
                if let Ok(resp) = state
                    .client
                    .get(url)
                    .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                    .send()
                    .await
                    && let Ok(v) = resp.json::<serde_json::Value>().await
                    && v.get("recording")
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false)
                {
                    recording = true;
                    break;
                }
            }
        }
    }
    Ok(Json(RecordStatusResponse { recording }))
}

async fn stop_record(
    State(mut state): State<AppState>,
    Path(stream): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let streams = state.storage.stream_all().await;
    let map_server = state.storage.get_map_server();
    let mut any_stopped = false;
    if let Some(nodes) = streams.get(&stream) {
        for alias in nodes {
            if let Some(server) = map_server.get(alias) {
                // check status first
                let record_url = format!("{}{}", server.url, api::path::record(&stream));
                let is_recording = match state
                    .client
                    .get(record_url.as_str())
                    .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                    .send()
                    .await
                {
                    Ok(resp) => match resp.json::<serde_json::Value>().await {
                        Ok(v) => v
                            .get("recording")
                            .and_then(|b| b.as_bool())
                            .unwrap_or(false),
                        Err(_) => false,
                    },
                    Err(_) => false,
                };
                if is_recording
                    && let Ok(resp) = state
                        .client
                        .delete(record_url)
                        .header(header::AUTHORIZATION, format!("Bearer {}", server.token))
                        .send()
                        .await
                    && resp.status().is_success()
                {
                    any_stopped = true;
                }
            }
        }
    }
    Ok(Json(serde_json::json!({ "stopped": any_stopped })))
}
