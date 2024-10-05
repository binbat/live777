use axum::body::Body;
use axum::extract::Request;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::Router;

use http::{header, StatusCode, Uri};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use rust_embed::RustEmbed;
use std::future::Future;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::{error, info_span};

#[cfg(feature = "liveion")]
use tracing::info;

use auth::{access::access_middleware, ManyValidate};

use crate::admin::{authorize, token};
use crate::config::Config;
use crate::mem::{MemStorage, Node, NodeKind, Server};

#[derive(RustEmbed)]
#[folder = "../assets/liveman/"]
struct Assets;

mod admin;
pub mod config;
mod error;
mod mem;
mod result;
mod route;
mod tick;

#[cfg(feature = "liveion")]
mod cluster;

pub async fn server_up<F>(cfg: Config, listener: TcpListener, signal: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    let client: Client =
        hyper_util::client::legacy::Client::<(), ()>::builder(TokioExecutor::new())
            .build(HttpConnector::new());

    #[cfg(feature = "net4mqtt")]
    let net4mqtt_domain = "net4mqtt.local";

    #[cfg(feature = "net4mqtt")]
    let proxy_addr = match cfg.net4mqtt.clone() {
        Some(c) => Some((c.listen, net4mqtt_domain.to_string())),
        None => None,
    };
    #[cfg(not(feature = "net4mqtt"))]
    let proxy_addr = None;

    let store = MemStorage::new(proxy_addr);
    let nodes = store.get_map_nodes_mut();
    for v in cfg.nodes.clone() {
        nodes
            .write()
            .unwrap()
            .insert(v.alias, Node::new(v.token, NodeKind::BuildIn, v.url));
    }

    #[cfg(feature = "liveion")]
    {
        let servers = cluster::cluster_up(cfg.liveion.clone()).await;
        info!("liveion buildin servers: {:?}", servers);
        for v in servers {
            nodes
                .write()
                .unwrap()
                .insert(v.alias, Node::new(v.token, NodeKind::BuildIn, v.url));
        }
    }

    #[cfg(feature = "net4mqtt")]
    {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<(String, String, Vec<u8>)>(10);

        if let Some(c) = cfg.net4mqtt.clone() {
            std::thread::spawn(move || {
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(async move {
                        let listener = TcpListener::bind(c.listen).await.unwrap();
                        net4mqtt::proxy::local_socks(
                            &c.mqtt_url,
                            listener,
                            "-",
                            &c.alias.clone(),
                            None,
                            Some(sender),
                            false,
                        )
                        .await
                        .unwrap()
                    });
            });
        }

        std::thread::spawn(move || {
            let dns = net4mqtt::kxdns::Kxdns::new(net4mqtt_domain);
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
    }

    let app_state = AppState {
        config: cfg.clone(),
        client,
        storage: store,
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

type Client = hyper_util::client::legacy::Client<HttpConnector, Body>;

#[derive(Clone)]
struct AppState {
    config: Config,
    client: Client,
    storage: MemStorage,
}
