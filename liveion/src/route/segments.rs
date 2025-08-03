use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};

use crate::route::AppState;
use api::recorder::{PullSegmentsRequest, PullSegmentsResponse};

pub fn router() -> Router<AppState> {
    Router::new().route(api::path::segments_pull(), get(pull_segments))
}

/// Pull segments endpoint for Liveman
async fn pull_segments(
    Query(req): Query<PullSegmentsRequest>,
    State(_state): State<AppState>,
) -> Result<Json<PullSegmentsResponse>, StatusCode> {
    let response =
        crate::recorder::pull_segments(req.stream.as_deref(), req.since_ts, req.limit).await;

    Ok(Json(response))
}
