use std::time::Duration;

use crate::{error::AppError, result::Result, AppState};
use axum::{
    extract::{Path, Request, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use chrono::Utc;
use http::Uri;
use sqlx::MySqlPool;
use tracing::Span;

use crate::model::{Node, Stream};

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
}

async fn whip(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    req: Request,
) -> Result<Response> {
    let stream_nodes = Node::find_stream_node(&state.pool, stream.clone()).await?;
    if stream_nodes.is_empty() {
        let node = Node::max_idlest_node(&state.pool).await?;
        match node {
            Some(node) => {
                let resp = request_proxy(state.clone(), req, &node).await;
                if resp.is_ok() && resp.as_ref().unwrap().status().is_success() {
                    let _ = add_node_stream(&node, stream, &state.pool).await;
                }
                resp
            }
            None => Err(AppError::NoAvailableNode),
        }
    } else {
        request_proxy(state.clone(), req, stream_nodes.first().unwrap()).await
    }
}

async fn add_node_stream(node: &Node, stream: String, pool: &MySqlPool) -> Result<Stream> {
    let stream = Stream {
        stream,
        addr: node.addr.clone(),
        publish: 1,
        subscribe: 0,
        reforward: 0,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        id: 0,
    };
    stream.db_save_or_update(pool).await?;
    Ok(stream)
}

async fn whep(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    req: Request,
) -> Result<Response> {
    let nodes: Vec<Node> = Node::find_stream_node(&state.pool, stream.clone()).await?;
    if nodes.is_empty() {
        return Err(AppError::ResourceNotFound);
    }
    let mut nodes_sort = nodes.clone();
    nodes_sort.sort();
    let max_idlest_node = nodes_sort
        .iter()
        .filter(|node| node.available(false))
        .last();
    if let Some(maximum_idle_node) = max_idlest_node {
        request_proxy(state.clone(), req, maximum_idle_node).await
    } else {
        let reforward_node = whep_reforward_node(state.clone(), &nodes, stream.clone()).await?;
        let resp = request_proxy(state.clone(), req, &reforward_node).await;
        if resp.is_ok() && resp.as_ref().unwrap().status().is_success() {
            let _ = add_node_stream(&reforward_node, stream, &state.pool).await;
        }
        resp
    }
}

async fn whep_reforward_node(state: AppState, nodes: &Vec<Node>, stream: String) -> Result<Node> {
    let mut reforward_node = nodes.first().cloned().unwrap();
    for stream_node in nodes {
        if !stream_node.reforward_cascade {
            reforward_node = stream_node.clone();
            break;
        }
    }
    if let Some(target_node) = Node::max_idlest_node(&state.pool).await? {
        reforward_node
            .reforward(&target_node, stream.clone(), stream.clone())
            .await?;
        for _ in 0..state.config.reforward.whep_check_frequency.0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let stream_info = target_node.stream_info(stream.clone()).await;
            if stream_info.is_ok() && stream_info.unwrap().is_some() {
                break;
            }
        }
        Ok(target_node)
    } else {
        Err(AppError::NoAvailableNode)
    }
}

async fn resource(
    State(state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    req: Request,
) -> Result<Response> {
    let nodes = Node::find_stream_node(&state.pool, stream.clone()).await?;
    for node in nodes {
        if let Ok(Some(stream_info)) = node.stream_info(stream.clone()).await {
            if let Some(session_info) = stream_info.publish_session_info {
                if session_info.id == session {
                    return request_proxy(state, req, &node).await;
                }
            }
            for session_info in stream_info.subscribe_session_infos {
                if session_info.id == session {
                    return request_proxy(state, req, &node).await;
                }
            }
        }
    }
    Err(AppError::ResourceNotFound)
}

async fn request_proxy(state: AppState, mut req: Request, target_node: &Node) -> Result<Response> {
    Span::current().record("target_addr", target_node.addr.clone());
    let path = req.uri().path();
    let path_query = req
        .uri()
        .path_and_query()
        .map(|v| v.as_str())
        .unwrap_or(path);
    let uri = format!("http://{}{}", target_node.addr, path_query);
    *req.uri_mut() = Uri::try_from(uri).unwrap();
    req.headers_mut().remove("Authorization");
    if let Some(authorization) = &target_node.authorization {
        req.headers_mut()
            .insert("Authorization", authorization.clone().parse().unwrap());
    };
    Ok(state
        .client
        .request(req)
        .await
        .map_err(|_| AppError::RequestProxyError)
        .into_response())
}
