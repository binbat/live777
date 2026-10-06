//! Runtime-managed output targets (live777#473): the API counterpart of
//! static `[[stream.<name>.targets]]` config entries.
//!
//! `POST /api/targets/{stream}` adds a `whip://` / `rtp://` / `rtsp://`
//! target at runtime — the same validation and supervisor semantics as a
//! static one (media-driven start/stop, backoff retries, standing demand on
//! `on_demand` streams). Runtime targets do not persist across restarts,
//! mirroring dynamic cascade pushes. Static (config-owned) targets are
//! listed alongside runtime ones but cannot be removed through the API.

use axum::Router;

#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
use axum::Json;
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
use axum::extract::{Path, Query, State};
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
use axum::routing::{get, post};
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
use serde::{Deserialize, Serialize};
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
use tracing::info;

use crate::AppState;
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
use crate::error::AppError;
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
use crate::result::Result;
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
use crate::target::{StartTargetError, TargetOrigin};

#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
#[derive(Debug, Serialize)]
pub struct TargetResponse {
    pub stream: String,
    /// Credential-free target URL (a WHIP token rides as userinfo).
    pub url: String,
    /// `config` (static `[[stream.<name>.targets]]` entry) or `runtime`.
    pub origin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multicast_interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_type: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sdp_file: Option<String>,
}

#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
impl TargetResponse {
    fn new(stream: String, config: crate::config::TargetConfig, origin: TargetOrigin) -> Self {
        Self {
            stream,
            url: crate::target::redact_target_url(&config.url),
            origin: origin.as_str().to_string(),
            multicast_interface: config.multicast_interface,
            ttl: config.ttl,
            payload_type: config.payload_type,
            sdp_file: config.sdp_file,
        }
    }
}

#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
#[derive(Debug, Serialize)]
pub struct TargetListResponse {
    pub targets: Vec<TargetResponse>,
}

#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
#[derive(Debug, Deserialize)]
pub struct DeleteTargetQuery {
    /// The target's URL, exactly as it was registered.
    pub url: String,
}

#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
pub fn route() -> Router<AppState> {
    Router::new()
        .route("/api/targets", get(list_targets))
        .route(
            &api::path::targets("{stream}"),
            post(create_target)
                .get(list_stream_targets)
                .delete(delete_target),
        )
}

#[cfg(not(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
)))]
pub fn route() -> Router<AppState> {
    Router::new()
}

#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
async fn list_targets(State(state): State<AppState>) -> Json<TargetListResponse> {
    let targets = state
        .stream_manager
        .list_targets()
        .into_iter()
        .map(|(stream, config, origin)| TargetResponse::new(stream, config, origin))
        .collect();
    Json(TargetListResponse { targets })
}

#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
async fn list_stream_targets(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> Json<TargetListResponse> {
    let targets = state
        .stream_manager
        .list_targets()
        .into_iter()
        .filter(|(s, _, _)| s == &stream)
        .map(|(stream, config, origin)| TargetResponse::new(stream, config, origin))
        .collect();
    Json(TargetListResponse { targets })
}

/// Add a runtime target to a stream. The body is a target entry in the same
/// shape as `[[stream.<name>.targets]]` config.
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
async fn create_target(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(target): Json<crate::config::TargetConfig>,
) -> Result<Json<TargetResponse>> {
    info!(
        "Creating runtime target for stream {} towards {}",
        stream,
        crate::target::redact_target_url(&target.url)
    );

    crate::target::start_runtime_target(&state.stream_manager, stream.clone(), target.clone())
        .await
        .map_err(|e| match e {
            StartTargetError::StreamNotFound(stream) => {
                AppError::stream_not_found(format!("Stream not found: {stream}"))
            }
            StartTargetError::Duplicate(url) => AppError::stream_already_exists(format!(
                "target already registered for stream {stream}: {url}"
            )),
            StartTargetError::Invalid(e) => AppError::bad_request(format!("{e}")),
        })?;

    Ok(Json(TargetResponse::new(
        stream,
        target,
        TargetOrigin::Runtime,
    )))
}

/// Remove a runtime target by URL. Config-owned targets are rejected: they
/// are managed by the config file.
#[cfg(any(
    feature = "target-whip",
    feature = "target-rtp",
    feature = "target-rtsp"
))]
async fn delete_target(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Query(query): Query<DeleteTargetQuery>,
) -> Result<Json<TargetResponse>> {
    let entry = state
        .stream_manager
        .remove_runtime_target(&stream, &query.url)
        .map_err(|e| match e {
            crate::target::RemoveTargetError::NotFound => AppError::target_not_found(format!(
                "no target registered for stream {stream}: {}",
                crate::target::redact_target_url(&query.url)
            )),
            crate::target::RemoveTargetError::ConfigOwned => AppError::StreamProvisioned(
                format!(
                    "stream {stream} target {} is declared in the config file and cannot be removed through the API",
                    crate::target::redact_target_url(&query.url)
                ),
            ),
        })?;

    info!(
        "Removing runtime target of stream {} towards {}",
        stream,
        crate::target::redact_target_url(&query.url)
    );
    entry.cancel.cancel();

    Ok(Json(TargetResponse::new(
        stream,
        entry.config,
        TargetOrigin::Runtime,
    )))
}
