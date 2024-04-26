use axum::body::{Body, Bytes};
use axum::extract::{Query, Request};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::routing::get;
use axum::Json;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use clap::Parser;

use error::AppError;
use forward::info::ReforwardInfo;
use http::Uri;
use http_body_util::BodyExt;
use local_ip_address::local_ip;
use std::collections::HashMap;
use std::env;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[cfg(debug_assertions)]
use tower_http::services::{ServeDir, ServeFile};

use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::info_span;
use tracing::{debug, error, info};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::auth::ManyValidate;
use crate::config::Config;
use crate::result::Result;
use crate::stream::config::ManagerConfig;
use config::IceServer;
use live777_http::response::Metrics;
use stream::manager::Manager;
#[cfg(not(debug_assertions))]
use {http::header, rust_embed::RustEmbed};

mod auth;
mod config;
mod constant;
mod convert;
mod error;
mod forward;
mod metrics;
mod result;
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
    set_log(format!("live777={},webrtc=error", cfg.log.level));
    debug!("config : {:?}", cfg);
    let listener = tokio::net::TcpListener::bind(&cfg.http.listen)
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    info!("Server listening on {}", addr);
    if cfg.node_info.ip_port.is_none() {
        let port = addr.port();
        cfg.node_info.ip_port =
            Some(SocketAddr::from_str(&format!("{}:{}", local_ip().unwrap(), port)).unwrap());
        debug!("config : {:?}", cfg);
    }
    let app_state = AppState {
        stream_manager: Arc::new(Manager::new(ManagerConfig::from_config(cfg.clone()).await).await),
        config: cfg.clone(),
    };
    let auth_layer = ValidateRequestHeaderLayer::custom(ManyValidate::new(vec![
        cfg.auth,
        cfg.admin_auth.clone(),
    ]));
    let admin_auth_layer =
        ValidateRequestHeaderLayer::custom(ManyValidate::new(vec![cfg.admin_auth]));
    let app = Router::new()
        .route(&live777_http::path::whip(":stream"), post(whip))
        .route(&live777_http::path::whep(":stream"), post(whep))
        .route(
            &live777_http::path::resource(":stream", ":session"),
            post(change_resource)
                .patch(add_ice_candidate)
                .delete(remove_stream_session),
        )
        .route(
            &live777_http::path::resource_layer(":stream", ":session"),
            get(get_layer).post(select_layer).delete(un_select_layer),
        )
        .layer(auth_layer)
        .route(
            live777_http::path::ADMIN_INFOS,
            get(infos).layer(admin_auth_layer.clone()),
        )
        .route(
            &live777_http::path::reforward(":stream"),
            post(reforward).layer(admin_auth_layer),
        )
        .route(live777_http::path::METRICS, get(metrics))
        .route(live777_http::path::METRICS_JSON, get(metrics_json))
        .with_state(app_state)
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
    tokio::select! {
        Err(e) = axum::serve(listener, static_server(app)).into_future() => error!("Application error: {e}"),
        msg = signal::wait_for_stop_signal() => debug!("Received signal: {}", msg),
    }
    info!("Server shutdown");
}

fn set_log(env_filter: String) {
    let _ = env::var("RUST_LOG").is_err_and(|_| {
        env::set_var("RUST_LOG", env_filter);
        true
    });
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .compact()
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .with_target(true)
        .init();
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

async fn metrics_json() -> Json<Metrics> {
    Json::from(Metrics {
        stream: metrics::STREAM.get() as u64,
        publish: metrics::PUBLISH.get() as u64,
        subscribe: metrics::SUBSCRIBE.get() as u64,
        reforward: metrics::REFORWARD.get() as u64,
    })
}

#[cfg(not(debug_assertions))]
#[derive(RustEmbed)]
#[folder = "assets/"]
struct Assets;

fn static_server(router: Router) -> Router {
    #[cfg(debug_assertions)]
    {
        let serve_dir =
            ServeDir::new("assets").not_found_service(ServeFile::new("assets/index.html"));
        router.nest_service("/", serve_dir.clone())
    }
    #[cfg(not(debug_assertions))]
    {
        router.fallback(static_handler)
    }
}

#[cfg(not(debug_assertions))]
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
    stream_manager: Arc<Manager>,
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

async fn whip(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    header: HeaderMap,
    body: String,
) -> Result<Response<String>> {
    let content_type = header
        .get("Content-Type")
        .ok_or(anyhow::anyhow!("Content-Type is required"))?;
    if content_type.to_str()? != "application/sdp" {
        return Err(anyhow::anyhow!("Content-Type must be application/sdp").into());
    }
    let offer = RTCSessionDescription::offer(body)?;
    let (answer, session) = state.stream_manager.publish(stream.clone(), offer).await?;
    let mut builder = Response::builder()
        .status(StatusCode::CREATED)
        .header("Content-Type", "application/sdp")
        .header("Accept-Patch", "application/trickle-ice-sdpfrag")
        .header("Location", live777_http::path::resource(&stream, &session));
    for link in link_header(state.config.ice_servers.clone()) {
        builder = builder.header("Link", link);
    }
    Ok(builder.body(answer.sdp)?)
}

async fn whep(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    header: HeaderMap,
    body: String,
) -> Result<Response<String>> {
    let content_type = header
        .get("Content-Type")
        .ok_or(anyhow::anyhow!("Content-Type is required"))?;
    if content_type.to_str()? != "application/sdp" {
        return Err(anyhow::anyhow!("Content-Type must be application/sdp").into());
    }
    let offer = RTCSessionDescription::offer(body)?;
    let (answer, session) = state
        .stream_manager
        .subscribe(stream.clone(), offer)
        .await?;
    let mut builder = Response::builder()
        .status(StatusCode::CREATED)
        .header("Content-Type", "application/sdp")
        .header("Accept-Patch", "application/trickle-ice-sdpfrag")
        .header("Location", live777_http::path::resource(&stream, &session));
    for link in link_header(state.config.ice_servers.clone()) {
        builder = builder.header("Link", link);
    }
    if state.stream_manager.layers(stream.clone()).await.is_ok() {
        builder = builder.header(
            "Link",
            format!(
                "<{}>; rel=\"urn:ietf:params:whep:ext:core:layer\"",
                live777_http::path::resource_layer(&stream, &session)
            ),
        )
    }
    Ok(builder.body(answer.sdp)?)
}

async fn add_ice_candidate(
    State(state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    header: HeaderMap,
    body: String,
) -> Result<Response<String>> {
    let content_type = header
        .get("Content-Type")
        .ok_or(AppError::from(anyhow::anyhow!("Content-Type is required")))?;
    if content_type.to_str()? != "application/trickle-ice-sdpfrag" {
        return Err(anyhow::anyhow!("Content-Type must be application/trickle-ice-sdpfrag").into());
    }
    state
        .stream_manager
        .add_ice_candidate(stream, session, body)
        .await?;
    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body("".to_string())?)
}

async fn remove_stream_session(
    State(state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    _uri: Uri,
) -> Result<Response<String>> {
    state
        .stream_manager
        .remove_stream_session(stream, session)
        .await?;
    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body("".to_string())?)
}

async fn change_resource(
    State(state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    Json(req): Json<live777_http::request::ChangeResource>,
) -> Result<Json<HashMap<String, String>>> {
    state
        .stream_manager
        .change_resource(stream, session, (req.kind, req.enabled))
        .await?;
    Ok(Json(HashMap::new()))
}

async fn get_layer(
    State(state): State<AppState>,
    Path((stream, _session)): Path<(String, String)>,
) -> Result<Json<Vec<live777_http::response::Layer>>> {
    Ok(Json(
        state
            .stream_manager
            .layers(stream)
            .await?
            .into_iter()
            .map(|layer| layer.into())
            .collect(),
    ))
}

async fn select_layer(
    State(state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
    Json(req): Json<live777_http::request::SelectLayer>,
) -> Result<String> {
    state
        .stream_manager
        .select_layer(
            stream,
            session,
            req.encoding_id
                .map(|encoding_id| forward::info::Layer { encoding_id }),
        )
        .await?;
    Ok("".to_string())
}

async fn un_select_layer(
    State(state): State<AppState>,
    Path((stream, session)): Path<(String, String)>,
) -> Result<String> {
    state
        .stream_manager
        .select_layer(
            stream,
            session,
            Some(forward::info::Layer {
                encoding_id: constant::RID_DISABLE.to_string(),
            }),
        )
        .await?;
    Ok("".to_string())
}

async fn infos(
    State(state): State<AppState>,
    Query(req): Query<live777_http::request::QueryInfo>,
) -> Result<Json<Vec<live777_http::response::StreamInfo>>> {
    Ok(Json(
        state
            .stream_manager
            .info(req.streams.map_or(vec![], |streams| {
                streams
                    .split(',')
                    .map(|stream| stream.to_string())
                    .collect()
            }))
            .await
            .into_iter()
            .map(|forward_info| forward_info.into())
            .collect(),
    ))
}

async fn reforward(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    Json(req): Json<live777_http::request::Reforward>,
) -> Result<String> {
    state
        .stream_manager
        .reforward(
            stream,
            ReforwardInfo {
                target_url: req.target_url,
                admin_authorization: req.admin_authorization,
                resource_url: None,
            },
        )
        .await?;
    Ok("".to_string())
}

fn link_header(ice_servers: Vec<IceServer>) -> Vec<String> {
    ice_servers
        .into_iter()
        .flat_map(|server| {
            let mut username = server.username;
            let mut credential = server.credential;
            if !username.is_empty() {
                username = string_encoder(&username);
                credential = string_encoder(&credential);
            }
            server.urls.into_iter().map(move |url| {
                let mut link = format!("<{}>; rel=\"ice-server\"", url);
                if !username.is_empty() {
                    link = format!(
                        "{}; username=\"{}\"; credential=\"{}\"; credential-type=\"{}\"",
                        link, username, credential, server.credential_type
                    );
                }
                link
            })
        })
        .collect()
}

fn string_encoder(s: &impl ToString) -> String {
    let s = serde_json::to_string(&s.to_string()).unwrap();
    s[1..s.len() - 1].to_string()
}
