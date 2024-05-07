use crate::forward::message::ReforwardInfo;
use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
pub fn route() -> Router<AppState> {
    Router::new()
        .route(live777_http::path::ADMIN_INFOS, get(infos))
        .route(&live777_http::path::reforward(":stream"), post(reforward))
}
async fn infos(
    State(state): State<AppState>,
    Query(req): Query<live777_http::request::QueryInfo>,
) -> crate::result::Result<Json<Vec<live777_http::response::StreamInfo>>> {
    Ok(Json(
        state
            .stream_manager
            .info(req.streams.map_or(vec![], |streams| {
                streams
                    .split(',')
                    .map(|stream| stream.to_string())
                    .collect()
            }))
            .await
            .into_iter()
            .map(|forward_info| forward_info.into())
            .collect(),
    ))
}

async fn reforward(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(req): Json<live777_http::request::Reforward>,
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
