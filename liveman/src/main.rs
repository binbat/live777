use axum::body::Body;
use axum::extract::Request;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;

use clap::Parser;
use http::{header, StatusCode, Uri};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use rust_embed::RustEmbed;
use std::future::Future;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::{debug, error, info, info_span, warn};

use auth::{access::access_middleware, ManyValidate};

use crate::admin::{authorize, token};
use crate::config::Config;
use crate::mem::{MemStorage, Server};

#[derive(RustEmbed)]
#[folder = "../assets/liveman/"]
struct Assets;

mod admin;
mod config;
mod error;
mod mem;
mod result;
mod route;
mod tick;

#[cfg(feature = "liveion")]
mod cluster;

#[derive(Parser)]
#[command(version)]
struct Args {
    /// Set config file path
    #[arg(short, long)]
    config: Option<String>,
}

#[cfg(debug_assertions)]
#[tokio::main]
async fn main() {
    let args = Args::parse();

    #[cfg(feature = "liveion")]
    let mut cfg = Config::parse(args.config);

    #[cfg(not(feature = "liveion"))]
    let cfg = Config::parse(args.config);

    utils::set_log(format!(
        "liveman={},liveion={},http_log={},webrtc=error",
        cfg.log.level, cfg.log.level, cfg.log.level
    ));

    warn!("set log level : {}", cfg.log.level);
    debug!("config : {:?}", cfg);

    #[cfg(feature = "liveion")]
    {
        let servers = cluster::cluster_up(cfg.liveion.clone()).await;
        info!("liveion buildin servers: {:?}", servers);
        cfg.nodes.extend(servers)
    }
    let listener = tokio::net::TcpListener::bind(cfg.http.listen)
        .await
        .unwrap();
    info!("Server listening on {}", listener.local_addr().unwrap());

    server_up(cfg, listener, shutdown_signal()).await;
    info!("Server shutdown");
}

#[cfg(not(debug_assertions))]
#[tokio::main]
async fn main() {
    let args = Args::parse();

    #[cfg(feature = "liveion")]
    let mut cfg = Config::parse(args.config);

    #[cfg(not(feature = "liveion"))]
    let cfg = Config::parse(args.config);

    utils::set_log(format!(
        "liveman={},http_log={},webrtc=error",
        cfg.log.level, cfg.log.level
    ));

    #[cfg(feature = "liveion")]
    {
        let servers = cluster::cluster_up(cfg.liveion.clone()).await;
        info!("liveion buildin servers: {:?}", servers);
        cfg.nodes.extend(servers)
    }

    warn!("set log level : {}", cfg.log.level);
    debug!("config : {:?}", cfg);
    let listener = tokio::net::TcpListener::bind(cfg.http.listen)
        .await
        .unwrap();
    info!("Server listening on {}", listener.local_addr().unwrap());

    server_up(cfg, listener, shutdown_signal()).await;
    info!("Server shutdown");
}

pub async fn server_up<F>(cfg: Config, listener: TcpListener, signal: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let client: Client =
        hyper_util::client::legacy::Client::<(), ()>::builder(TokioExecutor::new())
            .build(HttpConnector::new());
    let app_state = AppState {
        config: cfg.clone(),
        client,
        storage: MemStorage::new(cfg.nodes),
    };

    let auth_layer =
        ValidateRequestHeaderLayer::custom(ManyValidate::new(cfg.auth.secret, cfg.auth.tokens));
    let mut app = Router::new()
        .merge(
            route::proxy::route()
                .route("/api/token", post(token))
                .layer(middleware::from_fn(access_middleware))
                .layer(auth_layer),
        )
        .layer(if cfg.http.cors {
            CorsLayer::permissive()
        } else {
            CorsLayer::new()
        })
        .route("/api/login", post(authorize))
        .with_state(app_state.clone())
        .layer(axum::middleware::from_fn(http_log::print_request_response))
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

    app = app.fallback(static_handler);

    tokio::spawn(tick::reforward_check(app_state.clone()));
    axum::serve(listener, app)
        .with_graceful_shutdown(signal)
        .await
        .unwrap_or_else(|e| error!("Application error: {e}"));
}

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

async fn shutdown_signal() {
    let str = signal::wait_for_stop_signal().await;
    debug!("Received signal: {}", str);
}

type Client = hyper_util::client::legacy::Client<HttpConnector, Body>;

#[derive(Clone)]
struct AppState {
    config: Config,
    client: Client,
    storage: MemStorage,
}
