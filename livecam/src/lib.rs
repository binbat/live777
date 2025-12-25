use axum::Router;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use std::collections::HashMap;
use std::future::Future;
#[cfg(not(riscv_mode))]
use std::process::Child;
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};
use webrtc::api::API;
use webrtc::api::APIBuilder;
use webrtc::api::media_engine::MediaEngine;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;

#[cfg(feature = "webui")]
use axum::http::Uri;
#[cfg(feature = "webui")]
use axum::response::Response;
#[cfg(riscv_mode)]
use milkv_libs::stream;
#[cfg(feature = "webui")]
use rust_embed::RustEmbed;

use self::config::{CameraConfig, Config as ConfigRs};
use crate::auth::AppState;

pub mod auth;
pub mod config;
pub mod control_receiver;
pub mod network;
pub mod rtp_receiver;
mod test;
pub mod utils;
pub mod whep_handler;

#[cfg(feature = "webui")]
#[derive(RustEmbed, Clone)]
#[folder = "../assets/livecam/"]
struct Assets;

#[cfg(feature = "webui")]
async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();

            let mut response = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.as_ref());

            if path.starts_with("assets/") || path.ends_with(".js") || path.ends_with(".css") {
                response = response.header(header::CACHE_CONTROL, "public, max-age=31536000");
            } else {
                response = response.header(header::CACHE_CONTROL, "public, max-age=3600");
            }
            response.body(content.data.into()).unwrap()
        }
        None => {
            if !path.contains('.')
                && !path.starts_with("api/")
                && !path.starts_with("whep/")
                && !path.starts_with("session/")
            {
                if let Some(index) = Assets::get("index.html") {
                    debug!("Serving index.html for SPA route: {}", path);
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "text/html")
                        .header(header::CACHE_CONTROL, "no-cache")
                        .body(index.data.into())
                        .unwrap()
                } else {
                    error!("index.html not found in embedded assets");
                    (StatusCode::NOT_FOUND, "index.html not found").into_response()
                }
            } else {
                warn!("Static file not found: {}", path);
                (StatusCode::NOT_FOUND, "404 Not Found").into_response()
            }
        }
    }
}

async fn health_check() -> impl IntoResponse {
    use serde_json::json;
    axum::Json(json!({
        "status": "ok",
        "service": "livecam",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": env!("CARGO_PKG_VERSION")
    }))
}

pub struct StreamState {
    subscriber_count: usize,
    track: Arc<TrackLocalStaticRTP>,
    rtp_receiver_handle: Option<JoinHandle<()>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    control_receiver_handle: Option<JoinHandle<()>>,
    control_shutdown_tx: Option<mpsc::Sender<()>>,
    datachannel_tx: Option<tokio::sync::broadcast::Sender<Vec<u8>>>,
    datachannel_rx: Option<tokio::sync::broadcast::Receiver<Vec<u8>>>,
    config: CameraConfig,
    #[cfg(not(riscv_mode))]
    child_process: Option<Arc<Mutex<Child>>>,
    #[cfg(riscv_mode)]
    stream_handle: Option<Arc<Mutex<stream::StreamHandle>>>,
}

#[derive(Clone)]
pub struct PortManager {
    next_port: Arc<Mutex<u16>>,
}

impl PortManager {
    pub fn new(start_port: u16) -> Self {
        Self {
            next_port: Arc::new(Mutex::new(start_port)),
        }
    }

    pub fn get_next_port(&self) -> u16 {
        let mut port = self.next_port.lock().unwrap();
        let current = *port;
        *port = port.wrapping_add(1);
        current
    }
}

#[derive(Clone)]
pub struct LiveCamManager {
    streams: Arc<Mutex<HashMap<String, StreamState>>>,
    pub webrtc_api: Arc<API>,
    config: Arc<RwLock<ConfigRs>>,
    port_manager: PortManager,
    whep_sessions: Arc<Mutex<HashMap<String, Arc<RTCPeerConnection>>>>,
}

impl LiveCamManager {
    pub fn new(cfg: Arc<RwLock<ConfigRs>>, webrtc_api: Arc<API>) -> Self {
        let mut config_guard = cfg.write().unwrap();
        if let Err(e) = config_guard.validate() {
            error!("Config validation failed: {}", e);
        }
        drop(config_guard);

        let config_read = cfg.read().unwrap();
        let cameras = config_read.cameras.clone();
        let start_port = config_read.stream.rtp_port;
        drop(config_read);

        let streams = cameras
            .into_iter()
            .map(|cam| {
                let track = Arc::new(TrackLocalStaticRTP::new(
                    cam.codec.clone().into(),
                    cam.id.clone(),
                    "livecam-stream".to_owned(),
                ));
                let state = StreamState {
                    subscriber_count: 0,
                    track,
                    rtp_receiver_handle: None,
                    shutdown_tx: None,
                    control_receiver_handle: None,
                    control_shutdown_tx: None,
                    datachannel_tx: None,
                    datachannel_rx: None,
                    config: cam.clone(),
                    #[cfg(not(riscv_mode))]
                    child_process: None,
                    #[cfg(riscv_mode)]
                    stream_handle: None,
                };
                (cam.id.clone(), state)
            })
            .collect();

        Self {
            streams: Arc::new(Mutex::new(streams)),
            webrtc_api,
            config: cfg,
            port_manager: PortManager::new(start_port),
            whep_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn add_subscriber(&self, stream_id: &str) -> Option<Arc<TrackLocalStaticRTP>> {
        let mut streams = self.streams.lock().unwrap();
        let state = if let Some(existing) = streams.get_mut(stream_id) {
            existing
        } else {
            info!(stream_id, "Dynamic stream created.");
            let config_read = self.config.read().unwrap();
            let rtp_port = self.port_manager.get_next_port();
            let command = {
                let tmpl = &config_read.stream.command;
                if tmpl.contains("{port}") {
                    tmpl.replace("{port}", &rtp_port.to_string())
                } else {
                    tmpl.replace("5004", &rtp_port.to_string())
                }
            };
            let cam_config = CameraConfig {
                id: stream_id.to_string(),
                rtp_port,
                control_port: None,
                codec: config::CodecConfig {
                    mime_type: "video/H264".to_string(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: Some(
                        "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f"
                            .to_string(),
                    ),
                },
                command,
            };
            drop(config_read);

            let track = Arc::new(TrackLocalStaticRTP::new(
                cam_config.codec.clone().into(),
                stream_id.to_string(),
                "livecam-dynamic-stream".to_owned(),
            ));
            let new_state = StreamState {
                subscriber_count: 0,
                track: track.clone(),
                rtp_receiver_handle: None,
                shutdown_tx: None,
                control_receiver_handle: None,
                control_shutdown_tx: None,
                datachannel_tx: None,
                datachannel_rx: None,
                config: cam_config,
                #[cfg(not(riscv_mode))]
                child_process: None,
                #[cfg(riscv_mode)]
                stream_handle: None,
            };
            streams.insert(stream_id.to_string(), new_state);
            streams.get_mut(stream_id).unwrap()
        };

        state.subscriber_count += 1;
        info!(
            stream_id,
            subscribers = state.subscriber_count,
            port = state.config.rtp_port,
            "Subscriber added."
        );

        if state.subscriber_count == 1 {
            info!(
                stream_id,
                port = state.config.rtp_port,
                "First subscriber arrived, starting video source and RTP receiver."
            );

            #[cfg(not(riscv_mode))]
            {
                let command = state.config.command.clone();
                match cli::create_child(Some(command)) {
                    Ok(Some(child_mutex)) => {
                        let child_arc = Arc::new(child_mutex);
                        state.child_process = Some(child_arc);
                        info!(stream_id, "child process started successfully.");
                    }
                    Ok(None) => {
                        warn!(stream_id, "No child process provided.");
                    }
                    Err(e) => {
                        error!(stream_id, error = %e, "Failed to create child process.");
                    }
                }
            }

            let (tx, rx) = mpsc::channel(1);
            state.shutdown_tx = Some(tx);
            let track_clone = state.track.clone();
            let port = state.config.rtp_port;

            let handle = tokio::spawn(async move {
                if let Err(e) = rtp_receiver::start(port, track_clone, rx).await {
                    error!(port, error = %e, "RTP receiver task failed.");
                }
            });
            state.rtp_receiver_handle = Some(handle);

            // Start UDP control receiver if control_port is configured
            if let Some(control_port) = state.config.control_port {
                info!(
                    stream_id,
                    control_port,
                    "Starting UDP control receiver for PTZ/control commands"
                );

                // Create broadcast channels for bidirectional communication
                let (dc_tx, dc_rx1) = tokio::sync::broadcast::channel::<Vec<u8>>(1024);
                let dc_rx2 = dc_tx.subscribe();
                
                state.datachannel_tx = Some(dc_tx.clone());
                state.datachannel_rx = Some(dc_rx2);

                let (control_tx, control_rx) = mpsc::channel(1);
                state.control_shutdown_tx = Some(control_tx);

                let stream_id_clone = stream_id.to_string();
                let control_handle = tokio::spawn(async move {
                    if let Err(e) = control_receiver::start(
                        control_port,
                        stream_id_clone,
                        dc_tx,
                        dc_rx1,
                        control_rx,
                    )
                    .await
                    {
                        error!(
                            port = control_port,
                            error = %e,
                            "UDP control receiver task failed"
                        );
                    }
                });
                state.control_receiver_handle = Some(control_handle);
            }
        }
        Some(state.track.clone())
    }

    pub fn remove_subscriber(&self, stream_id: &str) {
        let mut streams = self.streams.lock().unwrap();
        if let Some(state) = streams.get_mut(stream_id) {
            if state.subscriber_count > 0 {
                state.subscriber_count -= 1;
                info!(
                    stream_id,
                    subscribers = state.subscriber_count,
                    "Subscriber removed."
                );
            }

            if state.subscriber_count == 0 {
                info!(
                    stream_id,
                    "Last subscriber left, stopping video source and RTP receiver."
                );

                let should_remove = !state.config.command.contains("testsrc");

                #[cfg(not(riscv_mode))]
                {
                    if let Some(child_arc) = state.child_process.take() {
                        debug!(stream_id, "Stopping child process.");

                        let child_clone = child_arc.clone();
                        let stream_id_clone = stream_id.to_string();
                        tokio::spawn(async move {
                            let mut child_guard = child_clone.lock().unwrap();

                            if let Err(e) = child_guard.kill() {
                                error!(stream_id = %stream_id_clone, error = %e, "Failed to kill child process.");
                            } else {
                                info!(stream_id = %stream_id_clone, "child process killed.");
                            }

                            match child_guard.wait() {
                                Ok(status) => {
                                    info!(stream_id = %stream_id_clone, status = ?status, "child process exited.");
                                }
                                Err(e) => {
                                    error!(stream_id = %stream_id_clone, error = %e, "Error waiting for child process.");
                                }
                            }
                        });
                    }
                }

                #[cfg(riscv_mode)]
                {
                    if let Some(handle_arc) = state.stream_handle.take() {
                        let stream_id_clone = stream_id.to_string();
                        tokio::spawn(async move {
                            let handle = handle_arc.lock().unwrap();
                            handle.stop();
                            info!(stream_id = %stream_id_clone, "stream handle stopped.");
                        });
                    }
                }

                if let Some(tx) = state.shutdown_tx.take() {
                    let _ = tx.try_send(());
                }
                if let Some(handle) = state.rtp_receiver_handle.take() {
                    handle.abort();
                }

                // Stop control receiver
                if let Some(tx) = state.control_shutdown_tx.take() {
                    let _ = tx.try_send(());
                }
                if let Some(handle) = state.control_receiver_handle.take() {
                    handle.abort();
                }

                if should_remove {
                    streams.remove(stream_id);
                    info!(stream_id, "stream removed from manager.");
                }
            }
        }
    }

    pub async fn shutdown(&self) {
        let sessions = {
            let mut sessions_guard = self.whep_sessions.lock().unwrap();
            std::mem::take(&mut *sessions_guard)
        };
        for (stream_id, pc) in sessions {
            debug!(stream_id = %stream_id, "Closing peer connection.");
            if let Err(e) = pc.close().await {
                error!(stream_id = %stream_id, error = %e, "Failed to close peer connection.");
            }
            self.remove_whep_session(&stream_id);
            self.remove_subscriber(&stream_id);
        }

        let streams = {
            let mut streams_guard = self.streams.lock().unwrap();
            std::mem::take(&mut *streams_guard)
        };
        for (stream_id, mut state) in streams {
            debug!(stream_id = %stream_id, "shutdown stream.");

            #[cfg(not(riscv_mode))]
            if let Some(child_arc) = state.child_process.take() {
                let mut child_guard = child_arc.lock().unwrap();
                if let Err(e) = child_guard.kill() {
                    error!(stream_id = %stream_id, error = %e, "Failed to kill child process.");
                } else {
                    info!(stream_id = %stream_id, "child process.");
                }
                if let Err(e) = child_guard.wait() {
                    error!(stream_id = %stream_id, error = %e, "Error waiting for child process.");
                }
            }

            if let Some(tx) = state.shutdown_tx.take() {
                let _ = tx.send(()).await;
            }
            if let Some(handle) = state.rtp_receiver_handle.take() {
                handle.abort();
            }

            if let Some(tx) = state.control_shutdown_tx.take() {
                let _ = tx.send(()).await;
            }
            if let Some(handle) = state.control_receiver_handle.take() {
                handle.abort();
            }
        }

        info!("shutdown completed.");
    }

    pub fn add_whep_session(&self, stream_id: String, pc: Arc<RTCPeerConnection>) {
        let mut sessions = self.whep_sessions.lock().unwrap();
        sessions.insert(stream_id, pc);
    }

    pub fn remove_whep_session(&self, stream_id: &str) {
        let mut sessions = self.whep_sessions.lock().unwrap();
        sessions.remove(stream_id);
    }

    pub fn get_whep_session(&self, stream_id: &str) -> Option<Arc<RTCPeerConnection>> {
        let sessions = self.whep_sessions.lock().unwrap();
        sessions.get(stream_id).cloned()
    }

    /// Get DataChannel sender for injecting control messages from UDP
    pub fn get_datachannel_sender(
        &self,
        stream_id: &str,
    ) -> Option<tokio::sync::broadcast::Sender<Vec<u8>>> {
        let streams = self.streams.lock().unwrap();
        streams
            .get(stream_id)
            .and_then(|state| state.datachannel_tx.clone())
    }

    /// Get DataChannel receiver for receiving feedback messages
    pub fn get_datachannel_receiver(
        &self,
        stream_id: &str,
    ) -> Option<tokio::sync::broadcast::Receiver<Vec<u8>>> {
        let streams = self.streams.lock().unwrap();
        streams.get(stream_id).and_then(|state| {
            state
                .datachannel_tx
                .as_ref()
                .map(|tx| tx.subscribe())
        })
    }
}

pub async fn serve(
    cfg: Arc<RwLock<ConfigRs>>,
    listener: TcpListener,
    shutdown_signal: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    use webrtc::api::setting_engine::SettingEngine;
    let mut setting_engine = SettingEngine::default();
    setting_engine.set_ice_timeouts(
        Some(std::time::Duration::from_secs(15)),
        Some(std::time::Duration::from_secs(30)),
        Some(std::time::Duration::from_secs(2)),
    );

    let registry = Registry::new();
    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_setting_engine(setting_engine)
        .with_interceptor_registry(registry)
        .build();
    let webrtc_api = Arc::new(api);

    let livecam_manager = LiveCamManager::new(cfg.clone(), webrtc_api.clone());

    let (_shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    let app_state = AppState {
        config: cfg,
        manager: livecam_manager,
    };

    let mut app = Router::new()
        .route("/api/health", axum::routing::get(health_check))
        .merge(whep_handler::create_router())
        .merge(auth::create_auth_router())
        .merge(network::create_network_router())
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(vec![header::CONTENT_TYPE, header::AUTHORIZATION])
                .expose_headers(vec![header::LOCATION, header::CONTENT_TYPE]),
        )
        .with_state(app_state.clone());

    #[cfg(feature = "webui")]
    {
        if Assets::get("index.html").is_some() {
            debug!("index.html found in embedded assets");
        } else {
            error!("index.html NOT found in embedded assets");
        }

        app = app.fallback(static_handler);
    }

    #[cfg(not(feature = "webui"))]
    {
        info!("WebUI disabled, only API endpoints available");
        app = app.fallback(|| async {
            (
                StatusCode::NOT_FOUND,
                "WebUI not enabled. Enable with --features webui",
            )
        });
    }

    let addr = listener.local_addr()?;
    info!("Server listening on http://{}", addr);

    info!("Server started, processing requests...");
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal)
        .await?;

    app_state.manager.shutdown().await;

    info!("Server shutdown completed");
    Ok(())
}
