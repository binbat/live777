use axum::body::{Body, Bytes};
use axum::extract::Request;
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
use forward::info::Layer;
use http::header::ToStrError;
use http::Uri;
use http_body_util::BodyExt;
use std::collections::HashMap;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use thiserror::Error;
#[cfg(debug_assertions)]
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use tracing::info_span;
use tracing::{debug, error, info};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::auth::ManyValidate;
use crate::config::Config;
use crate::dto::req::{ChangeResource, SelectLayer};
use config::IceServer;
use path::manager::Manager;
#[cfg(not(debug_assertions))]
use {http::header, rust_embed::RustEmbed};

mod auth;
mod config;
mod constant;
mod dto;
mod forward;
mod metrics;
mod path;
mod signal;

#[tokio::main]
async fn main() {
    metrics::REGISTRY
        .register(Box::new(metrics::PUBLISH.clone()))
        .unwrap();
    metrics::REGISTRY
        .register(Box::new(metrics::SUBSCRIBE.clone()))
        .unwrap();
    let cfg = Config::parse();
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("live777={},webrtc=error", cfg.log.level).into()),
        )
        .with(tracing_logfmt::layer())
        .init();
    let addr = SocketAddr::from_str(&cfg.listen).expect("invalid listen address");
    info!("Server listening on {}", addr);
    let ice_servers = cfg
        .ice_servers
        .clone()
        .into_iter()
        .map(|i| i.into())
        .collect();
    let app_state = AppState {
        paths: Arc::new(Manager::new(ice_servers)),
        config: cfg.clone(),
    };
    let auth_layer = ValidateRequestHeaderLayer::custom(ManyValidate::new(cfg.auth));
    let app = Router::new()
        .route("/whip/:id", post(whip))
        .route("/whep/:id", post(whep))
        .route(
            "/resource/:id/:key",
            post(change_resource)
                .patch(add_ice_candidate)
                .delete(remove_path_key),
        )
        .route(
            "/resource/:id/:key/layer",
            get(get_layer).post(select_layer).delete(un_select_layer),
        )
        .layer(auth_layer)
        .route("/metrics", get(metrics))
        .with_state(app_state)
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
        Err(e) = axum::serve(tokio::net::TcpListener::bind(&addr).await.unwrap(), static_server(app)).into_future() => error!("Application error: {e}"),
        msg = signal::wait_for_stop_signal() => debug!("Received signal: {}", msg),
    }
    info!("Server shutdown");
}

async fn metrics() -> String {
    metrics::ENCODER
        .encode_to_string(&metrics::REGISTRY.gather())
        .unwrap()
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
    paths: Arc<Manager>,
}

async fn print_request_response(
    req: Request,
    next: Next,
) -> Result<impl IntoResponse, (StatusCode, String)> {
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
) -> Result<Bytes, (StatusCode, String)>
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
        tracing::debug!("{direction} headers = {headers:?} body = {body:?}");
    }

    Ok(bytes)
}

async fn whip(
    State(state): State<AppState>,
    Path(id): Path<String>,
    header: HeaderMap,
    body: String,
) -> AppResult<Response<String>> {
    let content_type = header
        .get("Content-Type")
        .ok_or(anyhow::anyhow!("Content-Type is required"))?;
    if content_type.to_str()? != "application/sdp" {
        return Err(anyhow::anyhow!("Content-Type must be application/sdp").into());
    }
    let offer = RTCSessionDescription::offer(body)?;
    let (answer, key) = state.paths.publish(id.clone(), offer).await?;
    let mut builder = Response::builder()
        .status(StatusCode::CREATED)
        .header("Content-Type", "application/sdp")
        .header("Accept-Patch", "application/trickle-ice-sdpfrag")
        .header("Location", format!("/resource/{}/{}", id, key));
    for link in link_header(state.config.ice_servers.clone()) {
        builder = builder.header("Link", link);
    }
    Ok(builder.body(answer.sdp)?)
}

async fn whep(
    State(state): State<AppState>,
    Path(id): Path<String>,
    header: HeaderMap,
    body: String,
) -> AppResult<Response<String>> {
    let content_type = header
        .get("Content-Type")
        .ok_or(anyhow::anyhow!("Content-Type is required"))?;
    if content_type.to_str()? != "application/sdp" {
        return Err(anyhow::anyhow!("Content-Type must be application/sdp").into());
    }
    let offer = RTCSessionDescription::offer(body)?;
    let (answer, key) = state.paths.subscribe(id.clone(), offer).await?;
    let mut builder = Response::builder()
        .status(StatusCode::CREATED)
        .header("Content-Type", "application/sdp")
        .header("Accept-Patch", "application/trickle-ice-sdpfrag")
        .header("Location", format!("/resource/{}/{}", id, key));
    for link in link_header(state.config.ice_servers.clone()) {
        builder = builder.header("Link", link);
    }
    if state.paths.layers(id.clone()).await.is_ok() {
        builder = builder.header(
            "Link",
            format!(
                "</resource/{}/{}/layer>; rel=\"urn:ietf:params:whep:ext:core:layer\"",
                id, key
            ),
        )
    }
    Ok(builder.body(answer.sdp)?)
}

async fn add_ice_candidate(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
    header: HeaderMap,
    body: String,
) -> AppResult<Response<String>> {
    let content_type = header
        .get("Content-Type")
        .ok_or(AppError::from(anyhow::anyhow!("Content-Type is required")))?;
    if content_type.to_str()? != "application/trickle-ice-sdpfrag" {
        return Err(anyhow::anyhow!("Content-Type must be application/trickle-ice-sdpfrag").into());
    }
    state.paths.add_ice_candidate(id, key, body).await?;
    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body("".to_string())?)
}

async fn remove_path_key(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
    _uri: Uri,
) -> AppResult<Response<String>> {
    state.paths.remove_path_key(id, key).await?;
    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body("".to_string())?)
}

async fn change_resource(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
    Json(dto): Json<ChangeResource>,
) -> AppResult<Json<HashMap<String, String>>> {
    state.paths.change_resource(id, key, dto).await?;
    Ok(Json(HashMap::new()))
}

async fn get_layer(
    State(state): State<AppState>,
    Path((id, _key)): Path<(String, String)>,
) -> AppResult<Json<Vec<Layer>>> {
    let layers = state.paths.layers(id).await?;
    Ok(Json(layers))
}

async fn select_layer(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
    Json(layer): Json<SelectLayer>,
) -> AppResult<String> {
    state
        .paths
        .select_layer(
            id,
            key,
            layer.encoding_id.map(|encoding_id| Layer { encoding_id }),
        )
        .await?;
    Ok("".to_string())
}

async fn un_select_layer(
    State(state): State<AppState>,
    Path((id, key)): Path<(String, String)>,
) -> AppResult<String> {
    state
        .paths
        .select_layer(
            id,
            key,
            Some(Layer {
                encoding_id: constant::RID_DISABLE.to_string(),
            }),
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

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("resource not found:{0}")]
    ResourceNotFound(String),
    #[error("resource already exists:{0}")]
    ResourceAlreadyExists(String),
    #[error("internal server error")]
    InternalServerError(anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::ResourceNotFound(err) => {
                (StatusCode::NOT_FOUND, err.to_string()).into_response()
            }
            AppError::InternalServerError(err) => {
                (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
            }
            AppError::ResourceAlreadyExists(err) => {
                (StatusCode::CONFLICT, err.to_string()).into_response()
            }
        }
    }
}

impl From<http::Error> for AppError {
    fn from(err: http::Error) -> Self {
        AppError::InternalServerError(err.into())
    }
}

impl From<ToStrError> for AppError {
    fn from(err: ToStrError) -> Self {
        AppError::InternalServerError(err.into())
    }
}

impl From<webrtc::Error> for AppError {
    fn from(err: webrtc::Error) -> Self {
        AppError::InternalServerError(err.into())
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::InternalServerError(err)
    }
}
