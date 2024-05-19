use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::routing::get;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Router,
};
use clap::Parser;

use error::AppError;
use http_body_util::BodyExt;
use local_ip_address::local_ip;
use std::future::Future;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::{debug, error, info, info_span, warn};

use crate::auth::ManyValidate;
use crate::config::Config;
use crate::route::r#static::static_server;
use crate::route::{admin, resource, whep, whip, AppState};
use stream::manager::Manager;

mod auth;
mod config;
mod constant;
mod convert;
mod error;
mod forward;
mod hook;
mod metrics;
mod result;
mod route;
mod stream;

#[derive(Parser)]
#[command(version)]
struct Args {
    /// Set config file path
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() {
    metrics_register();
    let args = Args::parse();
    let mut cfg = Config::parse(args.config);
    utils::set_log(format!("live777={},webrtc=error", cfg.log.level));
    warn!("set log level : {}", cfg.log.level);
    debug!("config : {:?}", cfg);
    if cfg.node_addr.is_none() {
        let port = cfg.http.listen.port();
        cfg.node_addr =
            Some(SocketAddr::from_str(&format!("{}:{}", local_ip().unwrap(), port)).unwrap());
        warn!(
            "config node_addr not set, auto detect local_ip_port : {:?}",
            cfg.node_addr.unwrap()
        );
    }

    server_up(cfg, shutdown_signal()).await;
    info!("Server shutdown");
}

async fn server_up<F>(cfg: Config, signal: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind(&cfg.http.listen)
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    info!("Server listening on {}", addr);

    let app_state = AppState {
        stream_manager: Arc::new(Manager::new(cfg.clone()).await),
        config: cfg.clone(),
    };
    let auth_layer = ValidateRequestHeaderLayer::custom(ManyValidate::new(vec![
        cfg.auth,
        cfg.admin_auth.clone(),
    ]));
    let admin_auth_layer =
        ValidateRequestHeaderLayer::custom(ManyValidate::new(vec![cfg.admin_auth]));
    let app = Router::new()
        .merge(
            whip::route()
                .merge(whep::route())
                .merge(resource::route())
                .layer(auth_layer),
        )
        .merge(admin::route().layer(admin_auth_layer))
        .route(live777_http::path::METRICS, get(metrics))
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
                );
                span.record("span_id", span.id().unwrap().into_u64());
                span
            }),
        );

    axum::serve(listener, static_server(app))
        .with_graceful_shutdown(signal)
        .await
        .unwrap_or_else(|e| error!("Application error: {e}"));
    let _ = app_state.stream_manager.shotdown().await;
}

async fn shutdown_signal() {
    debug!("Received signal: {}", signal::wait_for_stop_signal().await)
}

fn metrics_register() {
    metrics::REGISTRY
        .register(Box::new(metrics::STREAM.clone()))
        .unwrap();
    metrics::REGISTRY
        .register(Box::new(metrics::PUBLISH.clone()))
        .unwrap();
    metrics::REGISTRY
        .register(Box::new(metrics::SUBSCRIBE.clone()))
        .unwrap();
    metrics::REGISTRY
        .register(Box::new(metrics::REFORWARD.clone()))
        .unwrap();
}

async fn metrics() -> String {
    metrics::ENCODER
        .encode_to_string(&metrics::REGISTRY.gather())
        .unwrap()
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
