use std::collections::HashMap;
use std::sync::{Arc, RwLock, Mutex};
use anyhow::Result;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::api::API;
use crate::config::{Config, PathConfig, SourceType};
use crate::sources::{Source, LibcameraSource, RpicamSource, V4l2Source, WhipSource, RtspSource, PublisherSource};

/// Represents the runtime state of a single path.
struct PathEntry {
    /// The source handler (ffmpeg, libcamera, etc.)
    source: Box<dyn Source + Send + Sync>,
    /// RTP track that will be offered to WebRTC peers.
    track: Arc<TrackLocalStaticRTP>,
    /// Number of current subscribers.
    subscriber_count: usize,
    /// Whether the source is currently running.
    running: bool,
    /// On‑demand flag from configuration.
    source_on_demand: bool,
    /// Maximum allowed subscribers (0 = unlimited).
    max_readers: usize,
}

/// Manages all configured paths.
pub struct PathManager {
    /// Mapping from path name to its runtime entry (protected by Mutex for interior mutability).
    paths: Arc<Mutex<HashMap<String, PathEntry>>>,
    /// Shared configuration (used for on‑demand start).
    config: Arc<RwLock<Config>>,
    /// WebRTC API (passed to source implementations if needed).
    webrtc_api: Arc<API>,
}

impl PathManager {
    /// Build a new manager from the global configuration.
    pub fn new(config: Arc<RwLock<Config>>, webrtc_api: Arc<API>) -> Self {
        let cfg = config.read().unwrap();
        let mut paths = HashMap::new();
        // Apply defaults from `path_defaults` when a field is missing.
        for (name, path_cfg) in &cfg.paths {
            // Merge with defaults.
            let merged = Self::merge_with_defaults(&cfg.path_defaults, path_cfg);
            
            // Create RTP track using the codec configuration.
            let track = Arc::new(TrackLocalStaticRTP::new(
                merged.codec.clone().into(),
                name.clone(),
                "livesrc-stream".to_owned(),
            ));
            
            // Choose source implementation with proper parameters
            let source: Box<dyn Source + Send + Sync> = match &merged.source {
                SourceType::Libcamera => {
                    let libcamera_config = merged.libcamera.clone()
                        .expect("libcamera config required for Libcamera source");
                    let rtp_port = merged.rtp_port
                        .expect("rtp_port required for Libcamera source");
                    Box::new(LibcameraSource::new(libcamera_config, rtp_port, merged.rtp_dest.clone(), track.clone()))
                }
                SourceType::Rpicam => {
                    let rpicam_config = merged.libcamera.clone()
                        .expect("libcamera config required for Rpicam source (reuses same config structure)");
                    let rtp_port = merged.rtp_port
                        .expect("rtp_port required for Rpicam source");
                    Box::new(RpicamSource::new(rpicam_config, rtp_port, merged.rtp_dest.clone(), track.clone()))
                }
                SourceType::V4l2 => Box::new(V4l2Source),
                SourceType::Whip => Box::new(WhipSource),
                SourceType::Rtsp(_) => Box::new(RtspSource),
                SourceType::Publisher => Box::new(PublisherSource),
                SourceType::File(_) => Box::new(PublisherSource), // treat file as publisher for now
            };
            
            paths.insert(
                name.clone(),
                PathEntry {
                    source,
                    track,
                    subscriber_count: 0,
                    running: false,
                    source_on_demand: merged.source_on_demand,
                    max_readers: merged.max_readers,
                },
            );
        }
        drop(cfg);
        
        // Auto-start sources that are not on-demand
        for (name, entry) in paths.iter_mut() {
            if !entry.source_on_demand && !entry.running {
                tracing::info!(path = %name, "Auto-starting source (source_on_demand = false)");
                match entry.source.start() {
                    Ok(_) => {
                        entry.running = true;
                        tracing::info!(path = %name, "Source auto-started successfully");
                    }
                    Err(e) => {
                        tracing::error!(path = %name, error = %e, "Failed to auto-start source");
                    }
                }
            }
        }
        
        Self { 
            paths: Arc::new(Mutex::new(paths)),
            config,
            webrtc_api,
        }
    }

    /// Merge a specific path config with the global defaults.
    fn merge_with_defaults(defaults: &PathConfig, specific: &PathConfig) -> PathConfig {
        PathConfig {
            source: specific.source.clone(),
            source_on_demand: specific.source_on_demand,
            max_readers: specific.max_readers,
            rtp_port: specific.rtp_port.or(defaults.rtp_port),
            rtp_dest: specific.rtp_dest.clone().or_else(|| defaults.rtp_dest.clone()),
            codec: specific.codec.clone(),
            libcamera: specific.libcamera.clone().or_else(|| defaults.libcamera.clone()),
            v4l2: specific.v4l2.clone().or_else(|| defaults.v4l2.clone()),
            whip: specific.whip.clone().or_else(|| defaults.whip.clone()),
            rtsp: specific.rtsp.clone().or_else(|| defaults.rtsp.clone()),
        }
    }

    /// Add a subscriber to a given path. Starts the source on‑demand if required.
    pub fn add_subscriber(&self, path_name: &str) -> Result<Option<Arc<TrackLocalStaticRTP>>> {
        tracing::debug!(path = %path_name, "add_subscriber called");
        
        let mut paths = self.paths.lock().unwrap();
        let entry = paths.get_mut(path_name)
            .ok_or_else(|| anyhow::anyhow!("Path '{}' not found", path_name))?;
        
        entry.subscriber_count += 1;
        tracing::debug!(path = %path_name, count = entry.subscriber_count, "subscriber_count incremented");
        
        // Enforce max_readers if set (>0).
        if entry.max_readers > 0 && entry.subscriber_count > entry.max_readers {
            entry.subscriber_count -= 1; // 回滚计数
            tracing::warn!(path = %path_name, limit = entry.max_readers, "max_readers limit exceeded");
            anyhow::bail!("Path '{}' exceeded max_readers limit ({})", path_name, entry.max_readers);
        }
        
        // Start source on first subscriber when on‑demand.
        if entry.subscriber_count == 1 && entry.source_on_demand {
            tracing::info!(path = %path_name, "Starting source (on-demand, first subscriber)");
            entry.source.start()?;
            entry.running = true;
            tracing::info!(path = %path_name, "Source started successfully");
        }
        
        Ok(Some(entry.track.clone()))
    }

    /// Remove a subscriber from a given path. Stops the source if on‑demand and no more subscribers.
    pub fn remove_subscriber(&self, path_name: &str) -> Result<()> {
        let mut paths = self.paths.lock().unwrap();
        let entry = paths.get_mut(path_name)
            .ok_or_else(|| anyhow::anyhow!("Path '{}' not found", path_name))?;
        
        if entry.subscriber_count == 0 {
            anyhow::bail!("Path '{}' has no subscribers to remove", path_name);
        }
        
        entry.subscriber_count -= 1;
        
        // Stop source when last subscriber leaves (if on‑demand)
        if entry.subscriber_count == 0 && entry.source_on_demand {
            entry.stop()?;
        }
        
        Ok(())
    }
}

// Helper methods for PathEntry
impl PathEntry {
    /// Stop the source if it is running.
    fn stop(&mut self) -> Result<()> {
        if self.running {
            self.source.stop()?;
            self.running = false;
        }
        Ok(())
    }
}
