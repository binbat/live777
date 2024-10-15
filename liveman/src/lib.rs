use std::{future::Future, time::Duration};

use axum::{extract::Request, middleware, response::IntoResponse, routing::post, Router};
use http::{header, StatusCode, Uri};
use rust_embed::RustEmbed;
use tokio::net::TcpListener;
use tower_http::{
    cors::CorsLayer, trace::TraceLayer, validate_request::ValidateRequestHeaderLayer,
};
use tracing::{error, info, info_span};

use crate::admin::{authorize, token};
use crate::config::Config;
use crate::mem::{MemStorage, Node, NodeKind, Server};
use auth::{access::access_middleware, ManyValidate};

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

pub async fn serve<F>(cfg: Config, listener: TcpListener, signal: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    info!("Server listening on {}", listener.local_addr().unwrap());
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

    let store = MemStorage::new(client_mem.build().unwrap());
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
        if let Some(c) = cfg.net4mqtt.clone() {
            let (sender, mut receiver) =
                tokio::sync::mpsc::channel::<(String, String, Vec<u8>)>(10);

            let domain = c.domain.clone();

            std::thread::spawn(move || {
                tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(async move {
                        let listener = TcpListener::bind(c.listen).await.unwrap();
                        net4mqtt::proxy::local_socks(
                            &c.mqtt_url,
                            listener,
                            ("-", &c.alias.clone()),
                            Some(c.domain),
                            Some(net4mqtt::proxy::VDataConfig {
                                receiver: Some(sender),
                                ..Default::default()
                            }),
                            false,
                        )
                        .await
                        .unwrap()
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
        }
    }

    let app_state = AppState {
        config: cfg.clone(),
        client: client_req.build().unwrap(),
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
                span.record(
                    "span_id",
                    span.id().unwrap_or(tracing::Id::from_u64(42)).into_u64(),
                );
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

#[derive(Clone)]
struct AppState {
    config: Config,
    client: reqwest::Client,
    storage: MemStorage,
}
