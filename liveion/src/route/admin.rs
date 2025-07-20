use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};

use crate::error::AppError;
use crate::result::Result;
use crate::AppState;

pub fn route() -> Router<AppState> {
    Router::new().route(&api::path::cascade("{stream}"), post(cascade))
}

async fn cascade(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(body): Json<api::request::Cascade>,
) -> Result<String> {
    if body.source_url.is_none() && body.target_url.is_none() {
        return Err(AppError::throw(
            "src and dst cannot be empty at the same time",
        ));
    }
    if body.source_url.is_some() && body.target_url.is_some() {
        return Err(AppError::throw(
            "src and dst cannot be non-empty at the same time",
        ));
    }
    if body.source_url.is_some() {
        state
            .stream_manager
            .cascade_pull(stream, body.source_url.unwrap(), body.token)
            .await?;
    } else {
        state
            .stream_manager
            .cascade_push(stream, body.target_url.unwrap(), body.token)
            .await?;
    }
    Ok("".to_string())
}
