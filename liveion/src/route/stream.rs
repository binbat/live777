use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use axum_extra::extract::Query;
use http::StatusCode;

use crate::error::AppError;
use crate::AppState;

pub fn route() -> Router<AppState> {
    Router::new()
        .route(&api::path::streams(""), get(index))
        .route(&api::path::streams(":stream"), get(show))
        .route(&api::path::streams(":stream"), post(create))
        .route(&api::path::streams(":stream"), delete(destroy))
}

async fn index(
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

async fn show(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> crate::result::Result<Json<api::response::Stream>> {
    match state
        .stream_manager
        .info(vec![stream.clone()])
        .await
        .into_iter()
        .map(|forward_info| forward_info.into())
        .collect::<Vec<api::response::Stream>>()
        .first()
    {
        Some(stream) => Ok(Json(stream.clone())),
        None => Err(AppError::StreamNotFound(stream.to_string())),
    }
}

async fn create(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> crate::result::Result<Response<String>> {
    match state.stream_manager.stream_create(stream).await {
        Ok(_) => Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body("".to_string())?),
        Err(e) => Err(AppError::StreamAlreadyExists(e.to_string())),
    }
}

async fn destroy(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> crate::result::Result<Response<String>> {
    match state.stream_manager.stream_delete(stream).await {
        Ok(_) => Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body("".to_string())?),
        Err(e) => Err(AppError::StreamNotFound(e.to_string())),
    }
}
