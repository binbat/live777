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

#[derive(Debug, Deserialize)]
pub struct SetTierRequest {
    /// Name of a configured quality tier to switch to (required).
    #[serde(default)]
    pub tier: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TierInfo {
    pub name: String,
    pub bitrate: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
}

impl From<crate::stream::source::tier::QualityTier> for TierInfo {
    fn from(t: crate::stream::source::tier::QualityTier) -> Self {
        Self {
            name: t.name,
            bitrate: t.bitrate,
            width: t.width,
            height: t.height,
            fps: t.fps,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TierResponse {
    pub stream_id: String,
    /// The tier that was switched to.
    pub tier: String,
    pub bitrate: u32,
    /// True when the tier carried resolution/framerate and the switch
    /// rebuilt the capture+encoder pipeline (brief frame gap), as opposed
    /// to a seamless in-place retune.
    pub rebuilt: bool,
}

#[derive(Debug, Serialize)]
pub struct SourceTierResponse {
    pub stream_id: String,
    /// The last tier applied through this API (`null` = the configured
    /// base profile).
    pub active_tier: Option<String>,
    /// Configured quality tiers (empty when the source defines none).
    pub tiers: Vec<TierInfo>,
}

#[derive(Debug, Serialize)]
pub struct SourceBitrateResponse {
    pub stream_id: String,
    /// How the encoder bitrate is driven: `adaptive` (AIMD controller) or
    /// `fixed` (configured value, or the last applied tier's).
    pub mode: String,
    /// Current encoder bitrate, when the source reports one.
    pub bitrate: Option<u32>,
    /// Whether the source opted into adaptive bitrate.
    pub adaptive: bool,
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
        .route("/api/sources/{stream}/bitrate", get(get_source_bitrate))
        .route(
            "/api/sources/{stream}/tier",
            get(get_source_tier).post(apply_source_tier),
        )
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
    info!(
        "Creating source for stream: {} from {}",
        stream,
        redact_url(&req.url)
    );

    let config = crate::config::SourceConfig {
        url: Some(req.url.clone()),
        #[cfg(feature = "native-source")]
        capture: None,
        #[cfg(feature = "native-source")]
        encoder: None,
        #[cfg(feature = "native-source")]
        output: Default::default(),
        #[cfg(feature = "native-source")]
        tiers: vec![],
    };

    let source = create_source_from_url(
        &stream,
        &req.url,
        &config,
        crate::stream::source::SourceNetConfig {
            ice_servers: iceserver::to_rtc_ice_servers(state.config.ice_servers.clone()),
            ice_udp_addrs: api::webrtc::resolve_webrtc_ice_udp_addrs(Some(
                state.config.webrtc.ice_udp_addrs.clone(),
            )),
        },
    )
    .await?;

    let source_type = url_source_kind(&req.url);

    let source_manager = &state.stream_manager.source_manager;
    let id = source_manager.add_source(source).await?;

    let forward = state
        .stream_manager
        .get_or_create_forward_for_source(&stream)
        .await;
    if let Err(e) = source_manager
        .create_bridge(
            &stream,
            forward,
            crate::stream::source::manager::DEFAULT_BRIDGE_CODEC_WAIT,
            crate::stream::source::manager::DEFAULT_BRIDGE_RTCP_WAIT,
        )
        .await
    {
        error!("Failed to create bridge: {}", e);
        if let Err(cleanup_err) = state
            .stream_manager
            .stop_stream_source(&stream, crate::event::SessionStopReason::PeerClosed)
            .await
        {
            error!(
                "Failed to clean up source {} after bridge creation failure: {:?}",
                stream, cleanup_err
            );
        }
        return Err(e.into());
    }
    state.stream_manager.emit_source_publish_started(&stream);

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
        .stop_stream_source(&stream, crate::event::SessionStopReason::ApiDeleted)
        .await?;

    Ok(Json(serde_json::json!({
        "message": "Source deleted successfully",
        "stream_id": stream,
    })))
}

/// Query the stream source's bitrate state: drive mode (adaptive / manual
/// / fixed), current bitrate, active override, and configured tiers.
#[cfg(feature = "source")]
async fn get_source_bitrate(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> Result<Json<SourceBitrateResponse>> {
    use crate::error::AppError;

    let info = state
        .stream_manager
        .source_manager
        .source_bitrate_info(&stream)
        .await
        .ok_or_else(|| AppError::source_not_found(format!("Source not found: {stream}")))?;

    Ok(Json(SourceBitrateResponse {
        stream_id: stream,
        mode: info.mode.as_str().to_string(),
        bitrate: info.current,
        adaptive: info.adaptive,
    }))
}

/// Query the stream source's quality-tier state: the configured tiers
/// and the active one.
#[cfg(feature = "source")]
async fn get_source_tier(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> Result<Json<SourceTierResponse>> {
    use crate::error::AppError;

    let info = state
        .stream_manager
        .source_manager
        .source_tier_info(&stream)
        .await
        .ok_or_else(|| AppError::source_not_found(format!("Source not found: {stream}")))?;

    Ok(Json(SourceTierResponse {
        stream_id: stream,
        active_tier: info.active_tier,
        tiers: info.tiers.into_iter().map(TierInfo::from).collect(),
    }))
}

/// Apply a configured quality tier: re-provision the stream source with
/// the tier's geometry and bitrate.  A bitrate-only tier retunes the
/// running encoder in place; a tier carrying resolution/framerate
/// rebuilds the capture+encoder pipeline (brief frame gap, subscribers
/// stay connected).  The adaptive controller is not suspended — the
/// tier's bitrate becomes its new rung ceiling.
#[cfg(feature = "source")]
async fn apply_source_tier(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(req): Json<SetTierRequest>,
) -> Result<Json<TierResponse>> {
    use crate::error::AppError;
    use crate::stream::source::manager::{ApplyTierOutcome, TierResolution};

    let Some(tier_name) = req.tier else {
        return Err(AppError::bad_request("missing required field 'tier'"));
    };

    let resolved = match state
        .stream_manager
        .source_manager
        .resolve_tier(&stream, &tier_name)
        .await
    {
        TierResolution::Resolved(t) => t,
        TierResolution::SourceNotFound => {
            return Err(AppError::source_not_found(format!(
                "Source not found: {stream}"
            )));
        }
        TierResolution::TierNotFound => {
            return Err(AppError::bad_request(format!(
                "tier '{tier_name}' is not defined for stream: {stream}"
            )));
        }
    };

    let rebuilt = resolved.needs_rebuild();
    match state
        .stream_manager
        .source_manager
        .apply_source_tier(&stream, &resolved)
        .await
    {
        ApplyTierOutcome::Applied => {
            info!(
                "Source quality tier applied: {} -> {}{} ({} bps)",
                stream,
                resolved.name,
                if rebuilt { " (pipeline rebuilt)" } else { "" },
                resolved.bitrate
            );
            Ok(Json(TierResponse {
                stream_id: stream,
                tier: resolved.name,
                bitrate: resolved.bitrate,
                rebuilt,
            }))
        }
        ApplyTierOutcome::SourceNotFound => Err(AppError::source_not_found(format!(
            "Source not found: {stream}"
        ))),
        ApplyTierOutcome::Unsupported => Err(AppError::source_bitrate_unsupported(format!(
            "Applying tier '{}' to {stream} failed (the source is not running, \
             or the pipeline rebuild failed and was rolled back)",
            resolved.name
        ))),
    }
}
