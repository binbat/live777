use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Router,
};
use tokio::sync::RwLock;
use tower_http::services::{ServeDir, ServeFile};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::forward::PeerForward;

mod forward;

#[tokio::main]
async fn main() {
    let shared_state = SharedState::default();
    let serve_dir = ServeDir::new("assets").not_found_service(ServeFile::new("assets/index.html"));
    let app = Router::new()
        .route("/whip/:id", post(whip))
        .route("/whep/endpoint/:id", post(whep))
        .nest_service("/", serve_dir.clone())
        .with_state(Arc::clone(&shared_state));
    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

type SharedState = Arc<RwLock<HashMap<String, PeerForward>>>;

async fn whip(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    header: HeaderMap,
    uri: Uri,
    body: String,
) -> Result<Response<String>, AppError> {
    let offer = RTCSessionDescription::offer(body)?;
    let map = state.read().await;
    let original_forward = map.get(&id);
    let is_none = original_forward.is_none();
    let forward = if is_none {
        let mut kind_many = false;
        let support_track_id = header.get("Support-TrackId");
        if support_track_id.is_some() && support_track_id.unwrap().as_bytes() == b"true" {
            kind_many = true;
        }
        PeerForward::new(id.clone(), kind_many)
    } else {
        original_forward.unwrap().clone()
    };
    drop(map);
    let answer = forward.set_anchor(offer).await?;
    if is_none {
        let mut map = state.write().await;
        if map.contains_key(&id) {
            return Err(anyhow::anyhow!("resource already exists").into());
        }
        map.insert(forward.get_id(), forward);
    }
    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header("Content-Type", "application/sdp")
        .header("Accept-Patch", "application/trickle-ice-sdpfrag")
        .header("E-Tag", id)
        .header("Location", uri.to_string())
        .body(answer.sdp)?)
}

async fn whep(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    uri: Uri,
    body: String,
) -> Result<Response<String>, AppError> {
    let offer = RTCSessionDescription::offer(body)?;
    let map = state.read().await;
    let forward = map.get(&id);
    if forward.is_none() {
        return Err(anyhow::anyhow!("resource not found").into());
    }
    let forward = forward.unwrap().clone();
    drop(map);
    let answer = forward.add_subscribe(offer).await?;
    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header("Content-Type", "application/sdp")
        .header("Accept-Patch", "application/trickle-ice-sdpfrag")
        .header("E-Tag", forward.get_id())
        .header("Location", uri.to_string())
        .body(answer.sdp)?)
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
