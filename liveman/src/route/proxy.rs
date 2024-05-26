use crate::{error::AppError, result::Result, AppState};
use axum::{
    extract::{Path, Request, State}, response::{IntoResponse, Response}, routing::{get, post}, Json, Router
};
use http::Uri;
use live777_http::response::StreamInfo;
use tracing::{debug, error, warn, Span};
use crate::Server;

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
        .route(
            "/admin/infos",
            get(info),
        )
}

async fn info(
    State(mut state): State<AppState>,
    _req: Request,
) -> crate::result::Result<Json<Vec<live777_http::response::StreamInfo>>> {
    Ok(Json(
        state.storage.info_all().await.unwrap()
    ))
}

async fn whip(
    State(mut state): State<AppState>,
    Path(stream): Path<String>,
    req: Request,
) -> Result<Response> {
    let stream_nodes = state.storage.stream_get(stream.clone()).await?;
    debug!("{:?}", stream_nodes);
    if stream_nodes.is_empty() {
        let nodes = state.storage.nodes().await;
        warn!("{:?}", nodes);
        if nodes.is_empty() {
            return Err(AppError::NoAvailableNode);
        };
        match maximum_idle_node(state.clone(), nodes, stream.clone()).await? {
            Some(node) => {
                warn!("node: {:?}", node);
                let resp = request_proxy(state.clone(), req, &node).await;
                match resp.as_ref() {
                    Ok(res) => {
                        match res.headers().get("Location") {
                            Some(location) => {
                                //state.storage.registry_stream(node.addr, stream).await;
                                //state.storage.put_resource(String::from(location.to_str().unwrap()), node).await.unwrap();
                                state.storage.stream_put(stream, node.clone()).await.unwrap();
                                state.storage.resource_put(String::from(location.to_str().unwrap()), node).await.unwrap();
                            },
                            None => error!("WHIP Error: Location not found"),
                        }
                    }
                    Err(e) => {
                        error!("WHIP Error: {:?}", e);
                    }
                }
                resp
            },
            None => Err(AppError::NoAvailableNode),
        }
    } else {
        request_proxy(state.clone(), req, stream_nodes.first().unwrap()).await
    }
}

async fn whep(
    State(mut state): State<AppState>,
    Path(stream): Path<String>,
    req: Request,
) -> Result<Response> {
    let servers = state.storage.stream_get(stream.clone()).await.unwrap();
    if servers.is_empty() {
        return Err(AppError::ResourceNotFound);
    }
    let maximum_idle_node = maximum_idle_node(state.clone(), servers.clone(), stream.clone()).await.unwrap();
    match maximum_idle_node {
        Some(maximum_idle_node) => {
            debug!("{:?}", maximum_idle_node);
            let resp = request_proxy(state.clone(), req, &maximum_idle_node).await;
            match resp.as_ref() {
                Ok(res) => {
                    match res.headers().get("Location") {
                        Some(location) =>
                        state.storage.resource_put(String::from(location.to_str().unwrap()), maximum_idle_node).await.unwrap(),
                        None => error!("WHEP Error: Location not found"),
                    }
                }
                Err(e) => error!("WHEP Error: {:?}", e),
            }
            resp
        },
        None => {
            let reforward_node = whep_reforward_node(state.clone(), &servers, stream).await?;
            request_proxy(state.clone(), req, &reforward_node).await
        }
    }
}

async fn whep_reforward_node(state: AppState, nodes: &Vec<Server>, stream: String) -> Result<Server> {
    Ok(nodes.first().unwrap().clone())
    //let mut reforward_node = stream_nodes.first().cloned().unwrap();
    //for stream_node in stream_nodes {
    //    if !stream_node.metadata.stream_info.reforward_cascade {
    //        reforward_node = stream_node.clone();
    //        break;
    //    }
    //}
    //let nodes = state.storage.nodes().await?;
    //if nodes.is_empty() {
    //    return Err(AppError::NoAvailableNode);
    //}
    //if let Some(target_node) = live777_storage::node_operate::maximum_idle_node(nodes, true).await?
    //{
    //    reforward_node
    //        .reforward(&target_node, stream.clone(), stream.clone())
    //        .await?;
    //    for _ in 0..state.config.reforward.reforward_check_frequency.0 {
    //        let timeout = tokio::time::sleep(Duration::from_millis(50));
    //        tokio::pin!(timeout);
    //        let _ = timeout.as_mut().await;
    //        let stream_info = target_node.stream_info(stream.clone()).await;
    //        if stream_info.is_ok() && stream_info.unwrap().is_some() {
    //            break;
    //        }
    //    }
    //    Ok(target_node)
    //} else {
    //    Err(AppError::NoAvailableNode)
    //}


    //let mut reforward_node = nodes.first().cloned().unwrap();
    //for stream_node in nodes {
    //    if !stream_node.reforward_cascade {
    //        reforward_node = stream_node.clone();
    //        break;
    //    }
    //}
    //if let Some(target_node) = Node::max_idlest_node(&state.pool).await? {
    //    reforward_node
    //        .reforward(&target_node, stream.clone(), stream.clone())
    //        .await?;
    //    for _ in 0..state.config.reforward.whep_check_frequency.0 {
    //        tokio::time::sleep(Duration::from_millis(50)).await;
    //        let stream_info = target_node.stream_info(stream.clone()).await;
    //        if stream_info.is_ok() && stream_info.unwrap().is_some() {
    //            break;
    //        }
    //    }
    //    Ok(target_node)
    //} else {
    //    Err(AppError::NoAvailableNode)
    //}
}

async fn resource(
    State(mut state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    req: Request,
) -> Result<Response> {
    let resource = format!("/resource/{}/{}", stream, session);
    match state.storage.resource_get(resource).await {
        Ok(server) => request_proxy(state, req, &server).await,
        Err(_) => Err(AppError::ResourceNotFound)
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

async fn maximum_idle_node(mut state: AppState, servers: Vec<Server>, stream: String) -> Result<Option<Server>> {
    let mut max = 0;
    let mut result = None;
    let info = state.storage.info_raw_all().await.unwrap();
    let infos: Vec<(String, Option<StreamInfo>)> = servers.clone().iter()
        .map(|i| {
            let streams = info.get(&i.key).unwrap().clone();
            let stream =  streams
                .into_iter()
                .find(|x| x.id == stream);
            (i.key.clone(), stream)
        })
        .collect();
    debug!("{:?}", infos);

    for (key, i) in infos {
        for s in servers.clone() {
            if s.key == key {
                let remain = match i.clone() {
                    Some(x) => s.pub_max - x.subscribe_session_infos.len() as u16,
                    None => s.pub_max,
                };

                if remain > max {
                    max = remain;
                    result = Some(s);
                }

            }
        }
    }
    Ok(result)
}
