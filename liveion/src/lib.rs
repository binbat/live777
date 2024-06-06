use axum::extract::Request;
use axum::routing::get;
use axum::Router;

use error::AppError;
use std::future::Future;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::{error, info_span};

use crate::auth::ManyValidate;
use crate::config::Config;
use crate::route::r#static::static_server;
use crate::route::{admin, resource, whep, whip, AppState};
use stream::manager::Manager;

pub mod config;

mod auth;
mod constant;
mod convert;
mod error;
mod forward;
mod hook;
mod metrics;
mod result;
mod route;
mod stream;

pub async fn server_up<F>(cfg: Config, listener: TcpListener, signal: F)
where
    F: Future<Output = ()> + Send + 'static,
{
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
        .layer(axum::middleware::from_fn(http_log::print_request_response))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<_>| {
                let span = info_span!(
                    "http_request",
                    uri = ?request.uri(),
                    method = ?request.method(),
                    span_id = tracing::field::Empty,
                );
                span.record(
                    "span_id",
                    span.id().unwrap_or(tracing::Id::from_u64(42)).into_u64(),
                );
                span
            }),
        );

    axum::serve(listener, static_server(app))
        .with_graceful_shutdown(signal)
        .await
        .unwrap_or_else(|e| error!("Application error: {e}"));
    let _ = app_state.stream_manager.shotdown().await;
}

pub fn metrics_register() {
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
