use std::convert::Infallible;

use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use axum::routing::post;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use tokio::sync::broadcast;

use crate::AppState;
use crate::error::AppError;
use crate::result::Result;

pub fn route() -> Router<AppState> {
    let router = Router::new().route(
        &api::path::stream_cable_messages("{stream}", "{label}"),
        post(cable_inject).get(cable_messages),
    );
    #[cfg(feature = "cascade")]
    {
        router.route(&api::path::cascade("{stream}"), post(cascade))
    }
    #[cfg(not(feature = "cascade"))]
    {
        router
    }
}

/// Inject a message into a stream's cable: broadcast to every subscriber
/// without `?session=`, or directly onto one session's open channel with
/// it. See `Manager::cable_inject`.
async fn cable_inject(
    State(state): State<AppState>,
    Path((stream, label)): Path<(String, String)>,
    Query(query): Query<api::request::CableMessageInjectQuery>,
    body: std::result::Result<Json<api::request::CableMessageInject>, JsonRejection>,
) -> Result<Json<api::response::CableMessageInjected>> {
    let Json(body) = body.map_err(|rej| AppError::bad_request(rej.body_text()))?;
    let receivers = state
        .stream_manager
        .cable_inject(
            &stream,
            &label,
            body.data.into_bytes(),
            query.session.as_deref(),
        )
        .await?;
    Ok(Json(api::response::CableMessageInjected { receivers }))
}

/// Receive a stream's cable traffic as SSE: one event per message flowing
/// towards the publisher seat, `{"data": "<base64>"}`, or `{"lagged": n}`
/// after a receiver lag (the stream continues). The stream ends when the
/// bus closes (stream deleted) or the server shuts down — application
/// servers hold this open permanently, so it must not hold graceful
/// shutdown open. See `Manager::cable_subscribe`.
async fn cable_messages(
    State(state): State<AppState>,
    Path((stream, label)): Path<(String, String)>,
) -> Result<Sse<impl tokio_stream::Stream<Item = std::result::Result<Event, Infallible>>>> {
    let mut recv = state
        .stream_manager
        .cable_subscribe(&stream, &label)
        .await?;
    let cancel = state.stream_manager.cancel_token();
    let events = async_stream::stream! {
        loop {
            let msg = tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                msg = recv.recv() => match msg {
                    Ok(data) => api::response::CableMessage {
                        data: Some(BASE64.encode(data)),
                        lagged: None,
                    },
                    Err(broadcast::error::RecvError::Lagged(n)) => api::response::CableMessage {
                        data: None,
                        lagged: Some(n),
                    },
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            };
            yield Ok(Event::default().json_data(msg).unwrap());
        }
    };
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

#[cfg(feature = "cascade")]
async fn cascade(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(body): Json<api::request::Cascade>,
) -> Result<String> {
    let api::request::Cascade {
        source_url,
        target_url,
        token,
    } = body;

    match (source_url, target_url) {
        (None, None) => {
            return Err(AppError::throw(
                "src and dst cannot be empty at the same time",
            ));
        }
        (Some(_), Some(_)) => {
            return Err(AppError::throw(
                "src and dst cannot be non-empty at the same time",
            ));
        }
        (Some(source_url), None) => {
            state
                .stream_manager
                .cascade_pull(stream, source_url, token)
                .await?;
        }
        (None, Some(target_url)) => {
            state
                .stream_manager
                .cascade_push(stream, target_url, token)
                .await?;
        }
    }
    Ok("".to_string())
}
