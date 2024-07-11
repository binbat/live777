use axum::extract::Request;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

use error::AppError;
use http::{header, StatusCode, Uri};
use rust_embed::RustEmbed;
use std::future::Future;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::{error, info_span};

use crate::auth::ManyValidate;
use crate::config::Config;
use crate::route::{admin, session, whep, whip, AppState};

use stream::manager::Manager;

#[derive(RustEmbed)]
#[folder = "../assets/liveion/"]
struct Assets;

pub mod config;

mod auth;
mod constant;
mod convert;
mod error;
mod forward;
mod hook;
mod r#macro;
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
    let auth_layer = ValidateRequestHeaderLayer::custom(ManyValidate::new(vec![cfg.auth]));
    let mut app = Router::new()
        .merge(
            whip::route()
                .merge(whep::route())
                .merge(session::route())
                .merge(admin::route())
                .merge(crate::route::stream::route())
                .layer(auth_layer),
        )
        .route(api::path::METRICS, get(metrics))
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

    app = app.fallback(static_handler);

    axum::serve(listener, app)
        .with_graceful_shutdown(signal)
        .await
        .unwrap_or_else(|e| error!("Application error: {e}"));
    let _ = app_state.stream_manager.shotdown().await;
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
