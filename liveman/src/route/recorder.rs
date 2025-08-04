use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tracing::{error, info};

use crate::service::recording_sessions::{RecordingQueryParams, RecordingSessionsService};
use crate::{error::AppError, result::Result, AppState};
use api::recorder::{RecordingSessionResponse, StreamsListResponse};

#[derive(Deserialize)]
struct RecordingQuery {
    stream: Option<String>,
    status: Option<String>,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
    limit: Option<u64>,
    offset: Option<u64>,
}

pub fn route() -> Router<AppState> {
    Router::new()
        .route("/api/record/streams", get(get_streams))
        .route("/api/record/sessions", get(get_sessions))
        .route("/api/record/sessions/{id}/mpd", get(get_session_mpd))
        .route("/api/record/object/{*path}", get(get_segment))
}

async fn get_streams(State(state): State<AppState>) -> Result<Json<StreamsListResponse>> {
    match RecordingSessionsService::get_streams(state.database.get_connection()).await {
        Ok(streams) => {
            info!("Retrieved {} recorded streams", streams.len());
            Ok(Json(StreamsListResponse { streams }))
        }
        Err(e) => {
            error!("Failed to retrieve recorded streams: {}", e);
            Err(AppError::DatabaseError(e.to_string()))
        }
    }
}

async fn get_sessions(
    State(state): State<AppState>,
    Query(query): Query<RecordingQuery>,
) -> Result<Json<RecordingSessionResponse>> {
    let params = RecordingQueryParams {
        stream: query.stream,
        status: query.status,
        start_ts: query.start_ts,
        end_ts: query.end_ts,
        limit: query.limit,
        offset: query.offset,
    };

    match RecordingSessionsService::get_recordings(state.database.get_connection(), params).await {
        Ok(sessions) => {
            let sessions_response: Vec<api::recorder::RecordingSession> = sessions
                .into_iter()
                .map(|session| api::recorder::RecordingSession {
                    id: Some(session.id.to_string()),
                    stream: session.stream,
                    start_ts: session.start_ts,
                    end_ts: session.end_ts,
                    duration_ms: session.duration_ms,
                    mpd_path: session.mpd_path,
                    status: session
                        .status
                        .parse()
                        .unwrap_or(api::recorder::RecordingStatus::Active),
                })
                .collect();

            let total_count = sessions_response.len() as u64;

            info!("Retrieved {} recording sessions", total_count);

            Ok(Json(RecordingSessionResponse {
                sessions: sessions_response,
                total_count,
            }))
        }
        Err(e) => {
            error!("Failed to retrieve recording sessions: {}", e);
            Err(AppError::DatabaseError(e.to_string()))
        }
    }
}

async fn get_session_mpd(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Response> {
    let session_uuid = session_id
        .parse()
        .map_err(|_| AppError::RequestProxyError)?;

    match RecordingSessionsService::get_recording_by_id(
        state.database.get_connection(),
        session_uuid,
    )
    .await
    {
        Ok(Some(session)) => {
            let _mpd_path = &session.mpd_path;

            #[cfg(feature = "recorder")]
            {
                if let Some(ref operator) = state.file_storage {
                    match operator.read(&session.mpd_path).await {
                        Ok(bytes) => {
                            let mut mpd_content =
                                String::from_utf8_lossy(&bytes.to_vec()).to_string();

                            // Extract directory path from mpd_path
                            let mpd_dir = session
                                .mpd_path
                                .rsplit('/')
                                .skip(1)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect::<Vec<_>>()
                                .join("/");
                            let base_url = format!("/api/record/object/{}/", mpd_dir);

                            // Add BaseURL element to MPD if not already present
                            if !mpd_content.contains("<BaseURL>") {
                                // Find the first Period element and insert BaseURL before it
                                if let Some(period_pos) = mpd_content.find("<Period") {
                                    let base_url_element =
                                        format!("    <BaseURL>{}</BaseURL>\n    ", base_url);
                                    mpd_content.insert_str(period_pos, &base_url_element);
                                }
                            }

                            info!("Successfully served MPD with BaseURL: {}", session.mpd_path);
                            Ok((
                                StatusCode::OK,
                                [("content-type", "application/dash+xml")],
                                mpd_content.into_bytes(),
                            )
                                .into_response())
                        }
                        Err(e) => {
                            error!("Failed to read MPD file '{}': {}", session.mpd_path, e);
                            Ok((StatusCode::NOT_FOUND, "MPD file not found").into_response())
                        }
                    }
                } else {
                    error!("File storage not configured for MPD access");
                    Ok((
                        StatusCode::SERVICE_UNAVAILABLE,
                        "File storage not available",
                    )
                        .into_response())
                }
            }

            #[cfg(not(feature = "recorder"))]
            {
                let _ = state;
                Ok((StatusCode::NOT_IMPLEMENTED, "Recorder feature not enabled").into_response())
            }
        }
        Ok(None) => Ok((StatusCode::NOT_FOUND, "Recording session not found").into_response()),
        Err(e) => {
            error!("Failed to retrieve recording session: {}", e);
            Err(AppError::DatabaseError(e.to_string()))
        }
    }
}

async fn get_segment(State(state): State<AppState>, Path(path): Path<String>) -> Result<Response> {
    #[cfg(feature = "recorder")]
    {
        if let Some(ref operator) = state.file_storage {
            match operator.read(&path).await {
                Ok(bytes) => {
                    info!("Successfully served segment: {}", path);

                    // Determine content type based on file extension
                    let content_type = if path.ends_with(".m4s") || path.ends_with(".mp4") {
                        "video/mp4"
                    } else if path.ends_with(".mpd") {
                        "application/dash+xml"
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
                    error!("Failed to read segment file '{}': {}", path, e);
                    Ok((StatusCode::NOT_FOUND, "Segment not found").into_response())
                }
            }
        } else {
            error!("File storage not configured for segment access");
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
