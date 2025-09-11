use axum::Router;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::post;
use http::{HeaderMap, StatusCode, header};
use tracing::debug;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

use iceserver::link_header;

use crate::AppState;

pub fn route() -> Router<AppState> {
    Router::new().route(&api::path::whip("{stream}"), post(whip))
}

async fn whip(
    State(state): State<AppState>,
    Path(stream): Path<String>,
    header: HeaderMap,
    body: String,
) -> crate::result::Result<Response<String>> {
    let content_type = header
        .get(header::CONTENT_TYPE)
        .ok_or(anyhow::anyhow!("Content-Type is required"))?;
    if content_type.to_str()? != "application/sdp" {
        return Err(anyhow::anyhow!("Content-Type must be application/sdp").into());
    }
    let offer = RTCSessionDescription::offer(body)?;
    debug!("offer: {}", offer.sdp);
    let (answer, session) = state.stream_manager.publish(stream.clone(), offer).await?;
    debug!("answer: {}", answer.sdp);
    let mut builder = Response::builder()
        .status(StatusCode::CREATED)
        .header(header::CONTENT_TYPE, "application/sdp")
        .header("Accept-Patch", "application/trickle-ice-sdpfrag")
        .header(header::LOCATION, api::path::session(&stream, &session));
    for link in link_header(state.config.ice_servers.clone()) {
        builder = builder.header(header::LINK, link);
    }
    Ok(builder.body(answer.sdp)?)
}
