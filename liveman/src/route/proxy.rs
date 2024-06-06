use axum::{
    extract::{Path, Request, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use http::Uri;
use std::collections::{HashMap, HashSet};
use tracing::{debug, error, info, warn, Span};

use live777_http::response::StreamInfo;

use crate::route::utils::{force_check_times, reforward, resource_delete};
use crate::Server;
use crate::{error::AppError, result::Result, AppState};

pub fn route() -> Router<AppState> {
    Router::new()
        .route(&live777_http::path::whip(":stream"), post(whip))
        .route(&live777_http::path::whep(":stream"), post(whep))
        .route(
            &live777_http::path::resource(":stream", ":session"),
            post(resource).patch(resource).delete(resource),
        )
        .route(
            &live777_http::path::resource_layer(":stream", ":session"),
            get(resource).post(resource).delete(resource),
        )
        .route("/admin/infos", get(info))
        .route("/api/admin/-/infos", get(api_info))
        .route("/api/whip/:alias/:stream", post(api_whip))
        .route("/api/whep/:alias/:stream", post(api_whep))
}

async fn api_info(
    State(mut state): State<AppState>,
    _req: Request,
) -> crate::result::Result<Json<HashMap<String, Vec<live777_http::response::StreamInfo>>>> {
    Ok(Json(state.storage.info_raw_all().await.unwrap()))
}

async fn api_whip(
    State(state): State<AppState>,
    Path((alias, stream)): Path<(String, String)>,
    mut req: Request,
) -> Result<Response> {
    let uri = format!("/whip/{}", stream);
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
    let uri = format!("/whep/{}", stream);
    *req.uri_mut() = Uri::try_from(uri).unwrap();

    match state.storage.get_map_server().get(&alias) {
        Some(server) => request_proxy(state, req, server).await,
        None => Err(AppError::NoAvailableNode),
    }
}

async fn info(
    State(mut state): State<AppState>,
    _req: Request,
) -> crate::result::Result<Json<Vec<live777_http::response::StreamInfo>>> {
    Ok(Json(state.storage.info_all().await.unwrap()))
}

async fn whip(
    State(mut state): State<AppState>,
    Path(stream): Path<String>,
    req: Request,
) -> Result<Response> {
    let stream_nodes = state.storage.stream_get(stream.clone()).await?;
    debug!("{:?}", stream_nodes);
    let target = match stream_nodes.is_empty() {
        true => {
            let nodes = state.storage.nodes().await;
            warn!("{:?}", nodes);
            maximum_idle_node(state.clone(), nodes, stream.clone()).await
        }
        false => match stream_nodes.first() {
            Some(node) => Some(node.clone()),
            None => {
                error!("WHIP Error: No available node");
                None
            }
        },
    };

    match target {
        Some(node) => {
            let resp = request_proxy(state.clone(), req, &node).await;
            match resp.as_ref() {
                Ok(res) => match res.headers().get("Location") {
                    Some(location) => {
                        state
                            .storage
                            .stream_put(stream, node.clone())
                            .await
                            .unwrap();
                        state
                            .storage
                            .resource_put(String::from(location.to_str().unwrap()), node.clone())
                            .await
                            .unwrap();
                    }
                    None => error!("WHIP Error: Location not found"),
                },
                Err(e) => {
                    error!("WHIP Error: {:?}", e);
                }
            }
            resp
        }
        None => Err(AppError::NoAvailableNode),
    }
}

async fn whep(
    State(mut state): State<AppState>,
    Path(stream): Path<String>,
    req: Request,
) -> Result<Response> {
    let servers = state.storage.stream_get(stream.clone()).await.unwrap();
    if servers.is_empty() {
        debug!("whep servers is empty");
        return Err(AppError::ResourceNotFound);
    }
    let maximum_idle_node = maximum_idle_node(state.clone(), servers.clone(), stream.clone()).await;

    let target = match maximum_idle_node {
        Some(server) => Some(server),
        None => match whep_reforward_node(state.clone(), servers.clone(), stream.clone()).await {
            Ok(server) => Some(server),
            Err(e) => return Err(e),
        },
    };

    match target {
        Some(server) => {
            debug!("{:?}", server);
            let resp = request_proxy(state.clone(), req, &server).await;
            match resp.as_ref() {
                Ok(res) => match res.headers().get("Location") {
                    Some(location) => state
                        .storage
                        .resource_put(String::from(location.to_str().unwrap()), server)
                        .await
                        .unwrap(),
                    None => error!("WHEP Error: Location not found {:?}", res),
                },
                Err(e) => error!("WHEP Error: {:?}", e),
            }
            resp
        }
        None => {
            error!("WHEP Error: No available node");
            Err(AppError::NoAvailableNode)
        }
    }
}

async fn whep_reforward_node(
    mut state: AppState,
    nodes: Vec<Server>,
    stream: String,
) -> Result<Server> {
    let set_all: HashSet<Server> = state.storage.nodes().await.into_iter().clone().collect();
    let set_src: HashSet<Server> = nodes.clone().into_iter().collect();
    let set_dst: HashSet<&Server> = set_all.difference(&set_src).collect();

    let arr = set_dst.into_iter().collect::<Vec<&Server>>();

    let server_src = nodes.first().unwrap().clone();
    let server_dst = *arr.first().unwrap();

    info!("reforward from: {:?}, to: {:?}", server_src, server_dst);

    match reforward(server_src.clone(), server_dst.clone(), stream.clone()).await {
        Ok(()) => {
            match force_check_times(
                server_dst.clone(),
                stream.clone(),
                state.config.reforward.check_attempts.0,
            )
            .await
            {
                Ok(count) => {
                    if state.config.reforward.close_other_sub {
                        reforward_close_other_sub(state, server_src, stream).await
                    }
                    info!("reforward success, checked attempts: {}", count)
                }
                Err(e) => error!("reforward check error: {:?}", e),
            }
            Ok(server_dst.clone())
        }
        Err(e) => {
            error!("reforward error: {:?}", e);
            Err(AppError::InternalServerError(e))
        }
    }
}

async fn reforward_close_other_sub(mut state: AppState, server: Server, stream: String) {
    match state.storage.info_get(server.clone().key).await {
        Ok(streams) => {
            for stream_info in streams.into_iter() {
                if stream_info.id == stream {
                    for sub_info in stream_info.subscribe_session_infos.into_iter() {
                        match sub_info.reforward {
                            Some(v) => info!("Skip. Is Reforward: {:?}", v),
                            None => {
                                match resource_delete(server.clone(), stream.clone(), sub_info.id)
                                    .await
                                {
                                    Ok(_) => {}
                                    Err(e) => error!("reforward close other sub error: {:?}", e),
                                }
                            }
                        }
                    }
                }
            }
        }
        Err(e) => error!("reforward don't closed other sub: {:?}", e),
    }
}

async fn resource(
    State(mut state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    req: Request,
) -> Result<Response> {
    let resource = format!("/resource/{}/{}", stream, session);
    match state.storage.resource_get(resource).await {
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
    //if let Some(authorization) = &target_node.authorization {
    //    req.headers_mut()
    //        .insert("Authorization", authorization.clone().parse().unwrap());
    //};
    Ok(state
        .client
        .request(req)
        .await
        .map_err(|_| AppError::RequestProxyError)
        .into_response())
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
    let infos: Vec<(String, Option<StreamInfo>)> = servers
        .clone()
        .iter()
        .map(|i| {
            let streams = info.get(&i.key).unwrap().clone();
            let stream = streams.into_iter().find(|x| x.id == stream);
            (i.key.clone(), stream)
        })
        .collect();
    debug!("{:?}", infos);

    for (key, i) in infos {
        for s in servers.clone() {
            if s.key == key {
                let remain = match i.clone() {
                    Some(x) => s.pub_max as i32 - x.subscribe_session_infos.len() as i32,
                    None => s.pub_max as i32,
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
