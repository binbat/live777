use std::net::SocketAddr;
use std::sync::Arc;
use std::{collections::HashMap, str::FromStr};

use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Router,
};
use config::IceServer;
use log::info;
use tokio::sync::RwLock;
use tower_http::services::{ServeDir, ServeFile};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::config::Config;
use crate::forward::PeerForward;

mod config;
mod forward;
mod media;

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .filter_module("webrtc", log::LevelFilter::Error)
        .write_style(env_logger::WriteStyle::Auto)
        .target(env_logger::Target::Stdout)
        .init();
    let cfg = Config::parse();
    let addr = SocketAddr::from_str(&cfg.listen).expect("invalid listen address");
    info!("Server listening on {}", addr);
    let app_state = AppState {
        forwards: Arc::new(RwLock::new(HashMap::new())),
        config: cfg,
    };
    let serve_dir = ServeDir::new("assets").not_found_service(ServeFile::new("assets/index.html"));
    let app = Router::new()
        .route(
            "/whip/:id",
            post(whip)
                .patch(add_ice_candidate)
                .options(ice_server_config),
        )
        .route(
            "/whep/:id",
            post(whep)
                .patch(add_ice_candidate)
                .options(ice_server_config),
        )
        .nest_service("/", serve_dir.clone())
        .with_state(app_state);
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

#[derive(Clone)]
struct AppState {
    forwards: Arc<RwLock<HashMap<String, PeerForward>>>,
    config: Config,
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
    if content_type.as_bytes() != b"application/sdp" {
        return Err(anyhow::anyhow!("Content-Type must be application/sdp").into());
    }
    let offer = RTCSessionDescription::offer(body)?;
    let map = state.forwards.read().await;
    let original_forward = map.get(&id);
    let is_none = original_forward.is_none();
    let forward = if is_none {
        PeerForward::new(
            id.clone(),
            state
                .config
                .ice_servers
                .into_iter()
                .map(|ice| ice.into())
                .collect(),
        )
    } else {
        original_forward.unwrap().clone()
    };
    drop(map);
    let (answer, key) = forward.set_anchor(offer).await?;
    if is_none {
        let mut map = state.forwards.write().await;
        if map.contains_key(&id) {
            return Err(anyhow::anyhow!("resource already exists").into());
        }
        map.insert(forward.get_id(), forward);
    }
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
    let map = state.forwards.read().await;
    let forward = map.get(&id);
    if forward.is_none() {
        return Err(anyhow::anyhow!("resource not found").into());
    }
    let forward = forward.unwrap().clone();
    drop(map);
    let (answer, key) = forward.add_subscribe(offer).await?;
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
    uri: Uri,
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
    let map = state.forwards.read().await;
    let forward = map.get(&id);
    if forward.is_none() {
        return Err(anyhow::anyhow!("resource not found").into());
    }
    let forward = forward.unwrap().clone();
    drop(map);
    forward
        .add_ice_candidate(key, body, uri.to_string().contains("whip"))
        .await?;
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
            if username != "" {
                username = string_encoder(&username);
                credential = string_encoder(&credential);
            }
            server.urls.into_iter().map(move |url| {
                let mut link = format!("<{}>; rel=\"ice-server\"", url.replacen(":", "://", 1));
                if username != "" {
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

struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, self.0.to_string()).into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
