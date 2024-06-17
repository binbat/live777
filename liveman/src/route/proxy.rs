use axum::{
    extract::{Path, Request, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use http::Uri;
use std::collections::HashSet;
use tracing::{debug, error, info, warn, Span};

use api::response::Stream;

use crate::route::utils::{force_check_times, reforward, session_delete};
use crate::Server;
use crate::{error::AppError, result::Result, AppState};

pub fn route() -> Router<AppState> {
    Router::new()
        .route(&api::path::whip(":stream"), post(whip))
        .route(&api::path::whep(":stream"), post(whep))
        .route(
            &api::path::session(":stream", ":session"),
            post(session).patch(session).delete(session),
        )
        .route(
            &api::path::session_layer(":stream", ":session"),
            get(session).post(session).delete(session),
        )
        .route("/admin/infos", get(info))
}

async fn info(
    State(mut state): State<AppState>,
    _req: Request,
) -> crate::result::Result<Json<Vec<api::response::Stream>>> {
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
        Some(server) => {
            let resp = request_proxy(state.clone(), req, &server).await;
            match resp {
                Ok(res) => {
                    if res.status().is_success() {
                        match res.headers().get("Location") {
                            Some(location) => {
                                state
                                    .storage
                                    .stream_put(stream.clone(), server.clone())
                                    .await
                                    .unwrap();

                                state
                                    .storage
                                    .session_put(String::from(location.to_str().unwrap()), server)
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
            match resp {
                Ok(res) => {
                    if res.status().is_success() {
                        match res.headers().get("Location") {
                            Some(location) => state
                                .storage
                                .session_put(String::from(location.to_str().unwrap()), server)
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
                    for sub_info in stream_info.subscribe.sessions.into_iter() {
                        match sub_info.reforward {
                            Some(v) => info!("Skip. Is Reforward: {:?}", v),
                            None => {
                                match session_delete(server.clone(), stream.clone(), sub_info.id)
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
    let infos: Vec<(String, Option<Stream>)> = servers
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
