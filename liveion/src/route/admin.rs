use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::Query;

use crate::forward::message::ReforwardInfo;
use crate::AppState;

pub fn route() -> Router<AppState> {
    Router::new()
        .route(api::path::ADMIN_INFOS, get(infos))
        .route(&api::path::reforward(":stream"), post(reforward))
}
async fn infos(
    State(state): State<AppState>,
    Query(req): Query<api::request::QueryInfo>,
) -> crate::result::Result<Json<Vec<api::response::Stream>>> {
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

async fn reforward(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(req): Json<api::request::Reforward>,
) -> crate::result::Result<String> {
    state
        .stream_manager
        .reforward(
            stream,
            ReforwardInfo {
                target_url: req.target_url,
                admin_authorization: req.admin_authorization,
                resource_url: None,
            },
        )
        .await?;
    Ok("".to_string())
}
