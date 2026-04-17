use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Router,
};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::track::track_local::TrackLocal;

use crate::LiveSrcManager;

pub fn create_router() -> Router<LiveSrcManager> {
    Router::new()
        // 保持向后兼容：/whep 使用默认 path
        .route("/whep", post(handle_whep_default))
        // 新的多路径路由：/whep/{path_name}
        .route("/whep/{path_name}", post(handle_whep_with_path))
}

/// 向后兼容的默认路由 - 使用旧的 LiveSrcManager
async fn handle_whep_default(
    State(manager): State<LiveSrcManager>,
    body: Bytes,
) -> Result<Response, WhepError> {
    let stream_id = &manager.stream_id;
    info!(stream_id, "WHEP request received (legacy mode)");

    let body_str = String::from_utf8_lossy(&body);
    let offer = RTCSessionDescription::offer(body_str.to_string()).map_err(|e| {
        error!("Failed to parse SDP offer: {}", e);
        WhepError::InvalidSdp
    })?;

    debug!("SDP offer parsed successfully");

    // Get ICE servers from config
    let ice_servers = {
        let config = manager.config.read().unwrap();
        if config.webrtc.ice_servers.is_empty() {
            vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                ..Default::default()
            }]
        } else {
            vec![RTCIceServer {
                urls: config.webrtc.ice_servers.clone(),
                ..Default::default()
            }]
        }
    };

    let rtc_config = RTCConfiguration {
        ice_servers,
        ..Default::default()
    };

    // Create peer connection
    let pc = manager
        .webrtc_api
        .new_peer_connection(rtc_config)
        .await
        .map_err(|e| {
            error!("Failed to create peer connection: {}", e);
            WhepError::InternalError
        })?;
    let pc = Arc::new(pc);

    info!("PeerConnection created");

    // Add subscriber and get track (legacy mode - uses old add_subscriber)
    let video_track = manager.add_subscriber().ok_or_else(|| {
        error!("Failed to add subscriber");
        WhepError::InternalError
    })?;

    pc.add_track(video_track as Arc<dyn TrackLocal + Send + Sync>)
        .await
        .map_err(|e| {
            error!("Failed to add track: {}", e);
            WhepError::InternalError
        })?;

    debug!("Video track added to peer connection");

    // Setup connection state handler
    let pc_clone = pc.clone();
    let manager_clone = manager.clone();
    let stream_id_clone = stream_id.clone();
    pc.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        let manager = manager_clone.clone();
        let pc = pc_clone.clone();
        let stream_id = stream_id_clone.clone();
        Box::pin(async move {
            info!(stream_id, state = ?s, "PeerConnection state changed");
            match s {
                RTCPeerConnectionState::Connected => {
                    info!(stream_id, "WebRTC connection established");
                }
                RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Disconnected
                | RTCPeerConnectionState::Closed => {
                    warn!(stream_id, "Connection closed");
                    manager.remove_subscriber();
                    manager.remove_whep_session();
                    let _ = pc.close().await;
                }
                _ => {}
            }
        })
    }));

    // Set remote description
    pc.set_remote_description(offer).await.map_err(|e| {
        error!("Failed to set remote description: {}", e);
        WhepError::InternalError
    })?;

    // Create answer
    let answer = pc.create_answer(None).await.map_err(|e| {
        error!("Failed to create answer: {}", e);
        WhepError::InternalError
    })?;

    // Set local description
    let mut gather_complete = pc.gathering_complete_promise().await;
    pc.set_local_description(answer).await.map_err(|e| {
        error!("Failed to set local description: {}", e);
        WhepError::InternalError
    })?;

    // Wait for ICE gathering with timeout
    let _ = tokio::time::timeout(Duration::from_secs(3), gather_complete.recv()).await;

    let local_desc = pc.local_description().await.ok_or_else(|| {
        error!("No local description available");
        WhepError::InternalError
    })?;

    // Store session
    manager.set_whep_session(pc.clone());

    info!(stream_id, "WHEP session created");

    Response::builder()
        .status(StatusCode::CREATED)
        .header(header::CONTENT_TYPE, "application/sdp")
        .header(header::LOCATION, format!("/session/{}", stream_id))
        .body(local_desc.sdp.into())
        .map_err(|e| {
            error!("Failed to build response: {}", e);
            WhepError::InternalError
        })
}

/// 新的多路径 WHEP handler - 使用 PathManager
async fn handle_whep_with_path(
    State(manager): State<LiveSrcManager>,
    Path(path_name): Path<String>,
    body: Bytes,
) -> Result<Response, WhepError> {
    info!(path = %path_name, "WHEP request received for path");

    let body_str = String::from_utf8_lossy(&body);
    let offer = RTCSessionDescription::offer(body_str.to_string()).map_err(|e| {
        error!(path = %path_name, "Failed to parse SDP offer: {}", e);
        WhepError::InvalidSdp
    })?;

    debug!("SDP offer parsed successfully");

    // Get ICE servers from config
    let ice_servers = {
        let config = manager.config.read().unwrap();
        if config.webrtc.ice_servers.is_empty() {
            vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_string()],
                ..Default::default()
            }]
        } else {
            vec![RTCIceServer {
                urls: config.webrtc.ice_servers.clone(),
                ..Default::default()
            }]
        }
    };

    let rtc_config = RTCConfiguration {
        ice_servers,
        ..Default::default()
    };

    // Create peer connection
    let pc = manager
        .webrtc_api
        .new_peer_connection(rtc_config)
        .await
        .map_err(|e| {
            error!("Failed to create peer connection: {}", e);
            WhepError::InternalError
        })?;
    let pc = Arc::new(pc);

    info!("PeerConnection created");

    // Add subscriber via PathManager
    let video_track = manager
        .path_manager
        .add_subscriber(&path_name)
        .map_err(|e| {
            error!(path = %path_name, "Failed to add subscriber: {}", e);
            // 检查错误类型
            let err_msg = e.to_string();
            if err_msg.contains("not found") {
                WhepError::PathNotFound
            } else if err_msg.contains("exceeded max_readers") {
                WhepError::MaxReadersExceeded
            } else {
                WhepError::InternalError
            }
        })?
        .ok_or_else(|| {
            error!(path = %path_name, "PathManager returned None for track");
            WhepError::InternalError
        })?;

    pc.add_track(video_track as Arc<dyn TrackLocal + Send + Sync>)
        .await
        .map_err(|e| {
            error!("Failed to add track: {}", e);
            // 回滚订阅者计数
            let _ = manager.path_manager.remove_subscriber(&path_name);
            WhepError::InternalError
        })?;

    debug!("Video track added to peer connection");

    // Setup connection state handler
    let pc_clone = pc.clone();
    let path_manager = manager.path_manager.clone();
    let path_name_clone = path_name.clone();
    pc.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        let path_manager = path_manager.clone();
        let pc = pc_clone.clone();
        let path_name = path_name_clone.clone();
        Box::pin(async move {
            info!(path = %path_name, state = ?s, "PeerConnection state changed");
            match s {
                RTCPeerConnectionState::Connected => {
                    info!(path = %path_name, "WebRTC connection established");
                }
                RTCPeerConnectionState::Failed
                | RTCPeerConnectionState::Disconnected
                | RTCPeerConnectionState::Closed => {
                    warn!(path = %path_name, "Connection closed");
                    // 通过 PathManager 移除订阅者
                    if let Err(e) = path_manager.remove_subscriber(&path_name) {
                        error!(path = %path_name, "Failed to remove subscriber: {}", e);
                    }
                    let _ = pc.close().await;
                }
                _ => {}
            }
        })
    }));

    // Set remote description
    pc.set_remote_description(offer).await.map_err(|e| {
        error!("Failed to set remote description: {}", e);
        WhepError::InternalError
    })?;

    // Create answer
    let answer = pc.create_answer(None).await.map_err(|e| {
        error!("Failed to create answer: {}", e);
        WhepError::InternalError
    })?;

    // Set local description
    let mut gather_complete = pc.gathering_complete_promise().await;
    pc.set_local_description(answer).await.map_err(|e| {
        error!("Failed to set local description: {}", e);
        WhepError::InternalError
    })?;

    // Wait for ICE gathering with timeout
    let _ = tokio::time::timeout(Duration::from_secs(3), gather_complete.recv()).await;

    let local_desc = pc.local_description().await.ok_or_else(|| {
        error!("No local description available");
        WhepError::InternalError
    })?;

    info!(path = %path_name, "WHEP session created");

    Response::builder()
        .status(StatusCode::CREATED)
        .header(header::CONTENT_TYPE, "application/sdp")
        .header(header::LOCATION, format!("/session/{}", path_name))
        .body(local_desc.sdp.into())
        .map_err(|e| {
            error!("Failed to build response: {}", e);
            WhepError::InternalError
        })
}

#[derive(Debug)]
pub enum WhepError {
    InvalidSdp,
    InternalError,
    PathNotFound,
    MaxReadersExceeded,
}

impl IntoResponse for WhepError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            WhepError::InvalidSdp => (StatusCode::BAD_REQUEST, "Invalid SDP"),
            WhepError::InternalError => (StatusCode::INTERNAL_SERVER_ERROR, "Internal error"),
            WhepError::PathNotFound => (StatusCode::NOT_FOUND, "Path not found"),
            WhepError::MaxReadersExceeded => (StatusCode::SERVICE_UNAVAILABLE, "Maximum number of readers exceeded"),
        };
        (status, message).into_response()
    }
}
