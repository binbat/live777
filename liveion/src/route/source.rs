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
pub struct SetBitrateRequest {
    /// Target encoder bitrate in bits per second.  Exactly one of
    /// `bitrate` / `tier` must be set.
    #[serde(default)]
    pub bitrate: Option<u32>,
    /// Name of a configured quality tier to switch to.
    #[serde(default)]
    pub tier: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BitrateResponse {
    pub stream_id: String,
    pub bitrate: u32,
    /// The tier that was switched to, when the request named one.
    pub tier: Option<String>,
    /// True when the stream's adaptive-bitrate controller suspended itself
    /// in favour of this manual override (`DELETE` resumes it).
    pub adaptive_suspended: bool,
}

#[derive(Debug, Serialize)]
pub struct TierInfo {
    pub name: String,
    pub bitrate: u32,
}

#[derive(Debug, Serialize)]
pub struct SourceBitrateResponse {
    pub stream_id: String,
    /// How the encoder bitrate is driven: `adaptive` (AIMD controller),
    /// `manual` (override holds), or `fixed` (configured value).
    pub mode: String,
    /// Current encoder bitrate, when the source reports one.
    pub bitrate: Option<u32>,
    /// Active manual override bitrate.
    pub manual_bitrate: Option<u32>,
    /// The tier whose bitrate equals the manual override, if any.
    pub active_tier: Option<String>,
    /// Whether the source opted into adaptive bitrate.
    pub adaptive: bool,
    /// Configured quality tiers (empty when the source defines none).
    pub tiers: Vec<TierInfo>,
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
        .route(
            "/api/sources/{stream}/bitrate",
            get(get_source_bitrate)
                .post(set_source_bitrate)
                .delete(clear_manual_bitrate),
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
        manual_bitrate: info.manual,
        active_tier: info.active_tier,
        adaptive: info.adaptive,
        tiers: info
            .tiers
            .into_iter()
            .map(|t| TierInfo {
                name: t.name,
                bitrate: t.bitrate,
            })
            .collect(),
    }))
}

/// Manually retune the stream source's encoder bitrate (issue #409),
/// either with a raw `bitrate` value or by naming a configured `tier`.
///
/// Takes effect immediately.  On a stream with `encoder.adaptive_bitrate`
/// enabled the AIMD controller suspends in favour of the manual value;
/// `DELETE` on the same path clears the override and resumes adaptive
/// control.
#[cfg(feature = "source")]
async fn set_source_bitrate(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(req): Json<SetBitrateRequest>,
) -> Result<Json<BitrateResponse>> {
    use crate::error::AppError;
    use crate::stream::source::manager::{SetBitrateOutcome, TierResolution};

    let (bps, tier_name) = match (req.bitrate, req.tier) {
        (Some(_), Some(_)) | (None, None) => {
            return Err(AppError::bad_request(
                "exactly one of 'bitrate' or 'tier' must be set",
            ));
        }
        (Some(0), None) => return Err(AppError::bad_request("bitrate must be non-zero")),
        (Some(bps), None) => (bps, None),
        (None, Some(tier)) => {
            match state
                .stream_manager
                .source_manager
                .resolve_bitrate_tier(&stream, &tier)
                .await
            {
                TierResolution::Resolved(bps) => (bps, Some(tier)),
                TierResolution::SourceNotFound => {
                    return Err(AppError::source_not_found(format!(
                        "Source not found: {stream}"
                    )));
                }
                TierResolution::TierNotFound => {
                    return Err(AppError::bad_request(format!(
                        "tier '{tier}' is not defined for stream: {stream}"
                    )));
                }
            }
        }
    };

    match state
        .stream_manager
        .source_manager
        .set_source_bitrate(&stream, bps)
        .await
    {
        SetBitrateOutcome::Applied { adaptive_suspended } => {
            info!(
                "Source bitrate manually set: {} -> {} bps{} (adaptive suspended: {})",
                stream,
                bps,
                tier_name
                    .as_ref()
                    .map(|t| format!(" [tier: {t}]"))
                    .unwrap_or_default(),
                adaptive_suspended
            );
            Ok(Json(BitrateResponse {
                stream_id: stream,
                bitrate: bps,
                tier: tier_name,
                adaptive_suspended,
            }))
        }
        SetBitrateOutcome::SourceNotFound => Err(AppError::source_not_found(format!(
            "Source not found: {stream}"
        ))),
        SetBitrateOutcome::Unsupported => Err(AppError::source_bitrate_unsupported(format!(
            "Source encoder of {stream} does not support runtime bitrate retuning \
             (or the source is not running)"
        ))),
    }
}

/// Clear a manual bitrate override.  The adaptive (AIMD) controller, when
/// enabled, resumes from the held value; a fixed-bitrate source simply
/// drops the override marker.
#[cfg(feature = "source")]
async fn clear_manual_bitrate(
    State(state): State<AppState>,
    Path(stream): Path<String>,
) -> Result<Json<serde_json::Value>> {
    use crate::error::AppError;

    if let Some((bitrate, adaptive)) = state
        .stream_manager
        .source_manager
        .clear_manual_bitrate(&stream)
        .await
    {
        info!(
            "Source bitrate manual override cleared: {} (now {} bps, mode {})",
            stream,
            bitrate,
            if adaptive { "adaptive" } else { "fixed" }
        );
        Ok(Json(serde_json::json!({
            "message": if adaptive {
                "Manual override cleared, adaptive bitrate control resumed"
            } else {
                "Manual bitrate override cleared"
            },
            "stream_id": stream,
            "bitrate": bitrate,
            "mode": if adaptive { "adaptive" } else { "fixed" },
        })))
    } else {
        Err(AppError::source_not_found(format!(
            "No bitrate state for stream: {stream} \
             (no source, or the source is not a native encoder source)"
        )))
    }
}
