use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};

#[cfg(feature = "recorder")]
use http::StatusCode;

use crate::AppState;
use crate::error::AppError;

pub fn route() -> Router<AppState> {
    Router::new().route(
        &api::path::record("{stream}"),
        post(record_stream).get(record_status).delete(stop_record),
    )
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
    let resp = api::recorder::StartRecordResponse {
        id: stream.clone(),
        record_id: recording.record_id.to_string(),
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
) -> crate::result::Result<Response<String>> {
    Err(AppError::Throw("feature recorder not enabled".into()))
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
    _path: Path<String>,
) -> crate::result::Result<Json<serde_json::Value>> {
    Err(AppError::Throw("feature recorder not enabled".into()))
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
) -> crate::result::Result<Response<String>> {
    Err(AppError::Throw("feature recorder not enabled".into()))
}
