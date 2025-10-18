use std::convert::Infallible;

use crate::AppState;
use crate::error::AppError;
use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive};
use axum::response::{Response, Sse};
use axum::routing::{delete, get, post};
use axum::{Json, Router};

// https://docs.rs/axum/latest/axum/extract/struct.Query.html
// For handling multiple values for the same query parameter, in a ?foo=1&foo=2&foo=3 fashion, use axum_extra::extract::Query instead.
use axum_extra::extract::Query;
#[cfg(feature = "recorder")]
use chrono::Utc;
use http::StatusCode;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

pub fn route() -> Router<AppState> {
    Router::new()
        .route(&api::path::streams(""), get(index))
        .route(&api::path::streams("{stream}"), get(show))
        .route(&api::path::streams("{stream}"), post(create))
        .route(&api::path::streams("{stream}"), delete(destroy))
        .route(api::path::streams_sse(), get(sse))
        .route(
            &api::path::record("{stream}"),
            post(record_stream).get(record_status).delete(stop_record),
        )
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
    Json(body): Json<api::recorder::StartRecordRequest>,
) -> crate::result::Result<Response<String>> {
    let base_dir = body.base_dir.clone();
    crate::recorder::start(
        state.stream_manager.clone(),
        stream.clone(),
        base_dir.clone(),
    )
    .await?;
    let date_path = Utc::now().format("%Y/%m/%d").to_string();
    let mpd_path = if let Some(prefix) = base_dir {
        format!("{prefix}/manifest.mpd")
    } else {
        format!("{stream}/{date_path}/manifest.mpd")
    };
    let resp = api::recorder::StartRecordResponse {
        id: stream.clone(),
        mpd_path,
    };
    match serde_json::to_string(&resp) {
        Ok(json_body) => Ok(Response::builder().status(StatusCode::OK).body(json_body)?),
        Err(e) => Err(AppError::InternalServerError(anyhow::anyhow!(
            "Failed to serialize response: {}",
            e
        ))),
    }
}

#[cfg(not(feature = "recorder"))]
async fn record_stream(
    _state: State<AppState>,
    Path(_stream): Path<String>,
) -> crate::result::Result<Response<String>> {
    Err(AppError::Throw("feature recorder not enabled".into()))
}

#[cfg(feature = "recorder")]
async fn record_status(
    State(_state): State<AppState>,
    Path(stream): Path<String>,
) -> crate::result::Result<Json<serde_json::Value>> {
    let recording = crate::recorder::is_recording(&stream).await;
    Ok(Json(serde_json::json!({ "recording": recording })))
}

#[cfg(not(feature = "recorder"))]
async fn record_status(
    _state: State<AppState>,
    _path: Path<String>,
) -> crate::result::Result<Json<serde_json::Value>> {
    Err(AppError::Throw("feature recorder not enabled".into()))
}

#[cfg(feature = "recorder")]
async fn stop_record(
    State(_state): State<AppState>,
    Path(stream): Path<String>,
) -> crate::result::Result<Response<String>> {
    crate::recorder::stop(stream.clone()).await?;
    Ok(Response::builder()
        .status(StatusCode::OK)
        .body("".to_string())?)
}

#[cfg(not(feature = "recorder"))]
async fn stop_record(
    _state: State<AppState>,
    Path(_stream): Path<String>,
) -> crate::result::Result<Response<String>> {
    Err(AppError::Throw("feature recorder not enabled".into()))
}
