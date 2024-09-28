use axum::extract::Request;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

use error::AppError;
use http::{header, StatusCode, Uri};
use rust_embed::RustEmbed;
use serde_json::json;
use std::future::Future;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::{error, info_span, Level};

use auth::{access::access_middleware, ManyValidate};

use crate::config::Config;
use crate::route::{admin, session, whep, whip, AppState};

use stream::manager::Manager;

#[derive(RustEmbed)]
#[folder = "../assets/liveion/"]
struct Assets;

pub mod config;

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
    let auth_layer =
        ValidateRequestHeaderLayer::custom(ManyValidate::new(cfg.auth.secret, cfg.auth.tokens));
    let mut app = Router::new()
        .merge(
            whip::route()
                .merge(whep::route())
                .merge(session::route())
                .merge(admin::route())
                .merge(crate::route::stream::route())
                .layer(middleware::from_fn(access_middleware))
                .layer(auth_layer),
        )
        .route(api::path::METRICS, get(metrics))
        .with_state(app_state.clone())
        .layer(if cfg.http.cors {
            CorsLayer::permissive()
        } else {
            CorsLayer::new()
        })
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<_>| {
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
                })
                .on_response(tower_http::trace::DefaultOnResponse::new().level(Level::INFO))
                .on_failure(tower_http::trace::DefaultOnFailure::new().level(Level::INFO)),
        );

    app = app.fallback(static_handler);

    #[cfg(feature = "net4mqtt")]
    {
        if let Some(c) = cfg.net4mqtt {
            std::thread::spawn(move || {
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(async move {
                        netmqtt::proxy::agent(
                            &c.mqtt_url,
                            cfg.http.listen,
                            &c.alias.clone(),
                            Some((
                                json!({
                                    "alias": c.alias,
                                })
                                .to_string()
                                .bytes()
                                .collect(),
                                Some("{}".bytes().collect()),
                            )),
                            None,
                        )
                        .await
                        .unwrap()
                    });
            });
        }
    }

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
