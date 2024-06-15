use crate::error::AppError;
use crate::AppState;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use axum_extra::extract::Query;
use http::StatusCode;

pub fn route() -> Router<AppState> {
    Router::new()
        .route("/api/streams/:stream", post(create))
        .route("/api/streams/:stream", delete(destroy))
        .route("/api/streams/", get(index))
}

async fn create(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> crate::result::Result<Response<String>> {
    match state.stream_manager.stream_create(stream).await {
        Ok(_) => Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body("".to_string())?),
        Err(e) => Err(AppError::ResourceAlreadyExists(e.to_string())),
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
        Err(e) => Err(AppError::ResourceNotFound(e.to_string())),
    }
}

async fn index(
    State(state): State<AppState>,
    Query(req): Query<api::request::QueryInfo>,
) -> crate::result::Result<Json<Vec<api::response::StreamInfo>>> {
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
