use axum::{
    Router,
    extract::{Path, Request, State},
    response::Response,
    routing::{get, post},
};
use http::Uri;

use crate::{AppState, error::AppError, result::Result};

/// Alias-pinned proxy for liveion's runtime source management API
/// (`/api/sources/...`): every route rewrites to the same path on the named
/// node and forwards with the node's bearer token injected
/// (`proxy::request_proxy`). Cluster clients manage per-node sources
/// through liveman only — nodes never need to be exposed directly.
pub fn route() -> Router<AppState> {
    Router::new()
        .route("/api/sources/{alias}", get(list_sources))
        .route(
            "/api/sources/{alias}/{stream}",
            post(create_source)
                .get(get_source_info)
                .delete(delete_source),
        )
        .route("/api/sources/{alias}/{stream}/state", get(get_source_state))
        .route(
            "/api/sources/{alias}/{stream}/bitrate",
            get(get_source_bitrate),
        )
        .route(
            "/api/sources/{alias}/{stream}/tier",
            get(get_source_tier).post(apply_source_tier),
        )
}

async fn proxy_sources(
    state: AppState,
    alias: &str,
    path: &str,
    mut req: Request,
) -> Result<Response> {
    *req.uri_mut() = Uri::try_from(path).unwrap();
    match state.storage.get_map_server().get(alias).cloned() {
        Some(server) => super::proxy::request_proxy(state, req, &server).await,
        None => Err(AppError::NoAvailableNode),
    }
}

async fn list_sources(
    State(state): State<AppState>,
    Path(alias): Path<String>,
    req: Request,
) -> Result<Response> {
    proxy_sources(state, &alias, "/api/sources", req).await
}

async fn create_source(
    State(state): State<AppState>,
    Path((alias, stream)): Path<(String, String)>,
    req: Request,
) -> Result<Response> {
    let res = proxy_sources(
        state.clone(),
        &alias,
        &format!("/api/sources/{stream}"),
        req,
    )
    .await?;
    // A node with a running source hosts the stream: register the mapping
    // eagerly so WHEP routing sees it before the next node snapshot.
    if res.status().is_success() {
        state.storage.stream_put(stream, alias).await?;
    }
    Ok(res)
}

async fn get_source_info(
    State(state): State<AppState>,
    Path((alias, stream)): Path<(String, String)>,
    req: Request,
) -> Result<Response> {
    proxy_sources(state, &alias, &format!("/api/sources/{stream}"), req).await
}

async fn delete_source(
    State(state): State<AppState>,
    Path((alias, stream)): Path<(String, String)>,
    req: Request,
) -> Result<Response> {
    proxy_sources(state, &alias, &format!("/api/sources/{stream}"), req).await
}

async fn get_source_state(
    State(state): State<AppState>,
    Path((alias, stream)): Path<(String, String)>,
    req: Request,
) -> Result<Response> {
    proxy_sources(state, &alias, &format!("/api/sources/{stream}/state"), req).await
}

async fn get_source_bitrate(
    State(state): State<AppState>,
    Path((alias, stream)): Path<(String, String)>,
    req: Request,
) -> Result<Response> {
    proxy_sources(
        state,
        &alias,
        &format!("/api/sources/{stream}/bitrate"),
        req,
    )
    .await
}

async fn get_source_tier(
    State(state): State<AppState>,
    Path((alias, stream)): Path<(String, String)>,
    req: Request,
) -> Result<Response> {
    proxy_sources(state, &alias, &format!("/api/sources/{stream}/tier"), req).await
}

async fn apply_source_tier(
    State(state): State<AppState>,
    Path((alias, stream)): Path<(String, String)>,
    req: Request,
) -> Result<Response> {
    proxy_sources(state, &alias, &format!("/api/sources/{stream}/tier"), req).await
}
