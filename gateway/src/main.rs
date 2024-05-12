use crate::result::Result;
use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::routing::get;
use axum::Json;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use axum_extra::extract::Query;
use chrono::Utc;
use clap::Parser;

use error::AppError;
use http::Uri;
use http_body_util::BodyExt;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use live777_http::event::Event;
use model::{Node, Stream};
use sqlx::mysql::MySqlConnectOptions;
use sqlx::MySqlPool;
use std::future::IntoFuture;
use std::str::FromStr;
use std::time::Duration;

#[cfg(debug_assertions)]
use tower_http::services::{ServeDir, ServeFile};

use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::{debug, error, info, info_span, warn, Span};

use crate::auth::ManyValidate;
use crate::config::Config;
#[cfg(not(debug_assertions))]
use {http::header, rust_embed::RustEmbed};

mod auth;
mod config;
mod db;
mod error;
mod model;
mod result;
mod tick;

#[derive(Parser)]
#[command(version)]
struct Args {
    /// Set config file path
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() {
    sqlx::any::install_default_drivers();
    let args = Args::parse();
    let cfg = Config::parse(args.config);
    utils::set_log(format!(
        "live777_gateway={},live777_storage={},sqlx={},webrtc=error",
        cfg.log.level, cfg.log.level, cfg.log.level
    ));
    warn!("set log level : {}", cfg.log.level);
    debug!("config : {:?}", cfg);
    let listener = tokio::net::TcpListener::bind(cfg.http.listen)
        .await
        .unwrap();
    info!("Server listening on {}", listener.local_addr().unwrap());
    let pool_connect_options = MySqlConnectOptions::from_str(&cfg.db_url).unwrap();
    let client: Client =
        hyper_util::client::legacy::Client::<(), ()>::builder(TokioExecutor::new())
            .build(HttpConnector::new());
    let app_state = AppState {
        config: cfg.clone(),
        pool: MySqlPool::connect_with(pool_connect_options)
            .await
            .map_err(|e| anyhow::anyhow!(format!("MySQL error : {}", e)))
            .unwrap(),
        client,
    };
    let auth_layer = ValidateRequestHeaderLayer::custom(ManyValidate::new(vec![cfg.auth]));
    let app = Router::new()
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
        .layer(auth_layer)
        .route("/webhook", post(webhook))
        .with_state(app_state.clone())
        .layer(if cfg.http.cors {
            CorsLayer::permissive()
        } else {
            CorsLayer::new()
        })
        .layer(axum::middleware::from_fn(print_request_response))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                let span = info_span!(
                    "http_request",
                    uri = ?request.uri(),
                    method = ?request.method(),
                    span_id = tracing::field::Empty,
                    target_addr = tracing::field::Empty,
                );
                span.record("span_id", span.id().unwrap().into_u64());
                span
            }),
        );
    tokio::spawn(tick::run(app_state));
    tokio::select! {
        Err(e) = axum::serve(listener, static_server(app)).into_future() => error!("Application error: {e}"),
        msg = signal::wait_for_stop_signal() => debug!("Received signal: {}", msg),
    }
    info!("Server shutdown");
}

#[cfg(not(debug_assertions))]
#[derive(RustEmbed)]
#[folder = "../assets/"]
struct Assets;

fn static_server(router: Router) -> Router {
    #[cfg(debug_assertions)]
    {
        let serve_dir =
            ServeDir::new("assets").not_found_service(ServeFile::new("assets/index.html"));
        router.nest_service("/", serve_dir.clone())
    }
    #[cfg(not(debug_assertions))]
    {
        router.fallback(static_handler)
    }
}

#[cfg(not(debug_assertions))]
async fn static_handler(uri: Uri) -> impl IntoResponse {
    let mut path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        path = "index.html";
    }
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

type Client = hyper_util::client::legacy::Client<HttpConnector, Body>;

#[derive(Clone)]
struct AppState {
    config: Config,
    pool: MySqlPool,
    client: Client,
}

async fn print_request_response(
    req: Request,
    next: Next,
) -> std::result::Result<impl IntoResponse, (StatusCode, String)> {
    let req_headers = req.headers().clone();
    let (parts, body) = req.into_parts();
    let bytes = buffer_and_print("request", req_headers, body).await?;
    let req = Request::from_parts(parts, Body::from(bytes));

    let res = next.run(req).await;
    let res_headers = res.headers().clone();
    let (parts, body) = res.into_parts();
    let bytes = buffer_and_print("response", res_headers, body).await?;
    let res = Response::from_parts(parts, Body::from(bytes));

    Ok(res)
}

async fn buffer_and_print<B>(
    direction: &str,
    headers: HeaderMap,
    body: B,
) -> std::result::Result<Bytes, (StatusCode, String)>
where
    B: axum::body::HttpBody<Data = Bytes>,
    B::Error: std::fmt::Display,
{
    let bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(err) => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("failed to read {direction} body: {err}"),
            ));
        }
    };

    if let Ok(body) = std::str::from_utf8(&bytes) {
        debug!("{direction} headers = {headers:?} body = {body:?}");
    }

    Ok(bytes)
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
        publish: 0,
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

use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
struct WebHookQuery {
    token: String,
    reforward_maximum_idle_time: Option<u64>,
    reforward_cascade: Option<bool>,
}

impl WebHookQuery {
    fn get_reforward_maximum_idle_time(&self) -> u64 {
        if let Some(reforward_maximum_idle_time) = self.reforward_maximum_idle_time {
            reforward_maximum_idle_time
        } else {
            0
        }
    }
    fn get_reforward_cascade(&self) -> bool {
        if let Some(reforward_cascade) = self.reforward_cascade {
            reforward_cascade
        } else {
            false
        }
    }
}

async fn webhook(
    State(state): State<AppState>,
    Query(qry): Query<WebHookQuery>,
    Json(event_body): Json<live777_http::event::EventBody>,
) -> Result<String> {
    let pool = &state.pool;
    let addr = event_body.addr;
    let metrics = event_body.metrics;
    let mut node = Node {
        addr: addr.to_string(),
        stream: metrics.stream,
        publish: metrics.publish,
        subscribe: metrics.subscribe,
        reforward: metrics.reforward,
        reforward_maximum_idle_time: qry.get_reforward_maximum_idle_time(),
        reforward_cascade: qry.get_reforward_cascade(),
        ..Default::default()
    };
    match event_body.event {
        Event::Node { r#type, metadata } => {
            node.authorization = metadata.authorization;
            node.admin_authorization = metadata.admin_authorization;
            node.pub_max = metadata.pub_max;
            node.sub_max = metadata.sub_max;
            match r#type {
                live777_http::event::NodeEventType::Up => node.db_save_or_update(pool).await?,
                live777_http::event::NodeEventType::Down => {
                    node.db_remove(pool).await?;
                    Stream::db_remove_addr_stream(pool, addr.to_string()).await?
                }
                live777_http::event::NodeEventType::KeepAlive => {
                    if node.db_update_metrics(pool).await.is_err() {
                        node.db_save_or_update(pool).await?;
                    }
                }
            }
        }
        Event::Stream { r#type, stream } => {
            let _ = node.db_update_metrics(pool).await;
            let db_stream = Stream {
                stream: stream.stream,
                addr: addr.to_string(),
                publish: stream.publish,
                subscribe: stream.subscribe,
                reforward: stream.reforward,
                ..Default::default()
            };
            match r#type {
                live777_http::event::StreamEventType::StreamUp => {
                    db_stream.db_save_or_update(pool).await?
                }
                live777_http::event::StreamEventType::StreamDown => {
                    db_stream.db_remove(pool).await?
                }
                _ => {
                    db_stream.db_update_metrics(pool).await?;
                }
            }
        }
    }
    Ok("".to_string())
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
