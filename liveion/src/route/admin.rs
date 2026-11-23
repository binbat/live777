use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::routing::post;

use crate::AppState;
use crate::error::AppError;
use crate::result::Result;

pub fn route() -> Router<AppState> {
    let router = Router::new().route(
        &api::path::stream_channel_messages("{stream}", "{label}"),
        post(cable_inject),
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
    Query(query): Query<api::request::ChannelMessageInjectQuery>,
    body: std::result::Result<Json<api::request::ChannelMessageInject>, JsonRejection>,
) -> Result<Json<api::response::ChannelMessageInjected>> {
    let Json(body) = body.map_err(|_| AppError::bad_request("invalid body"))?;
    let receivers = state
        .stream_manager
        .cable_inject(
            &stream,
            &label,
            body.data.into_bytes(),
            query.session.as_deref(),
        )
        .await?;
    Ok(Json(api::response::ChannelMessageInjected { receivers }))
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
