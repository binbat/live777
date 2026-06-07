use axum::body::Bytes;
use axum::http::{StatusCode, header};
use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::{delete, options, patch, post},
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tracing::{debug, error, info, warn};
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCIceCandidateInit, RTCIceGatheringState, RTCIceServer,
    RTCPeerConnectionState, RTCSessionDescription, Registry, SettingEngine,
};

use super::auth::{AppState, Claims};

pub fn create_router() -> Router<AppState> {
    Router::<AppState>::new()
        .route("/api/whep/:stream_id", post(whep_handler))
        .route("/api/whep/:stream_id", patch(whep_patch))
        .route("/api/whep/:stream_id", delete(whep_delete))
        .route("/api/whep/:stream_id", options(whep_options))
}

#[derive(Clone)]
struct WhepHandler {
    manager: crate::LiveCamManager,
    stream_id: String,
    gather_complete: Arc<Notify>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for WhepHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        debug!(stream_id = %self.stream_id, state = %state, "PeerConnection state changed.");
        if state == RTCPeerConnectionState::Failed {
            debug!(stream_id = %self.stream_id, "Cleaning up subscriber and session.");
            self.manager.remove_subscriber(&self.stream_id);
            self.manager.remove_whep_session(&self.stream_id);
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            debug!(stream_id = %self.stream_id, "ICE gathering complete");
            self.gather_complete.notify_one();
        }
    }
}

async fn whep_handler(
    State(app_state): State<AppState>,
    _claims: Claims,
    Path(stream_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    info!(stream_id, "Received WHEP POST request for stream.");
    let body_str = String::from_utf8_lossy(&body);
    let offer = match RTCSessionDescription::offer((body_str).to_string()) {
        Ok(offer) => {
            debug!(stream_id, "SDP offer parsed successfully.");
            offer
        }
        Err(e) => {
            error!(stream_id, error = %e, "Failed to parse SDP offer.");
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    };

    let manager = &app_state.manager;
    let track = match manager.add_subscriber(&stream_id) {
        Some(track) => {
            info!(stream_id, "Subscriber added successfully.");
            track
        }
        None => {
            warn!(stream_id, "Requested stream not found.");
            return (StatusCode::NOT_FOUND, "Stream not found".to_string()).into_response();
        }
    };

    let ice_servers = {
        let config = app_state.config.read().unwrap();
        let servers: Vec<RTCIceServer> = config
            .ice_servers
            .iter()
            .map(|s| RTCIceServer {
                urls: s.urls.clone(),
                username: s.username.clone(),
                credential: s.credential.clone(),
            })
            .collect();
        if servers.is_empty() {
            vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                ..Default::default()
            }]
        } else {
            servers
        }
    };
    debug!(stream_id, "RTC configuration prepared.");

    let mut m = MediaEngine::default();
    if let Err(e) = m.register_default_codecs() {
        error!(stream_id, error = %e, "Failed to register default codecs.");
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    debug!(stream_id, "Media engine initialized with default codecs.");

    let mut setting_engine = SettingEngine::default();
    setting_engine.set_ice_timeouts(
        Some(Duration::from_secs(15)),
        Some(Duration::from_secs(30)),
        Some(Duration::from_secs(2)),
    );

    let registry = Registry::new();
    let gather_complete = Arc::new(Notify::new());
    let handler = Arc::new(WhepHandler {
        manager: manager.clone(),
        stream_id: stream_id.clone(),
        gather_complete: gather_complete.clone(),
    });

    let config = RTCConfigurationBuilder::new()
        .with_ice_servers(ice_servers)
        .build();

    let peer: Arc<dyn PeerConnection> = match PeerConnectionBuilder::<std::net::SocketAddr>::new()
        .with_media_engine(m)
        .with_setting_engine(setting_engine)
        .with_interceptor_registry(registry)
        .with_handler(handler)
        .with_udp_addrs(vec!["0.0.0.0:0".parse().unwrap()])
        .with_configuration(config)
        .build()
        .await
    {
        Ok(pc) => {
            info!(stream_id, "PeerConnection created successfully.");
            Arc::new(pc)
        }
        Err(e) => {
            error!(stream_id, error = %e, "Failed to create PeerConnection.");
            manager.remove_subscriber(&stream_id);
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    if let Err(e) = peer.add_track(track.clone() as Arc<dyn TrackLocal>).await {
        error!(stream_id, error = %e, "Failed to add track.");
        manager.remove_subscriber(&stream_id);
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    debug!(stream_id, "Track added to PeerConnection.");

    if let Err(e) = peer.set_remote_description(offer).await {
        error!(stream_id, error = %e, "Failed to set remote description.");
        manager.remove_subscriber(&stream_id);
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    debug!(stream_id, "Remote description set.");

    let answer = match peer.create_answer(None).await {
        Ok(a) => {
            debug!(stream_id, "Answer created successfully.");
            a
        }
        Err(e) => {
            error!(stream_id, error = %e, "Failed to create answer.");
            manager.remove_subscriber(&stream_id);
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    if let Err(e) = peer.set_local_description(answer).await {
        error!(stream_id, error = %e, "Failed to set local description.");
        manager.remove_subscriber(&stream_id);
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }
    debug!(
        stream_id,
        "Local description set, waiting for ICE gathering..."
    );

    // Wait for ICE gathering to complete with a timeout.
    if tokio::time::timeout(Duration::from_secs(3), gather_complete.notified())
        .await
        .is_err()
    {
        warn!(
            stream_id,
            "ICE gathering timed out after 3s, using partial description"
        );
    }

    let local_desc = match peer.local_description().await {
        Some(desc) => {
            info!(stream_id, "Local description obtained.");
            desc
        }
        None => {
            error!(stream_id, "No local description available.");
            manager.remove_subscriber(&stream_id);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "No local description".to_string(),
            )
                .into_response();
        }
    };

    manager.add_whep_session(stream_id.clone(), peer.clone());
    info!(stream_id, "WHEP session added.");
    let config = app_state.config.read().unwrap();
    let server_url = config.http.public.clone();
    let location = format!("{}/api/whep/{}", server_url, stream_id);

    info!(stream_id, "WHEP handler completed successfully.");
    match Response::builder()
        .status(StatusCode::CREATED)
        .header(header::CONTENT_TYPE, "application/sdp")
        .header(header::LOCATION, &location)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCESS_CONTROL_EXPOSE_HEADERS, "Location")
        .body(Body::from(local_desc.sdp))
    {
        Ok(response) => response,
        Err(e) => {
            error!(stream_id, error = %e, "Failed to build response");
            manager.remove_subscriber(&stream_id);
            manager.remove_whep_session(&stream_id);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build response: {}", e),
            )
                .into_response()
        }
    }
}

async fn whep_patch(
    State(app_state): State<AppState>,
    _claims: Claims,
    Path(stream_id): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    info!(stream_id, "Received WHEP PATCH request for stream.");
    let body_str = String::from_utf8_lossy(&body).to_string();
    let candidate_str = body_str.trim();

    let manager = &app_state.manager;
    let pc = match manager.get_whep_session(&stream_id) {
        Some(pc) => {
            debug!(stream_id, "WHEP session retrieved.");
            pc
        }
        None => {
            warn!(stream_id, "Session not found for PATCH.");
            return (StatusCode::NOT_FOUND, "Session not found").into_response();
        }
    };

    let candidate_line = if candidate_str.starts_with("a=candidate:") {
        candidate_str.to_string()
    } else if candidate_str.starts_with("candidate:") {
        format!("a={}", candidate_str)
    } else {
        warn!(stream_id, candidate = %candidate_str, "Invalid ICE candidate format.");
        return (StatusCode::BAD_REQUEST, "Invalid candidate format").into_response();
    };

    let candidate_init = RTCIceCandidateInit {
        candidate: candidate_line,
        sdp_mid: Some("0".to_string()),
        sdp_mline_index: Some(0),
        username_fragment: None,
        url: None,
    };

    match pc.add_ice_candidate(candidate_init).await {
        Ok(_) => {
            info!(stream_id, "ICE candidate added.");
            (StatusCode::OK, "").into_response()
        }
        Err(e) => {
            error!(stream_id, error = %e, "Failed to add ICE candidate.");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

async fn whep_delete(
    State(app_state): State<AppState>,
    _claims: Claims,
    Path(stream_id): Path<String>,
) -> impl IntoResponse {
    info!(stream_id, "Received WHEP DELETE request for stream.");
    let manager = &app_state.manager;
    if let Some(pc) = manager.get_whep_session(&stream_id) {
        let _ = pc.close().await;
        manager.remove_whep_session(&stream_id);
        manager.remove_subscriber(&stream_id);
        info!(stream_id, "WHEP session closed.");
        (StatusCode::NO_CONTENT, "").into_response()
    } else {
        warn!(stream_id, "Session not found for DELETE.");
        (StatusCode::NOT_FOUND, "Session not found").into_response()
    }
}

async fn whep_options() -> impl IntoResponse {
    debug!("Received WHEP OPTIONS request.");
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            "OPTIONS, POST, PATCH, DELETE",
        )
        .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "Content-Type")
        .body(Body::empty())
        .unwrap()
}
