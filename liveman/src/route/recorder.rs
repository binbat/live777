use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tracing::{error, info};

use crate::{result::Result, AppState};
use api::recorder::StreamsListResponse;

#[derive(Deserialize)]
struct RecordingQuery {}

pub fn route() -> Router<AppState> {
    Router::new()
        .route("/api/record/streams", get(get_streams))
        .route("/api/record/object/{*path}", get(get_segment))
}

async fn get_streams(State(_state): State<AppState>) -> Result<Json<StreamsListResponse>> {
    Ok(Json(StreamsListResponse { streams: vec![] }))
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
