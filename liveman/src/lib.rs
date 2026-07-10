use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use auth::{AuthState, access::access_middleware, validate_middleware};
use axum::{Router, extract::Request, middleware, response::IntoResponse, routing::post};
use http::{StatusCode, Uri};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::{error, info, info_span};

use crate::admin::{authorize, token};
use crate::config::Config;
use crate::service::database::DatabaseService;
use crate::store::{Node, NodeKind, Storage};

#[cfg(feature = "webui")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../assets/liveman/"]
struct Assets;

mod admin;
pub mod config;
pub mod entity;
mod error;
pub mod migration;
mod result;
mod route;
pub mod service;
mod sse;
mod store;
mod tick;
mod utils;

pub async fn serve<F>(cfg: Config, listener: TcpListener, signal: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    info!("Server listening on {}", listener.local_addr().unwrap());

    // Initialize database connection (recordings index)
    let database_service = DatabaseService::new(&cfg.database)
        .await
        .expect("Failed to initialize database connection");

    // Initialize file storage operator if recorder feature is enabled
    #[cfg(feature = "recorder")]
    let file_storage = if cfg!(feature = "recorder") {
        match storage::init_operator(&cfg.recorder.storage).await {
            Ok(operator) => {
                info!("File storage initialized successfully");
                Some(operator)
            }
            Err(e) => {
                error!(
                    "Failed to initialize file storage: {}, continuing without file storage",
                    e
                );
                None
            }
        }
    } else {
        None
    };

    let client_req = reqwest::Client::builder();
    let client_mem = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(500))
        .timeout(Duration::from_millis(1000));

    #[cfg(feature = "net4mqtt")]
    let (client_req, client_mem) = if let Some(proxy) = match cfg.net4mqtt.clone() {
        Some(c) => {
            // References: https://github.com/seanmonstar/reqwest/issues/899
            let target = reqwest::Url::parse(&format!("socks5h://{}", c.listen)).unwrap();
            Some(reqwest::Proxy::custom(move |url| match url.host_str() {
                Some(host) => {
                    if host.ends_with(c.domain.as_str()) {
                        Some(target.clone())
                    } else {
                        None
                    }
                }
                None => None,
            }))
        }
        None => None,
    } {
        info!("net4mqtt proxy: {:?}", proxy);
        (client_req.proxy(proxy.clone()), client_mem.proxy(proxy))
    } else {
        (client_req, client_mem)
    };

    let store = Storage::new(client_mem.build().unwrap());
    let cancel = CancellationToken::new();
    let nodes = store.get_map_nodes_mut();
    for v in cfg.nodes.clone() {
        nodes.write().unwrap().insert(
            v.alias.clone(),
            Node::new(v.token.clone(), NodeKind::Static, v.url.clone()),
        );

        tokio::spawn(crate::sse::subscribe_streams(
            v.url,
            v.token,
            v.alias,
            store.clone(),
            cancel.clone(),
        ));
    }

    #[cfg(feature = "net4mqtt")]
    {
        if let Some(mut c) = cfg.net4mqtt.clone() {
            c.validate();
            let (sender, mut receiver) =
                tokio::sync::mpsc::channel::<(String, String, Vec<u8>)>(10);
            let (x_sender, mut x_receiver) =
                tokio::sync::mpsc::unbounded_channel::<(String, String, Vec<u8>)>();

            let domain = c.domain.clone();

            std::thread::spawn(move || {
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(async move {
                        loop {
                            let listener = TcpListener::bind(c.listen).await.unwrap();
                            match net4mqtt::proxy::local_socks(
                                &c.mqtt_url,
                                listener,
                                ("-", &c.alias.clone()),
                                Some(c.domain.clone()),
                                Some(net4mqtt::proxy::VDataConfig {
                                    receiver: Some(sender.clone()),
                                    ..Default::default()
                                }),
                                Some(net4mqtt::proxy::XDataConfig {
                                    sender: Some(x_sender.clone()),
                                    receiver: None,
                                }),
                                false,
                            )
                            .await
                            {
                                Ok(_) => tracing::warn!(
                                    "net4mqtt service is end, restart net4mqtt service"
                                ),
                                Err(e) => error!("mqtt4mqtt error: {:?}", e),
                            }
                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        }
                    });
            });

            std::thread::spawn(move || {
                let dns = net4mqtt::kxdns::Kxdns::new(domain);
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(async move {
                        loop {
                            match receiver.recv().await {
                                Some((agent_id, _local_id, data)) => {
                                    if data.len() > 5 {
                                        nodes.write().unwrap().insert(
                                            agent_id.clone(),
                                            Node::new(
                                                "".to_string(),
                                                NodeKind::Net4mqtt,
                                                format!("http://{}", dns.registry(&agent_id)),
                                            ),
                                        );
                                    } else {
                                        nodes.write().unwrap().remove(&agent_id);
                                    }
                                }
                                None => {
                                    error!("net4mqtt discovery receiver channel closed");
                                    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                                }
                            }
                        }
                    })
            });

            let store_xdata = store.clone();
            tokio::spawn(async move {
                while let Some((alias, key, data)) = x_receiver.recv().await {
                    if key.as_str() != "streams" {
                        continue;
                    }
                    match serde_json::from_slice::<serde_json::Value>(&data) {
                        Ok(value) => {
                            let streams = value
                                .get("streams")
                                .and_then(|v| {
                                    serde_json::from_value::<Vec<api::response::Stream>>(v.clone())
                                        .ok()
                                })
                                .unwrap_or_default();
                            if let Err(e) =
                                crate::sse::update_storage(&store_xdata, &alias, streams).await
                            {
                                tracing::warn!(
                                    alias,
                                    error = ?e,
                                    "failed to update storage from net4mqtt"
                                );
                            }
                        }
                        Err(err) => {
                            tracing::warn!(?err, "failed to decode net4mqtt streams")
                        }
                    }
                }
            });
        }
    }

    let app_state = AppState {
        config: cfg.clone(),
        client: client_req.build().unwrap(),
        storage: store,
        database: database_service,
        record_sync_cursor: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        #[cfg(feature = "recorder")]
        file_storage,
    };

    let app = Router::new()
        .merge(
            route::proxy::route()
                .route("/api/token", post(token))
                .layer(middleware::from_fn(access_middleware))
                .layer(middleware::from_fn_with_state(
                    AuthState::new(cfg.auth.secret, cfg.auth.tokens),
                    validate_middleware,
                )),
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
                span.record(
                    "span_id",
                    span.id().unwrap_or(tracing::Id::from_u64(42)).into_u64(),
                );
                span
            }),
        )
        .fallback(static_handler);

    tokio::spawn(tick::cascade_check(app_state.clone()));

    tokio::spawn(tick::auto_record_check(app_state.clone()));

    tokio::spawn(tick::auto_record_rotate(app_state.clone()));

    tokio::spawn(tick::record_sync(app_state.clone()));

    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            signal.await;
            cancel.cancel();
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

#[derive(Clone)]
struct AppState {
    config: Config,
    client: reqwest::Client,
    storage: Storage,
    database: DatabaseService,
    record_sync_cursor: Arc<tokio::sync::RwLock<HashMap<String, i64>>>,
    #[cfg(feature = "recorder")]
    file_storage: Option<opendal::Operator>,
}
