pub mod config;
pub mod utils;
pub mod whep_handler;  // Keep for testing
pub mod rtp;  // New: RTP output module

#[cfg(feature = "v4l2")]
pub mod v4l2_capture;

pub mod path_manager;
pub mod sources;

use axum::http::header;
use axum::Router;
use std::future::Future;
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{debug, error, info, warn};
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::{APIBuilder, API};
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;


use config::{CaptureSource, Config, Mode};

/// Stream state for StartOnDemand mode
struct StreamState {
    subscriber_count: usize,
    track: Arc<TrackLocalStaticRTP>,
    rtp_receiver_handle: Option<JoinHandle<()>>,
    shutdown_tx: Option<mpsc::Sender<()>>,
    
    // FFmpeg mode
    child_process: Option<Arc<cli::ChildGuard>>,
    
    // V4L2 mode
    #[cfg(feature = "v4l2")]
    v4l2_handle: Option<JoinHandle<()>>,
}

/// LiveSrc Manager - handles single camera stream with StartOnDemand
#[derive(Clone)]
pub struct LiveSrcManager {
    stream_id: String,
    state: Arc<Mutex<StreamState>>,
    whep_session: Arc<Mutex<Option<Arc<RTCPeerConnection>>>>,
    pub webrtc_api: Arc<API>,
    pub config: Arc<RwLock<Config>>,
    pub path_manager: Arc<path_manager::PathManager>,
}

impl LiveSrcManager {
    pub fn new(config: Arc<RwLock<Config>>, webrtc_api: Arc<API>) -> Self {
        let cfg = config.read().unwrap();
        
        // 检查是否为新的 paths 模式
        let is_paths_mode = !cfg.paths.is_empty();
        
        // Legacy mode: 需要 stream 和 camera 配置
        // Paths mode: 不需要，使用默认值
        let (stream_id, track) = if is_paths_mode {
            // Paths 模式 - 使用占位值（旧的 LiveSrcManager 不会被使用）
            let default_track = Arc::new(TrackLocalStaticRTP::new(
                cfg.path_defaults.codec.clone().into(),
                "legacy".to_string(),
                "livesrc-stream".to_owned(),
            ));
            ("legacy".to_string(), default_track)
        } else {
            // Legacy 模式 - 需要完整配置
            let stream_id = cfg.stream.as_ref().expect("stream config required in legacy mode").id.clone();
            let track = Arc::new(TrackLocalStaticRTP::new(
                cfg.camera.as_ref().expect("camera config required in legacy mode").codec.clone().into(),
                stream_id.clone(),
                "livesrc-stream".to_owned(),
            ));
            (stream_id, track)
        };
        
        drop(cfg);

        let state = StreamState {
            subscriber_count: 0,
            track,
            rtp_receiver_handle: None,
            shutdown_tx: None,
            child_process: None,
            #[cfg(feature = "v4l2")]
            v4l2_handle: None,
        };

        let path_manager = Arc::new(path_manager::PathManager::new(config.clone(), webrtc_api.clone()));

        Self {
            stream_id,
            state: Arc::new(Mutex::new(state)),
            whep_session: Arc::new(Mutex::new(None)),
            webrtc_api,
            config,
            path_manager,
        }
    }

    /// Add a subscriber - starts camera source on first subscriber (StartOnDemand)
    pub fn add_subscriber(&self) -> Option<Arc<TrackLocalStaticRTP>> {
        let mut state = self.state.lock().unwrap();
        state.subscriber_count += 1;

        info!(
            stream_id = %self.stream_id,
            subscribers = state.subscriber_count,
            "Subscriber added"
        );

        // Start on first subscriber (StartOnDemand mode)
        if state.subscriber_count == 1 {
            info!(
                stream_id = %self.stream_id,
                "First subscriber, starting camera source"
            );

            let config = self.config.read().unwrap();
            let camera = config.camera.as_ref().expect("camera config required");
            let stream = config.stream.as_ref().expect("stream config required");
            let capture_source = camera.source.clone();
            let rtp_port = stream.rtp_port;
            
            match capture_source {
                config::CaptureSource::Ffmpeg => {
                    let command = camera.command.clone().unwrap_or_default();
                    drop(config);
                    
                    // Start FFmpeg encoder
                    match cli::create_child(Some(command)) {
                        Ok(Some(child_guard)) => {
                            state.child_process = Some(Arc::new(child_guard));
                            info!(stream_id = %self.stream_id, "FFmpeg encoder started");
                        }
                        Ok(None) => {
                            warn!(stream_id = %self.stream_id, "No command provided");
                        }
                        Err(e) => {
                            error!(stream_id = %self.stream_id, error = %e, "Failed to start FFmpeg");
                        }
                    }

                    // TODO: Replace with new RTP output module
                    // Old RTP receiver code removed - will be replaced with libcamera-bridge integration
                    warn!(stream_id = %self.stream_id, "FFmpeg mode needs RTP integration update");
                }
                
                #[cfg(feature = "v4l2")]
                config::CaptureSource::V4l2 => {
                    let device = camera.device.clone();
                    let v4l2_config = camera.v4l2.clone().expect("v4l2 config required");
                    drop(config);
                    
                    info!(stream_id = %self.stream_id, "Starting V4L2 capture mode");
                    
                    // Create V4L2 capture
                    match v4l2_capture::V4l2Capture::new(&device, v4l2_config) {
                        Ok(mut capture) => {
                            if let Err(e) = capture.start() {
                                error!(stream_id = %self.stream_id, error = %e, "Failed to start V4L2 capture");
                                return Some(state.track.clone());
                            }
                            
                            info!(stream_id = %self.stream_id, "V4L2 capture started");
                            
                            // Create channels for frame data and shutdown
                            let (frame_tx, mut frame_rx) = mpsc::channel::<Vec<u8>>(10);
                            let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
                            state.shutdown_tx = Some(shutdown_tx);
                            
                            // Spawn V4L2 capture loop
                            let v4l2_handle = tokio::task::spawn_blocking(move || {
                                let rt = tokio::runtime::Handle::current();
                                rt.block_on(async {
                                    if let Err(e) = capture.capture_loop(frame_tx, shutdown_rx).await {
                                        error!("V4L2 capture loop error: {}", e);
                                    }
                                });
                            });
                            state.v4l2_handle = Some(v4l2_handle);
                            
                            // Spawn RTP packetizer to process frames
                            let track_clone = state.track.clone();
                            let rtp_handle = tokio::spawn(async move {
                                while let Some(frame_data) = frame_rx.recv().await {
                                    // Send raw H264/H265 frame to RTP track
                                    // TODO: Proper RTP packetization for H.264/H.265
                                    use bytes::Bytes;
                                    let packet = webrtc::rtp::packet::Packet {
                                        header: webrtc::rtp::header::Header {
                                            version: 2,
                                            padding: false,
                                            extension: false,
                                            marker: true, // Mark end of frame
                                            payload_type: 96, // Dynamic payload type for H264
                                            sequence_number: 0, // Will be set by track
                                            timestamp: 0, // Will be set by track
                                            ssrc: 0, // Will be set by track
                                            ..Default::default()
                                        },
                                        payload: Bytes::from(frame_data),
                                    };
                                    
                                    if let Err(e) = track_clone.write_rtp(&packet).await {
                                        error!("Failed to write RTP packet to track: {}", e);
                                        break;
                                    }
                                }
                            });
                            state.rtp_receiver_handle = Some(rtp_handle);
                        }
                        Err(e) => {
                            error!(stream_id = %self.stream_id, error = %e, "Failed to create V4L2 capture");
                        }
                    }
                }
                
                #[cfg(not(feature = "v4l2"))]
                config::CaptureSource::V4l2 => {
                    drop(config);
                    error!(stream_id = %self.stream_id, "V4L2 mode requested but v4l2 feature not enabled");
                }
            }
        }

        Some(state.track.clone())
    }

    /// Remove a subscriber - stops camera source when last subscriber leaves
    pub fn remove_subscriber(&self) {
        let mut state = self.state.lock().unwrap();

        if state.subscriber_count > 0 {
            state.subscriber_count -= 1;
            info!(
                stream_id = %self.stream_id,
                subscribers = state.subscriber_count,
                "Subscriber removed"
            );
        }

        // Stop on last subscriber (StartOnDemand mode)
        if state.subscriber_count == 0 {
            info!(
                stream_id = %self.stream_id,
                "Last subscriber left, stopping camera source"
            );

            // Stop FFmpeg (ChildGuard kills process on drop)
            if let Some(_child) = state.child_process.take() {
                debug!(stream_id = %self.stream_id, "FFmpeg encoder stopped");
            }
            
            // Stop V4L2 capture
            #[cfg(feature = "v4l2")]
            if let Some(handle) = state.v4l2_handle.take() {
                handle.abort();
                debug!(stream_id = %self.stream_id, "V4L2 capture stopped");
            }

            // Stop RTP receiver/packetizer
            if let Some(tx) = state.shutdown_tx.take() {
                let _ = tx.try_send(());
            }
            if let Some(handle) = state.rtp_receiver_handle.take() {
                handle.abort();
            }
        }
    }

    pub fn set_whep_session(&self, pc: Arc<RTCPeerConnection>) {
        let mut session = self.whep_session.lock().unwrap();
        *session = Some(pc);
    }

    pub fn remove_whep_session(&self) {
        let mut session = self.whep_session.lock().unwrap();
        *session = None;
    }

    pub async fn shutdown(&self) {
        // Close WHEP session
        if let Some(pc) = self.whep_session.lock().unwrap().take() {
            let _ = pc.close().await;
        }

        // Stop stream
        let mut state = self.state.lock().unwrap();
        if let Some(_child) = state.child_process.take() {
            info!(stream_id = %self.stream_id, "FFmpeg stopped");
        }
        
        #[cfg(feature = "v4l2")]
        if let Some(handle) = state.v4l2_handle.take() {
            handle.abort();
            info!(stream_id = %self.stream_id, "V4L2 capture stopped");
        }
        
        if let Some(tx) = state.shutdown_tx.take() {
            let _ = tx.try_send(());
        }
        if let Some(handle) = state.rtp_receiver_handle.take() {
            handle.abort();
        }
    }
}

pub async fn serve(
    config: Arc<RwLock<Config>>,
    listener: TcpListener,
    shutdown_signal: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let mode = config.read().unwrap().mode.clone().unwrap_or_default();    // Main run loop based on mode
    let result = match mode {
        Mode::Whep => serve_whep(config, listener, shutdown_signal).await,
        Mode::Whip => {
            error!("WHIP mode is no longer supported - livesrc now outputs RTP to liveion");
            Err(anyhow::anyhow!("WHIP mode removed, please use WHEP mode for testing"))
        }
    };
    result
}

async fn serve_whep(
    config: Arc<RwLock<Config>>,
    listener: TcpListener,
    shutdown_signal: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    // Create WebRTC API
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

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

    let manager = LiveSrcManager::new(config.clone(), webrtc_api);

    let app = Router::new()
        .merge(whep_handler::create_router())
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(vec![header::CONTENT_TYPE, header::AUTHORIZATION])
                .expose_headers(vec![header::LOCATION, header::CONTENT_TYPE]),
        )
        .with_state(manager.clone());

    let addr = listener.local_addr()?;
    info!("livesrc WHEP server listening on http://{}", addr);
    info!("WHEP endpoint: http://{}/whep", addr);

    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal)
        .await?;

    manager.shutdown().await;
    info!("livesrc shutdown completed");

    Ok(())
}

/*
// WHIP mode removed - livesrc now outputs RTP to liveion, not WHIP
// This function is preserved for reference but should not be used
pub async fn run_whip_mode(config: Arc<RwLock<config::Config>>, shutdown_signal: impl Future<Output = ()>) -> Result<()> {
    let cfg = config.read().unwrap();
    let stream = cfg.stream.as_ref().expect("stream config required");
    let whip_config = cfg.whip.as_ref().expect("WHIP config required");
    let camera = cfg.camera.as_ref().expect("camera config required");
    
    info!("Starting in WHIP mode");
    info!("  Stream ID: {}", stream.id);
    info!("  RTP Port: {}", stream.rtp_port);
    info!("  WHIP URL: {}", whip_config.url);
    info!("  Camera: {}", camera.device);
    
    // Build FFmpeg command
    let ffmpeg_command = match camera.source {
        config::CaptureSource::Ffmpeg => {
            camera.command.clone()
                .ok_or_else(|| anyhow::anyhow!("FFmpeg command required"))?
        }
        config::CaptureSource::V4l2 => {
            return Err(anyhow::anyhow!("V4L2 mode with WHIP not yet supported. Use FFmpeg mode."));
        }
    };
    
    let rtp_port = stream.rtp_port;
    let whip_url = whip_config.url.clone();
    let token = whip_config.token.clone();
    let codec_config = camera.codec.clone();
    drop(cfg);
    
    // Create shutdown channel for WHIP client
    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    
    // Forward the shutdown signal
    tokio::spawn(async move {
        shutdown_signal.await;
        let _ = shutdown_tx.send(()).await;
    });
    
    // NOTE: whip_client module has been removed
    warn!("WHIP mode is no longer supported - livesrc outputs RTP to liveion instead");
    Err(anyhow::anyhow!("WHIP mode removed, please use WHEP mode for testing or integrate with liveion"))
}
*/
