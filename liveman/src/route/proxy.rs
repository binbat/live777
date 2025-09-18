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
}

async fn api_whip(
    State(state): State<AppState>,
    Path((alias, stream)): Path<(String, String)>,
    mut req: Request,
) -> Result<Response> {
    let uri = format!("/whip/{stream}");
    *req.uri_mut() = Uri::try_from(uri).unwrap();

    match state.storage.get_map_server().get(&alias) {
        Some(server) => request_proxy(state, req, server).await,
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

    match state.storage.get_map_server().get(&alias) {
        Some(server) => request_proxy(state, req, server).await,
        None => Err(AppError::NoAvailableNode),
    }
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
                    extra_ice(res.headers_mut(), state.config.extra_ice).await?;

                    if res.status().is_success() {
                        match res.headers().get(header::LOCATION) {
                            Some(location) => {
                                state
                                    .storage
                                    .stream_put(stream.clone(), server.alias.clone())
                                    .await
                                    .unwrap();

                                state
                                    .storage
                                    .session_put(
                                        String::from(location.to_str().unwrap()),
                                        server.alias,
                                    )
                                    .await
                                    .unwrap();
                            }
                            None => error!("WHIP Error: Location not found"),
                        };
                        Ok(res)
                    } else {
                        error!("WHIP Error: {:?}", res);
                        Ok(res)
                    }
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
                    extra_ice(res.headers_mut(), state.config.extra_ice).await?;

                    if res.status().is_success() {
                        match res.headers().get(header::LOCATION) {
                            Some(location) => state
                                .storage
                                .session_put(String::from(location.to_str().unwrap()), server.alias)
                                .await
                                .unwrap(),
                            None => error!("WHEP Error: Location not found {:?}", res),
                        };
                        Ok(res)
                    } else {
                        error!("WHEP Error: {:?}", res);
                        Ok(res)
                    }
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
    let session = api::path::session(&stream, &session);
    match state.storage.session_get(session).await {
        Ok(server) => request_proxy(state, req, &server).await,
        Err(_) => Err(AppError::ResourceNotFound),
    }
}

async fn request_proxy(state: AppState, mut req: Request, target: &Server) -> Result<Response> {
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
    servers: Vec<Server>,
    stream: String,
) -> Option<Server> {
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
                    Some(x) => s.sub_max as i32 - x.subscribe.sessions.len() as i32,
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
