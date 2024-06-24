use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::Query;

use crate::error::AppError;
use crate::result::Result;
use crate::AppState;

pub fn route() -> Router<AppState> {
    Router::new()
        .route(api::path::ADMIN_INFOS, get(infos))
        .route(&api::path::cascade(":stream"), post(cascade))
}
async fn infos(
    State(state): State<AppState>,
    Query(req): Query<api::request::QueryInfo>,
) -> Result<Json<Vec<api::response::Stream>>> {
    Ok(Json(
        state
            .stream_manager
            .info(req.streams)
            .await
            .into_iter()
            .map(|forward_info| forward_info.into())
            .collect(),
    ))
}

async fn cascade(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(body): Json<api::request::Cascade>,
) -> Result<String> {
    if body.src.is_none() && body.dst.is_none() {
        return Err(AppError::throw(
            "src and dst cannot be empty at the same time",
        ));
    }
    if body.src.is_some() && body.dst.is_some() {
        return Err(AppError::throw(
            "src and dst cannot be non-empty at the same time",
        ));
    }
    if body.src.is_some() {
        state
            .stream_manager
            .cascade_pull(stream, body.src.unwrap(), body.token)
            .await?;
    } else {
        state
            .stream_manager
            .cascade_push(stream, body.dst.unwrap(), body.token)
            .await?;
    }
    Ok("".to_string())
}
