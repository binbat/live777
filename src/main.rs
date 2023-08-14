use std::net::SocketAddr;
use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Router, routing::post};
use hyper::header;
use tower_http::services::{ServeDir, ServeFile};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use crate::rtc::PeerForward;

mod rtc;


#[tokio::main]
async fn main() {
    let shared_state = SharedState::default();
    let serve_dir = ServeDir::new("assets").not_found_service(ServeFile::new("assets/index.html"));
    let app = Router::new()
        .route("/whip", post(whip))
        .route("/whep/endpoint", post(whep))
        .nest_service("/", serve_dir.clone())
        .with_state(Arc::clone(&shared_state));
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

type SharedState = Arc<PeerForward>;

async fn whip(State(state): State<SharedState>, body: String) -> impl IntoResponse {
    let offer = RTCSessionDescription::offer(body).unwrap();
    match state.set_anchor(offer).await {
        Ok(sdp) => {
            (
                StatusCode::CREATED,
                [(header::CONTENT_TYPE, "application/sdp")],
                sdp.sdp,
            )
        }
        Err(err) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "application/sdp")],
                err.to_string(),
            )
        }
    }
}

async fn whep(State(state): State<SharedState>, body: String) -> impl IntoResponse {
    let offer = RTCSessionDescription::offer(body).unwrap();
    match state.add_subscribe(offer).await {
        Ok(sdp) => {
            (
                StatusCode::CREATED,
                [(header::CONTENT_TYPE, "application/sdp")],
                sdp.sdp,
            )
        }
        Err(err) => {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "application/sdp")],
                err.to_string(),
            )
        }
    }
}
