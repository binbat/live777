use std::env;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use axum::http::{HeaderMap, Uri};
use axum::routing::get;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use log::info;
#[cfg(debug_assertions)]
use tower_http::services::{ServeDir, ServeFile};
use tower_http::validate_request::ValidateRequestHeaderLayer;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use config::IceServer;
use path::manager::Manager;
#[cfg(not(debug_assertions))]
use {http::header, rust_embed::RustEmbed};

use crate::auth::ManyValidate;
use crate::config::Config;

mod auth;
mod config;
mod forward;
mod media;
mod metrics;
mod path;

#[tokio::main]
async fn main() {
    metrics::REGISTRY
        .register(Box::new(metrics::PUBLISH.clone()))
        .unwrap();
    metrics::REGISTRY
        .register(Box::new(metrics::SUBSCRIBE.clone()))
        .unwrap();
    let log_level = if cfg!(debug_assertions) {
        env::var("LOG_LEVEL").unwrap_or_else(|_| "debug".to_string())
    } else {
        env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string())
    };
    let level_filter = match log_level.as_str() {
        "off" => log::LevelFilter::Off,
        "error" => log::LevelFilter::Error,
        "warn" => log::LevelFilter::Warn,
        "info" => log::LevelFilter::Info,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    };
    env_logger::builder()
        .filter_level(level_filter)
        .filter_module("webrtc", log::LevelFilter::Error)
        .write_style(env_logger::WriteStyle::Auto)
        .target(env_logger::Target::Stdout)
        .init();
    let cfg = Config::parse();
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
    let mut app = Router::new()
        .route(
            "/whip/:id",
            post(whip)
                .patch(add_ice_candidate)
                .delete(remove_path_key)
                .layer(auth_layer.clone())
                .options(ice_server_config),
        )
        .route(
            "/whep/:id",
            post(whep)
                .patch(add_ice_candidate)
                .delete(remove_path_key)
                .layer(auth_layer)
                .options(ice_server_config),
        )
        .route("/metrics", get(metrics))
        .with_state(app_state);
    app = static_server(app);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
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

async fn whip(
    State(state): State<AppState>,
    Path(id): Path<String>,
    header: HeaderMap,
    uri: Uri,
    body: String,
) -> Result<Response<String>, AppError> {
    let content_type = header
        .get("Content-Type")
        .ok_or(anyhow::anyhow!("Content-Type is required"))?;
    if content_type.to_str()? != "application/sdp" {
        return Err(anyhow::anyhow!("Content-Type must be application/sdp").into());
    }
    let offer = RTCSessionDescription::offer(body)?;
    let (answer, key) = state.paths.publish(id, offer).await.map_err(|err|{
        AppError::not_found(anyhow::anyhow!(err))})?;
    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header("Content-Type", "application/sdp")
        .header("Accept-Patch", "application/trickle-ice-sdpfrag")
        .header("E-Tag", key)
        .header("Location", uri.to_string())
        .body(answer.sdp)?)
}

async fn whep(
    State(state): State<AppState>,
    Path(id): Path<String>,
    header: HeaderMap,
    uri: Uri,
    body: String,
) -> Result<Response<String>, AppError> {
    let content_type = header
        .get("Content-Type")
        .ok_or(anyhow::anyhow!("Content-Type is required"))?;
    if content_type.to_str()? != "application/sdp" {
        return Err(anyhow::anyhow!("Content-Type must be application/sdp").into());
    }
    let offer = RTCSessionDescription::offer(body)?;
    let (answer, key) = state.paths.subscribe(id, offer).await.map_err(|err|{
        AppError::not_found(anyhow::anyhow!(err))})?;
    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header("Content-Type", "application/sdp")
        .header("Accept-Patch", "application/trickle-ice-sdpfrag")
        .header("E-Tag", key)
        .header("Location", uri.to_string())
        .body(answer.sdp)?)
}

async fn add_ice_candidate(
    State(state): State<AppState>,
    Path(id): Path<String>,
    header: HeaderMap,
    body: String,
) -> Result<Response<String>, AppError> {
    let content_type = header
        .get("Content-Type")
        .ok_or(AppError::from(anyhow::anyhow!("Content-Type is required")))?;
    if content_type.to_str()? != "application/trickle-ice-sdpfrag" {
        return Err(anyhow::anyhow!("Content-Type must be application/trickle-ice-sdpfrag").into());
    }
    let key = header
        .get("If-Match")
        .ok_or(AppError::from(anyhow::anyhow!("If-Match is required")))?
        .to_str()?
        .to_string();
    state.paths.add_ice_candidate(id, key, body).await?;
    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body("".to_string())?)
}

async fn remove_path_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
    header: HeaderMap,
) -> Result<Response<String>, AppError> {
    let key = header
        .get("If-Match")
        .ok_or(AppError::from(anyhow::anyhow!("If-Match is required")))?
        .to_str()?
        .to_string();
    state.paths.remove_path_key(id, key).await?;
    Ok(Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body("".to_string())?)
}

async fn ice_server_config(State(state): State<AppState>) -> Result<Response<String>, AppError> {
    let mut builder = Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header("Access-Control-Allow-Methods", "OPTIONS, GET, POST, PATCH")
        .header(
            "Access-Control-Allow-Headers",
            "Authorization, Content-Type, If-Match",
        );
    for link in link_header(state.config.ice_servers.clone()) {
        builder = builder.header("Link", link);
    }
    Ok(builder.body("".to_owned())?)
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
                let mut link = format!("<{}>; rel=\"ice-server\"", url.replacen(':', "://", 1));
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

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            error: err.into(),
        }
    }
}

impl AppError {
   pub fn not_found<E>(err: E) -> Self
    where
        E: Into<anyhow::Error>,
    {
        Self {
            status_code: StatusCode::NOT_FOUND,
            error: err.into(),
        }
    }
  
}
