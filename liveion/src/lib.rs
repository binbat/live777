use std::{future::Future, sync::Arc};

use axum::{extract::Request, middleware, response::IntoResponse, routing::get, Router};
use http::{StatusCode, Uri};
use tokio::net::TcpListener;
use tower_http::{
    cors::CorsLayer, trace::TraceLayer, validate_request::ValidateRequestHeaderLayer,
};
use tracing::{error, info_span, warn, Level};

use auth::{access::access_middleware, ManyValidate};
use error::AppError;

use crate::config::Config;
use crate::route::{admin, session, whep, whip, AppState};

use stream::manager::Manager;

#[cfg(feature = "webui")]
#[derive(rust_embed::RustEmbed)]
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

pub async fn serve<F>(cfg: Config, listener: TcpListener, signal: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let app_state = AppState {
        stream_manager: Arc::new(Manager::new(cfg.clone()).await),
        config: cfg.clone(),
    };
    let auth_layer =
        ValidateRequestHeaderLayer::custom(ManyValidate::new(cfg.auth.secret, cfg.auth.tokens));
    let app = Router::new()
        .merge(
            whip::route()
                .merge(whep::route())
                .merge(session::route())
                .merge(admin::route())
                .merge(crate::route::stream::route())
                .merge(crate::route::strategy::route())
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
        )
        .fallback(static_handler);

    #[cfg(feature = "net4mqtt")]
    {
        if let Some(c) = cfg.net4mqtt {
            std::thread::spawn(move || {
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(async move {
                        loop {
                            match net4mqtt::proxy::agent(
                                &c.mqtt_url,
                                &cfg.http.listen.to_string(),
                                &c.alias.clone(),
                                Some(net4mqtt::proxy::VDataConfig {
                                    online: Some(
                                        serde_json::json!({
                                            "alias": c.alias,
                                        })
                                        .to_string()
                                        .bytes()
                                        .collect(),
                                    ),
                                    offline: Some("{}".bytes().collect()),
                                    ..Default::default()
                                }),
                            )
                            .await
                            {
                                Ok(_) => warn!("net4mqtt service is end, restart net4mqtt service"),
                                Err(e) => error!("mqtt4mqtt error: {:?}", e),
                            }
                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        }
                    });
            });
        }
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(signal)
        .await
        .unwrap_or_else(|e| error!("Application error: {e}"));
}

#[cfg(feature = "webui")]
async fn static_handler(uri: Uri) -> impl IntoResponse {
    let mut path = uri.path().trim_start_matches('/');
    if path.is_empty() {
        path = "index.html";
    }
    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(http::header::CONTENT_TYPE, mime.as_ref())], content.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(not(feature = "webui"))]
async fn static_handler(_: Uri) -> impl IntoResponse {
    (StatusCode::NOT_FOUND, "feature webui not enable")
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
