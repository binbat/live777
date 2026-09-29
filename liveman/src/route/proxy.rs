use axum::{
    Router,
    extract::{Path, Request, State},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
// https://docs.rs/axum/latest/axum/extract/struct.Query.html
// For handling multiple values for the same query parameter, in a ?foo=1&foo=2&foo=3 fashion, use axum_extra::extract::Query instead.
use axum_extra::extract::Query;
use http::{HeaderValue, Uri, header};
use serde::{Deserialize, Serialize};
use tracing::{Span, debug, error, warn};

use api::response::Stream;
use iceserver::{cloudflare, coturn, format_iceserver, link_header};

use crate::route::cascade;
use crate::route::node;
use crate::route::recorder;
use crate::route::source;
use crate::route::storage;
use crate::route::stream;
use crate::store::Server;
use crate::{AppState, error::AppError, result::Result};

#[derive(Serialize, Deserialize, Clone)]
pub struct QueryExtract {
    #[serde(default)]
    pub nodes: Vec<String>,
}

pub fn route() -> Router<AppState> {
    Router::new()
        .route(&api::path::whip("{stream}"), post(whip))
        .route(&api::path::whep("{stream}"), post(whep))
        .route(
            &api::path::session("{stream}", "{session}"),
            post(session).patch(session).delete(session),
        )
        .route(
            &api::path::session_layer("{stream}", "{session}"),
            get(session).post(session).delete(session),
        )
        .route(
            &api::path::whip_with_node("{stream}", "{alias}"),
            post(api_whip),
        )
        .route(
            &api::path::whep_with_node("{stream}", "{alias}"),
            post(api_whep),
        )
        .route("/api/nodes/", get(node::index))
        .route("/api/streams/", get(stream::index))
        .route("/api/streams/{stream}", get(stream::show))
        .route("/api/streams/{stream}", post(stream::create))
        .route("/api/streams/{stream}", delete(stream::destroy))
        .merge(recorder::route())
        .merge(source::route())
        .merge(storage::route())
}

async fn api_whip(
    State(state): State<AppState>,
    Path((alias, stream)): Path<(String, String)>,
    mut req: Request,
) -> Result<Response> {
    let uri = format!("/whip/{stream}");
    *req.uri_mut() = Uri::try_from(uri).unwrap();

    match state.storage.get_map_server().get(&alias).cloned() {
        Some(server) => {
            let res = request_proxy(state.clone(), req, &server).await?;
            let location = session_location_of(&res, "WHIP");
            record_session_location(&state, location, &stream, &server.alias, "WHIP").await;
            Ok(res)
        }
        None => Err(AppError::NoAvailableNode),
    }
}

async fn api_whep(
    State(state): State<AppState>,
    Path((alias, stream)): Path<(String, String)>,
    mut req: Request,
) -> Result<Response> {
    let uri = format!("/whep/{stream}");
    *req.uri_mut() = Uri::try_from(uri).unwrap();

    match state.storage.get_map_server().get(&alias).cloned() {
        Some(server) => {
            let res = request_proxy(state.clone(), req, &server).await?;
            let location = session_location_of(&res, "WHEP");
            record_session_location(&state, location, &stream, &server.alias, "WHEP").await;
            Ok(res)
        }
        None => Err(AppError::NoAvailableNode),
    }
}

/// Eagerly record the stream -> node and session -> node mappings from a
/// proxied WHIP/WHEP response, so session routing (trickle PATCH, DELETE)
/// and capacity accounting work before the next node snapshot (SSE or poll)
/// arrives. `location` is the response's Location header, extracted by the
/// caller (`Response<Body>` is not `Sync`, so it must not cross an `.await`).
async fn record_session_location(
    state: &AppState,
    location: Option<String>,
    stream: &str,
    alias: &str,
    op: &str,
) {
    let Some(location) = location else {
        error!("{op} Error: Location not found");
        return;
    };
    state
        .storage
        .stream_put(stream.to_string(), alias.to_string())
        .await
        .unwrap();
    state
        .storage
        .session_put(location, alias.to_string())
        .await
        .unwrap();
}

/// Pull the fields `record_session_location` needs out of a proxied
/// response, logging non-success responses.
fn session_location_of(res: &Response, op: &str) -> Option<String> {
    if !res.status().is_success() {
        error!("{op} Error: {:?}", res);
        return None;
    }
    res.headers()
        .get(header::LOCATION)
        .map(|v| String::from(v.to_str().unwrap()))
}

async fn extra_ice(
    headers: &mut reqwest::header::HeaderMap,
    cfg: crate::config::ExtraIce,
) -> Result<()> {
    if cfg.override_upstream_ice_servers {
        headers.remove(header::LINK);
    }

    if !cfg.ice_servers.is_empty() {
        for link in link_header(cfg.ice_servers) {
            headers.append(header::LINK, HeaderValue::from_str(&link)?);
        }
    }

    if let Some(cfg) = cfg.coturn {
        let (username, password) = coturn::generate_credentials(
            cfg.secret,
            coturn::generate_expiry_timestamp(cfg.ttl),
            None,
        );
        let coturn_ice_server = format_iceserver(cfg.urls, username, password);
        debug!("coturn ice server: {:?}", coturn_ice_server);

        for link in link_header(vec![coturn_ice_server]) {
            headers.append(header::LINK, HeaderValue::from_str(&link)?);
        }
    }

    if let Some(cfg) = cfg.cloudflare {
        let cloudflare_ice_servers =
            cloudflare::request_iceserver(cfg.key_id, cfg.api_token, cfg.ttl).await?;
        debug!("cloudflare ice server {:?}", cloudflare_ice_servers);

        for link in link_header(cloudflare_ice_servers) {
            headers.append(header::LINK, HeaderValue::from_str(&link)?);
        }
    }

    Ok(())
}

async fn whip(
    State(mut state): State<AppState>,
    Path(stream): Path<String>,
    Query(query_extract): Query<QueryExtract>,
    req: Request,
) -> Result<Response> {
    let stream_nodes = state.storage.stream_get(stream.clone()).await?;
    debug!("{:?}", stream_nodes);
    let target = match stream_nodes.is_empty() {
        true => {
            let mut nodes = state.storage.nodes().await;
            warn!("{:?}", nodes);
            if !query_extract.nodes.is_empty() {
                nodes.retain(|x| query_extract.nodes.contains(&x.alias));
            }
            maximum_idle_node(state.clone(), nodes, stream.clone()).await
        }
        false => {
            let mut nodes = stream_nodes.clone();
            if !query_extract.nodes.is_empty() {
                nodes.retain(|x| query_extract.nodes.contains(&x.alias));
            }
            nodes.first().cloned()
        }
    };

    match target {
        Some(server) => {
            let resp = request_proxy(state.clone(), req, &server).await;
            match resp {
                Ok(mut res) => {
                    let location = session_location_of(&res, "WHIP");
                    record_session_location(&state, location, &stream, &server.alias, "WHIP").await;
                    extra_ice(res.headers_mut(), state.config.extra_ice).await?;
                    Ok(res)
                }
                Err(e) => Err(e),
            }
        }
        None => Err(AppError::NoAvailableNode),
    }
}

async fn whep(
    State(mut state): State<AppState>,
    Path(stream): Path<String>,
    Query(query_extract): Query<QueryExtract>,
    req: Request,
) -> Result<Response> {
    let mut servers = state.storage.stream_get(stream.clone()).await.unwrap();
    if !query_extract.nodes.is_empty() {
        servers.retain(|x| query_extract.nodes.contains(&x.alias));
    }
    if servers.is_empty() {
        debug!("whep servers is empty");
        return Err(AppError::ResourceNotFound);
    }
    let maximum_idle_node = maximum_idle_node(state.clone(), servers.clone(), stream.clone()).await;

    let target = match maximum_idle_node {
        Some(server) => Some(server),
        None => {
            match cascade::cascade_new_node(state.clone(), servers.clone(), stream.clone()).await {
                Ok(server) => Some(server),
                Err(e) => return Err(e),
            }
        }
    };

    match target {
        Some(server) => {
            debug!("{:?}", server);
            let resp = request_proxy(state.clone(), req, &server).await;
            match resp {
                Ok(mut res) => {
                    let location = session_location_of(&res, "WHEP");
                    record_session_location(&state, location, &stream, &server.alias, "WHEP").await;
                    extra_ice(res.headers_mut(), state.config.extra_ice).await?;
                    Ok(res)
                }
                Err(e) => Err(e),
            }
        }
        None => Err(AppError::NoAvailableNode),
    }
}

async fn session(
    State(mut state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    req: Request,
) -> Result<Response> {
    let session_path = api::path::session(&stream, &session);
    let is_delete = req.method() == http::Method::DELETE;
    match state.storage.session_get(session_path.clone()).await {
        Ok(server) => {
            let res = request_proxy(state.clone(), req, &server).await?;
            // A deleted session leaves the routing table immediately; the
            // node snapshot (SSE or poll) only converges later.
            if is_delete && res.status().is_success() {
                state.storage.session_remove(&session_path).await?;
            }
            Ok(res)
        }
        Err(_) => Err(AppError::ResourceNotFound),
    }
}

pub(crate) async fn request_proxy(
    state: AppState,
    mut req: Request,
    target: &Server,
) -> Result<Response> {
    Span::current().record("target_addr", target.url.clone());
    let path = req.uri().path();
    let path_query = req
        .uri()
        .path_and_query()
        .map(|v| v.as_str())
        .unwrap_or(path);
    let uri = format!("{}{}", target.url, path_query);
    *req.uri_mut() = Uri::try_from(uri).unwrap();
    req.headers_mut().remove("Authorization");
    if !target.token.is_empty() {
        req.headers_mut().insert(
            &header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", target.token))?,
        );
    };

    let (headers, body) = req.into_parts();
    use http_body_util::BodyExt;
    let body = body.collect().await.unwrap().to_bytes();
    let req: Request<axum::body::Bytes> = Request::from_parts(headers, body);
    let req = reqwest::Request::try_from(req).unwrap();

    let res = state
        .client
        .execute(req)
        .await
        .map_err(|_| AppError::RequestProxyError)?;
    let res = http::Response::from(res);
    Ok(res.into_response())
}

async fn maximum_idle_node(
    mut state: AppState,
    mut servers: Vec<Server>,
    stream: String,
) -> Option<Server> {
    // Never route a viewer to an offline node: it has the largest apparent
    // remaining capacity (its snapshot is stale) and the proxy would just
    // fail.
    state.storage.filter_online(&mut servers);
    if servers.is_empty() {
        return None;
    }
    let mut max = 0;
    let mut result = None;
    let info = state.storage.info_raw_all().await.unwrap();
    let infos: Vec<(String, Option<Stream>)> = servers
        .clone()
        .iter()
        .map(|i| {
            let streams = info.get(&i.alias).unwrap().clone();
            let stream = streams.into_iter().find(|x| x.id == stream);
            (i.alias.clone(), stream)
        })
        .collect();
    debug!("{:?}", infos);

    for (alias, i) in infos {
        for s in servers.clone() {
            if s.alias == alias {
                let remain = match i.clone() {
                    Some(x) => {
                        // Closed sessions linger in the node snapshot for
                        // display (a 30 s TTL); they no longer hold capacity.
                        let active = x
                            .subscribe
                            .sessions
                            .iter()
                            .filter(|s| s.state != api::response::RTCPeerConnectionState::Closed)
                            .count();
                        s.sub_max as i32 - active as i32
                    }
                    None => s.sub_max as i32,
                };

                if remain > max {
                    max = remain;
                    result = Some(s);
                }
            }
        }
    }
    result
}
