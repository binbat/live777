use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};

use crate::route::AppState;
use api::recorder::{PullRecordingsRequest, PullRecordingsResponse};

pub fn router() -> Router<AppState> {
    Router::new().route(api::path::recordings_pull(), get(pull_recordings))
}

/// Pull recording sessions endpoint for Liveman
async fn pull_recordings(
    Query(req): Query<PullRecordingsRequest>,
    State(_state): State<AppState>,
) -> Result<Json<PullRecordingsResponse>, StatusCode> {
    let response =
        crate::recorder::pull_recordings(req.stream.as_deref(), req.since_ts, req.limit).await;

    Ok(Json(response))
}
