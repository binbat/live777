use std::convert::Infallible;

use crate::error::AppError;
use crate::AppState;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive};
use axum::response::{Response, Sse};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use axum_extra::extract::Query;
use http::StatusCode;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

pub fn route() -> Router<AppState> {
    Router::new()
        .route(&api::path::streams(""), get(index))
        .route(&api::path::streams("{stream}"), get(show))
        .route(&api::path::streams("{stream}"), post(create))
        .route(&api::path::streams("{stream}"), delete(destroy))
        .route(api::path::streams_sse(), get(sse))
        .route(&api::path::record(":stream"), post(record_stream))
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

async fn sse(
    State(state): State<AppState>,
    Query(req): Query<api::request::StreamSSE>,
) -> crate::result::Result<
    Sse<impl tokio_stream::Stream<Item = Result<axum::response::sse::Event, Infallible>>>,
> {
    let recv = state
        .stream_manager
        .sse_handler(req.streams.clone())
        .await?;
    let stream = ReceiverStream::new(recv).map(|forward_infos| {
        Ok(Event::default()
            .json_data(
                forward_infos
                    .into_iter()
                    .map(api::response::Stream::from)
                    .collect::<Vec<_>>(),
            )
            .unwrap())
    });
    let resp = Sse::new(stream).keep_alive(KeepAlive::default());
    Ok(resp)
}

#[cfg(feature = "recorder")]
async fn record_stream(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> crate::result::Result<Response<String>> {
    crate::recorder::start(state.stream_manager.clone(), stream).await?;
    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body("".to_string())?)
}

#[cfg(not(feature = "recorder"))]
async fn record_stream(
    _state: State<AppState>,
    Path(_stream): Path<String>,
) -> crate::result::Result<Response<String>> {
    Err(AppError::Throw("feature recorder not enabled".into()))
}
