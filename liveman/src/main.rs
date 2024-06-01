use crate::route::r#static::static_server;
use axum::body::Body;
use axum::extract::Request;
use axum::Router;
use clap::Parser;

use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use std::future::Future;

use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::{debug, error, info, info_span, warn};

use crate::auth::ManyValidate;
use crate::config::Config;
use crate::route::embed::{EmbedStorage, Server};

mod auth;
mod config;
mod error;
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
        let addrs = cluster::cluster_up(cfg.liveion.count, cfg.liveion.address).await;
        info!("{:?}", addrs);

        cfg.servers
            .extend(addrs.iter().enumerate().map(|(i, addr)| Server {
                key: format!("buildin-{}", i),
                url: format!("http://{}", addr),
                pub_max: 1,
                ..Default::default()
            }));
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
        let addrs = cluster::cluster_up(cfg.liveion.count, cfg.liveion.address).await;
        info!("{:?}", addrs);

        cfg.servers
            .extend(addrs.iter().enumerate().map(|(i, addr)| Server {
                key: format!("buildin-{}", i),
                url: format!("http://{}", addr),
                ..Default::default()
            }));
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
        storage: EmbedStorage::new("live777_db".to_string(), cfg.servers),
    };
    let auth_layer = ValidateRequestHeaderLayer::custom(ManyValidate::new(vec![cfg.auth]));
    let app = Router::new()
        .merge(route::proxy::route().layer(auth_layer))
        .with_state(app_state.clone())
        .layer(if cfg.http.cors {
            CorsLayer::permissive()
        } else {
            CorsLayer::new()
        })
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
    tokio::spawn(tick::reforward_check(app_state.clone()));
    axum::serve(listener, static_server(app))
        .with_graceful_shutdown(signal)
        .await
        .unwrap_or_else(|e| error!("Application error: {e}"));
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
    storage: EmbedStorage,
}
