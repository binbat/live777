use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use http::StatusCode;
#[cfg(feature = "recorder")]
use http::header;

use crate::AppState;
#[cfg(feature = "recorder")]
use crate::error::AppError;

pub fn route() -> Router<AppState> {
    Router::new()
        .route(
            &api::path::record("{stream}"),
            post(record_stream).get(record_status).delete(stop_record),
        )
        .route(
            api::path::recordings(),
            get(pull_recordings)
                .patch(ack_recordings)
                .delete(delete_recordings),
        )
        .route("/api/playback", get(list_playback_streams))
        .route("/api/playback/{stream}", get(list_playback_entries))
        .route("/api/record/object/{*path}", get(get_record_object))
}
#[cfg(feature = "recorder")]
async fn record_stream(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(body): Json<api::recorder::StartRecordRequest>,
) -> crate::result::Result<Response<String>> {
    let base_dir = body.base_dir.clone();
    let recording = crate::recorder::start(
        state.stream_manager.clone(),
        stream.clone(),
        base_dir.clone(),
    )
    .await?;

    let mpd_path = format!("{}/manifest.mpd", recording.record_dir);
    let record_id_str = if recording.record_id > 0 {
        recording.record_id.to_string()
    } else {
        let ts = (recording.start_ts_micros / 1_000_000).max(0);
        ts.to_string()
    };
    let resp = api::recorder::StartRecordResponse {
        id: stream.clone(),
        record_id: record_id_str,
        record_dir: recording.record_dir,
        mpd_path,
    };
    match serde_json::to_string(&resp) {
        Ok(json_body) => Ok(Response::builder().status(StatusCode::OK).body(json_body)?),
        Err(e) => Err(AppError::InternalServerError(anyhow::anyhow!(
            "Failed to serialize response: {}",
            e
        ))),
    }
}

#[cfg(not(feature = "recorder"))]
async fn record_stream(
    _state: State<AppState>,
    Path(_stream): Path<String>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn record_status(
    State(_state): State<AppState>,
    Path(stream): Path<String>,
) -> crate::result::Result<Json<serde_json::Value>> {
    let recording = crate::recorder::is_recording(&stream).await;
    Ok(Json(serde_json::json!({ "recording": recording })))
}

#[cfg(not(feature = "recorder"))]
async fn record_status(
    _state: State<AppState>,
    Path(_stream): Path<String>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn stop_record(
    State(_state): State<AppState>,
    Path(stream): Path<String>,
) -> crate::result::Result<Response<String>> {
    crate::recorder::stop(stream.clone()).await?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .body("".to_string())?)
}

#[cfg(not(feature = "recorder"))]
async fn stop_record(
    _state: State<AppState>,
    Path(_stream): Path<String>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn pull_recordings(
    Query(req): Query<api::recorder::PullRecordingsRequest>,
) -> crate::result::Result<Json<api::recorder::PullRecordingsResponse>> {
    let resp = crate::recorder::pull_recordings(req).await?;
    Ok(Json(resp))
}

#[cfg(not(feature = "recorder"))]
async fn pull_recordings(
    Query(_req): Query<api::recorder::PullRecordingsRequest>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn ack_recordings(
    Json(req): Json<api::recorder::AckRecordingsRequest>,
) -> crate::result::Result<Json<api::recorder::AckRecordingsResponse>> {
    let resp = crate::recorder::ack_recordings(req).await?;
    Ok(Json(resp))
}

#[cfg(not(feature = "recorder"))]
async fn ack_recordings(
    Json(_req): Json<api::recorder::AckRecordingsRequest>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn delete_recordings(
    Json(req): Json<api::recorder::DeleteRecordingsRequest>,
) -> crate::result::Result<Json<api::recorder::DeleteRecordingsResponse>> {
    let resp = crate::recorder::delete_recordings(req).await?;
    Ok(Json(resp))
}

#[cfg(not(feature = "recorder"))]
async fn delete_recordings(
    Json(_req): Json<api::recorder::DeleteRecordingsRequest>,
) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn list_playback_streams() -> crate::result::Result<Json<Vec<String>>> {
    Ok(Json(crate::recorder::list_playback_streams().await?))
}

#[cfg(not(feature = "recorder"))]
async fn list_playback_streams() -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn list_playback_entries(
    Path(stream): Path<String>,
) -> crate::result::Result<Json<Vec<crate::recorder::PlaybackIndexEntry>>> {
    Ok(Json(crate::recorder::list_playback_entries(&stream).await?))
}

#[cfg(not(feature = "recorder"))]
async fn list_playback_entries(Path(_stream): Path<String>) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(feature = "recorder")]
async fn get_record_object(Path(path): Path<String>) -> crate::result::Result<Response> {
    let Some(bytes) = crate::recorder::read_object(&path).await? else {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            "Recorder storage is not available",
        )
            .into_response());
    };

    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, record_object_content_type(&path))],
        bytes,
    )
        .into_response())
}

#[cfg(not(feature = "recorder"))]
async fn get_record_object(Path(_path): Path<String>) -> crate::result::Result<Response> {
    Ok(recorder_not_enabled())
}

#[cfg(not(feature = "recorder"))]
fn recorder_not_enabled() -> Response {
    (StatusCode::NOT_IMPLEMENTED, "feature recorder not enabled").into_response()
}

#[cfg(feature = "recorder")]
fn record_object_content_type(path: &str) -> &'static str {
    if path.ends_with(".mpd") {
        "application/dash+xml"
    } else if path.ends_with(".m4s") || path.ends_with(".mp4") {
        if path.contains("audio_") {
            "audio/mp4"
        } else {
            "video/mp4"
        }
    } else {
        "application/octet-stream"
    }
}
