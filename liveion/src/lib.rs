use std::{future::Future, sync::Arc};

#[cfg(feature = "net4mqtt")]
use std::time::Duration;

use axum::{Router, extract::Request, middleware, response::IntoResponse, routing::get};
use http::{StatusCode, Uri};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{Level, error, info_span};

use crate::config::Config;
use crate::route::{AppState, admin, info, session, whep, whip};
use api::path;
use auth::{AuthState, access::access_middleware, validate_middleware};
use error::AppError;

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

#[cfg(feature = "recorder")]
pub mod recorder;

pub async fn serve<F>(cfg: Config, listener: TcpListener, signal: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let app_state = AppState {
        stream_manager: Arc::new(Manager::new(cfg.clone()).await),
        config: cfg.clone(),
    };

    #[cfg(feature = "recorder")]
    {
        crate::recorder::init(app_state.stream_manager.clone(), cfg.recorder.clone()).await;
    }

    #[cfg(feature = "source")]
    {
        let total: usize = cfg.stream.streams.values().map(|e| e.sources.len()).sum();
        if total > 0 {
            tracing::info!("[Server] Auto-starting {} configured sources...", total);

            if let Err(e) = app_state
                .stream_manager
                .auto_start_sources(&cfg.stream)
                .await
            {
                tracing::error!("Failed to auto-start sources: {:?}", e);
            } else {
                tracing::info!("All configured sources started successfully");
            }
        } else {
            tracing::info!("No sources configured for auto-start");
        }
    }
    let app = Router::new().merge(
        whip::route()
            .merge(whep::route())
            .merge(session::route())
            .merge(admin::route())
            .merge(crate::route::stream::route())
            .merge(crate::route::recorder::route())
            .merge(crate::route::strategy::route())
            .merge({
                #[cfg(feature = "source")]
                {
                    crate::route::source::route()
                }
                #[cfg(not(feature = "source"))]
                {
                    Router::new()
                }
            })
            .layer(middleware::from_fn(access_middleware))
            .layer(middleware::from_fn_with_state(
                AuthState::new(cfg.auth.secret, cfg.auth.tokens),
                validate_middleware,
            )),
    );

    let app = app
        .route(path::METRICS, get(metrics))
        .merge(info::route())
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

    let cancel = CancellationToken::new();

    #[cfg(feature = "net4mqtt")]
    {
        if let Some(mut c) = cfg.net4mqtt {
            c.validate();
            let stream_manager = app_state.stream_manager.clone();
            let cancel_net4mqtt = cancel.clone();
            std::thread::spawn(move || {
                let listen = cfg.http.listen.to_string();
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(async move {
                        loop {
                            if cancel_net4mqtt.is_cancelled() {
                                return;
                            }
                            let stream_manager = stream_manager.clone();
                            let (x_sender, x_receiver) =
                                tokio::sync::mpsc::channel::<(String, String, Vec<u8>)>(64);

                            let alias = c.alias.clone();
                            let stream_manager_notify = stream_manager.clone();
                            let x_sender_notify = x_sender.clone();

                            let notify_handle = tokio::spawn(async move {
                                let mut event_recv = stream_manager_notify.subscribe_event();
                                let mut last_sent: Option<Vec<api::response::Stream>> = None;
                                while let Ok(_event) = event_recv.recv().await {
                                    // Debounce: wait a short interval and drain
                                    // additional events so rapid state changes
                                    // produce only one snapshot.
                                    let deadline =
                                        tokio::time::Instant::now() + Duration::from_millis(100);
                                    loop {
                                        tokio::select! {
                                            Ok(_) = event_recv.recv() => {}
                                            _ = tokio::time::sleep_until(deadline) => break,
                                        }
                                    }

                                    let streams = stream_manager_notify.snapshot(&[]).await;
                                    if last_sent.as_ref() == Some(&streams) {
                                        continue;
                                    }
                                    last_sent = Some(streams.clone());
                                    let body = serde_json::json!({ "streams": streams });
                                    if let Ok(data) = serde_json::to_vec(&body)
                                        && let Err(e) = x_sender_notify.try_send((
                                            alias.clone(),
                                            "streams".to_string(),
                                            data,
                                        ))
                                    {
                                        tracing::warn!(
                                            alias,
                                            error = %e,
                                            "net4mqtt xdata channel full or closed"
                                        );
                                    }
                                }
                            });

                            let alias = c.alias.clone();
                            tokio::select! {
                                _ = cancel_net4mqtt.cancelled() => {
                                    notify_handle.abort();
                                    tracing::info!("net4mqtt agent shutting down");
                                    return;
                                }
                                result = net4mqtt::proxy::agent(
                                    &c.mqtt_url,
                                    &listen,
                                    &alias,
                                    Some(net4mqtt::proxy::VDataConfig {
                                        online: Some(
                                            serde_json::json!({ "online": true })
                                                .to_string()
                                                .bytes()
                                                .collect(),
                                        ),
                                        offline: Some("{}".bytes().collect()),
                                        ..Default::default()
                                    }),
                                    Some(net4mqtt::proxy::XDataConfig {
                                        sender: None,
                                        receiver: Some(x_receiver),
                                    }),
                                ) => {
                                    notify_handle.abort();
                                    match result {
                                        Ok(_) => tracing::warn!(
                                            "net4mqtt service is end, restart net4mqtt service"
                                        ),
                                        Err(e) => error!("mqtt4mqtt error: {:?}", e),
                                    }
                                }
                            }
                            tokio::select! {
                                _ = cancel_net4mqtt.cancelled() => {
                                    tracing::info!("net4mqtt agent shutting down");
                                    return;
                                }
                                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                            }
                        }
                    });
            });
        }
    }

    #[cfg(feature = "source")]
    let stream_manager = app_state.stream_manager.clone();

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            signal.await;
            tracing::info!("Shutdown signal received");
            cancel.cancel();

            #[cfg(feature = "source")]
            {
                tracing::info!("Stopping all sources...");
                if let Err(e) = stream_manager.source_manager.stop_all().await {
                    tracing::error!("Failed to stop sources: {}", e);
                }
            }
        })
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
