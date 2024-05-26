use crate::route::r#static::static_server;
use axum::body::Body;
use axum::extract::Request;
use axum::Router;
use clap::Parser;

use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use sqlx::mysql::MySqlConnectOptions;
use sqlx::MySqlPool;
use std::future::Future;
use std::str::FromStr;

use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::{debug, error, info, info_span, warn};

use crate::auth::ManyValidate;
use crate::config::Config;
use crate::route::embed::{Server, EmbedStorage};

mod auth;
mod config;
mod db;
mod error;
mod model;
mod result;
mod route;
mod tick;

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

    let addrs = cluster::cluster_up(5).await;
    println!("{:?}", addrs);

    sqlx::any::install_default_drivers();
    let args = Args::parse();
    let mut cfg = Config::parse(args.config);
    cfg.servers = addrs.iter().enumerate().map(|(i, addr)| Server {
        key: format!("buildin-{}", i),
        url: format!("http://{}", addr),
        ..Default::default()
    }).collect();
    utils::set_log(format!(
        "liveman={},liveion={},http_utils={},webrtc=error",
        cfg.log.level, cfg.log.level, cfg.log.level
    ));

    warn!("set log level : {}", cfg.log.level);
    debug!("config : {:?}", cfg);
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
    sqlx::any::install_default_drivers();
    let args = Args::parse();
    let cfg = Config::parse(args.config);
    utils::set_log(format!(
        "live777_gateway={},sqlx={},webrtc=error",
        cfg.log.level, cfg.log.level
    ));
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
        storage: EmbedStorage::new("live777_db".to_string(), cfg.servers),
    };
    let auth_layer = ValidateRequestHeaderLayer::custom(ManyValidate::new(vec![cfg.auth]));
    let manager_auth_layer =
        ValidateRequestHeaderLayer::custom(ManyValidate::new(vec![cfg.manager_auth]));
    let app = Router::new()
        .merge(route::proxy::route().layer(auth_layer))
        .merge(route::manager::route().layer(manager_auth_layer))
        .merge(route::hook::route())
        .with_state(app_state.clone())
        .layer(if cfg.http.cors {
            CorsLayer::permissive()
        } else {
            CorsLayer::new()
        })
        .layer(axum::middleware::from_fn(
            http_utils::print_request_response,
        ))
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
    //tokio::spawn(tick::run(app_state));
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
    pool: MySqlPool,
    client: Client,
    storage: EmbedStorage,
    //storage: Arc<Box<dyn Storage + 'static + Send + Sync>>,
}
