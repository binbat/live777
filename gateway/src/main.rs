use crate::result::Result;
use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::routing::get;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use clap::Parser;

use error::AppError;
use http::Uri;
use http_body_util::BodyExt;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use live777_storage::node_operate::Node;
use live777_storage::Storage;
use std::env;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::EnvFilter;

#[cfg(debug_assertions)]
use tower_http::services::{ServeDir, ServeFile};

use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::info_span;
use tracing::{debug, error, info};

use crate::auth::ManyValidate;
use crate::config::Config;
#[cfg(not(debug_assertions))]
use {http::header, rust_embed::RustEmbed};

mod auth;
mod config;
mod convert;
mod error;
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
    let args = Args::parse();
    let cfg = Config::parse(args.config);
    set_log(format!(
        "live777_gateway={},live777_storage={},webrtc=error",
        cfg.log.level, cfg.log.level
    ));
    debug!("config : {:?}", cfg);
    let addr = SocketAddr::from_str(&cfg.http.listen).expect("invalid listen address");
    info!("Server listening on {}", addr);
    let client: Client =
        hyper_util::client::legacy::Client::<(), ()>::builder(TokioExecutor::new())
            .build(HttpConnector::new());
    let app_state = AppState {
        config: cfg.clone(),
        storage: Arc::new(live777_storage::new(cfg.storage.into()).await.unwrap()),
        client,
    };
    tokio::spawn(tick::reforward_check(app_state.clone()));
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
        .with_state(app_state)
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
                );
                span.record("span_id", span.id().unwrap().into_u64());
                span
            }),
        );
    tokio::select! {
        Err(e) = axum::serve(tokio::net::TcpListener::bind(&addr).await.unwrap(), static_server(app)).into_future() => error!("Application error: {e}"),
        msg = signal::wait_for_stop_signal() => debug!("Received signal: {}", msg),
    }
    info!("Server shutdown");
}

fn set_log(env_filter: String) {
    let _ = env::var("RUST_LOG").is_err_and(|_| {
        env::set_var("RUST_LOG", env_filter);
        true
    });
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .init();
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
    storage: Arc<Box<dyn Storage + 'static + Send + Sync>>,
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
    let stream_nodes = state.storage.stream_nodes(stream).await?;
    if stream_nodes.is_empty() {
        let nodes = state.storage.nodes().await?;
        if nodes.is_empty() {
            return Err(AppError::NoAvailableNode);
        };
        match live777_storage::node_operate::maximum_idle_node(nodes, true).await? {
            Some(node) => request_proxy(state.clone(), req, &node).await,
            None => Err(AppError::NoAvailableNode),
        }
    } else {
        request_proxy(state.clone(), req, stream_nodes.first().unwrap()).await
    }
}

async fn whep(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    req: Request,
) -> Result<Response> {
    let stream_nodes = state.storage.stream_nodes(stream.clone()).await?;
    if stream_nodes.is_empty() {
        return Err(AppError::ResourceNotFound);
    }
    let maximum_idle_node =
        live777_storage::node_operate::maximum_idle_node(stream_nodes.clone(), false).await?;
    match maximum_idle_node {
        Some(maximum_idle_node) => request_proxy(state.clone(), req, &maximum_idle_node).await,
        None => {
            let reforward_node = whep_reforward_node(state.clone(), &stream_nodes, stream).await?;
            request_proxy(state.clone(), req, &reforward_node).await
        }
    }
}

async fn whep_reforward_node(
    state: AppState,
    stream_nodes: &Vec<Node>,
    stream: String,
) -> Result<Node> {
    let mut reforward_node = stream_nodes.first().cloned().unwrap();
    for stream_node in stream_nodes {
        if !stream_node.metadata.stream_info.reforward_cascade {
            reforward_node = stream_node.clone();
            break;
        }
    }
    let nodes = state.storage.nodes().await?;
    if nodes.is_empty() {
        return Err(AppError::NoAvailableNode);
    }
    if let Some(target_node) = live777_storage::node_operate::maximum_idle_node(nodes, true).await?
    {
        reforward_node
            .reforward(&target_node, stream.clone(), stream.clone())
            .await?;
        for _ in 0..state.config.reforward.reforward_check_frequency.0 {
            let timeout = tokio::time::sleep(Duration::from_millis(50));
            tokio::pin!(timeout);
            let _ = timeout.as_mut().await;
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
    let stream_nodes = state.storage.stream_nodes(stream.clone()).await?;
    for node in stream_nodes {
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
    let path = req.uri().path();
    let path_query = req
        .uri()
        .path_and_query()
        .map(|v| v.as_str())
        .unwrap_or(path);
    let uri = format!("http://{}{}", target_node.addr, path_query);
    *req.uri_mut() = Uri::try_from(uri).unwrap();
    Ok(state
        .client
        .request(req)
        .await
        .map_err(|_| AppError::RequestProxyError)
        .into_response())
}
