use crate::AppState;
use crate::result::Result;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

#[cfg(feature = "source")]
use crate::stream::source::*;

#[derive(Debug, Deserialize)]
pub struct CreateSourceRequest {
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct SourceResponse {
    pub id: String,
    pub source_type: String,
    pub state: StreamSourceState,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct SourceListResponse {
    pub sources: Vec<SourceInfo>,
}

#[derive(Debug, Serialize)]
pub struct SourceInfo {
    pub id: String,
    pub stream_id: String,
    pub source_type: String,
    pub state: StreamSourceState,
}

#[cfg(feature = "source")]
pub fn route() -> Router<AppState> {
    Router::new()
        .route("/api/sources", get(list_sources))
        .route(
            "/api/sources/{stream}",
            post(create_source)
                .get(get_source_info)
                .delete(delete_source),
        )
        .route("/api/sources/{stream}/state", get(get_source_state))
}

#[cfg(not(feature = "source"))]
pub fn route() -> Router<AppState> {
    Router::new()
}

#[cfg(feature = "source")]
async fn create_source(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(req): Json<CreateSourceRequest>,
) -> Result<Json<SourceResponse>> {
    info!("Creating source for stream: {} from {}", stream, req.url);

    let config = crate::config::SourceConfig {
        stream_id: stream.clone(),
        url: req.url.clone(),
    };

    let source = create_source_from_url(&req.url, &config).await?;

    let source_type = if req.url.starts_with("rtsp://") {
        "rtsp"
    } else {
        "sdp"
    };

    let source_manager = &state.stream_manager.source_manager;
    let id = source_manager.add_source(source).await?;

    let forward = state
        .stream_manager
        .get_or_create_forward_for_source(&stream)
        .await;
    if let Err(e) = source_manager.create_bridge(&stream, forward).await {
        error!("Failed to create bridge: {}", e);
        return Err(e.into());
    }

    Ok(Json(SourceResponse {
        id,
        source_type: source_type.to_string(),
        state: StreamSourceState::Connected,
        message: format!(
            "{} source created and started with bridge",
            source_type.to_uppercase()
        ),
    }))
}

#[cfg(feature = "source")]
async fn list_sources(State(state): State<AppState>) -> Result<Json<SourceListResponse>> {
    let sources = state.stream_manager.source_manager.list_sources().await;

    let source_infos: Vec<SourceInfo> = sources
        .into_iter()
        .map(|(id, stream_id, state)| {
            let source_type = "unknown";
            SourceInfo {
                id,
                stream_id,
                source_type: source_type.to_string(),
                state,
            }
        })
        .collect();

    Ok(Json(SourceListResponse {
        sources: source_infos,
    }))
}

#[cfg(feature = "source")]
async fn get_source_info(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> Result<Json<SourceInfo>> {
    let sources = state.stream_manager.source_manager.list_sources().await;

    let source_info = sources
        .into_iter()
        .find(|(_, sid, _)| sid == &stream)
        .map(|(id, stream_id, state)| SourceInfo {
            id,
            stream_id,
            source_type: "unknown".to_string(),
            state,
        })
        .ok_or_else(|| anyhow::anyhow!("Source not found"))?;

    Ok(Json(source_info))
}

#[cfg(feature = "source")]
async fn get_source_state(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let sources = state.stream_manager.source_manager.list_sources().await;

    let source_state = sources
        .into_iter()
        .find(|(_, sid, _)| sid == &stream)
        .map(|(_, _, state)| state)
        .ok_or_else(|| anyhow::anyhow!("Source not found"))?;

    Ok(Json(serde_json::json!({
        "stream_id": stream,
        "state": format!("{:?}", source_state),
    })))
}

#[cfg(feature = "source")]
async fn delete_source(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> Result<Json<serde_json::Value>> {
    info!("Deleting source: {}", stream);

    state
        .stream_manager
        .source_manager
        .remove_source(&stream)
        .await?;

    Ok(Json(serde_json::json!({
        "message": "Source deleted successfully",
        "stream_id": stream,
    })))
}
