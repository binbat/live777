use crate::error::AppError;
use crate::route::AppState;
use crate::{constant, forward};
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use http::{HeaderMap, StatusCode, Uri};
use std::collections::HashMap;

pub fn route() -> Router<AppState> {
    Router::new()
        .route(
            &live777_http::path::resource(":stream", ":session"),
            post(change_resource)
                .patch(add_ice_candidate)
                .delete(remove_stream_session),
        )
        .route(
            &live777_http::path::resource_layer(":stream", ":session"),
            get(get_layer).post(select_layer).delete(un_select_layer),
        )
}
async fn change_resource(
    State(state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    Json(req): Json<live777_http::request::ChangeResource>,
) -> crate::result::Result<Json<HashMap<String, String>>> {
    state
        .stream_manager
        .change_resource(stream, session, (req.kind, req.enabled))
        .await?;
    Ok(Json(HashMap::new()))
}

async fn add_ice_candidate(
    State(state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    header: HeaderMap,
    body: String,
) -> crate::result::Result<Response<String>> {
    let content_type = header
        .get("Content-Type")
        .ok_or(AppError::from(anyhow::anyhow!("Content-Type is required")))?;
    if content_type.to_str()? != "application/trickle-ice-sdpfrag" {
        return Err(anyhow::anyhow!("Content-Type must be application/trickle-ice-sdpfrag").into());
    }
    state
        .stream_manager
        .add_ice_candidate(stream, session, body)
        .await?;
    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body("".to_string())?)
}

async fn remove_stream_session(
    State(state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    _uri: Uri,
) -> crate::result::Result<Response<String>> {
    state
        .stream_manager
        .remove_stream_session(stream, session)
        .await?;
    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body("".to_string())?)
}

async fn get_layer(
    State(state): State<AppState>,
    Path((stream, _session)): Path<(String, String)>,
) -> crate::result::Result<Json<Vec<live777_http::response::Layer>>> {
    Ok(Json(
        state
            .stream_manager
            .layers(stream)
            .await?
            .into_iter()
            .map(|layer| layer.into())
            .collect(),
    ))
}

async fn select_layer(
    State(state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    Json(req): Json<live777_http::request::SelectLayer>,
) -> crate::result::Result<String> {
    state
        .stream_manager
        .select_layer(
            stream,
            session,
            req.encoding_id
                .map(|encoding_id| forward::message::Layer { encoding_id }),
        )
        .await?;
    Ok("".to_string())
}

async fn un_select_layer(
    State(state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
) -> crate::result::Result<String> {
    state
        .stream_manager
        .select_layer(
            stream,
            session,
            Some(forward::message::Layer {
                encoding_id: constant::RID_DISABLE.to_string(),
            }),
        )
        .await?;
    Ok("".to_string())
}
