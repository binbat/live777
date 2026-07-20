use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use cli::Codec;
use serde::Serialize;

#[cfg(feature = "rsmpeg")]
pub mod decoder;
#[cfg(feature = "rsmpeg")]
pub mod rsmpeg;

/// Configuration for a WHEP probe session.
#[derive(Debug, Clone)]
pub struct ProbeConfig {
    /// WHEP endpoint URL, e.g. `http://localhost:7777/whep/live`.
    pub whep_url: String,
    /// Maximum time to wait for connection and decoding/playback.
    pub timeout: Duration,
    /// Expected video codec. Used to build the receiver SDP for UDP-based backends.
    /// The rsmpeg and playwright backends ignore this and use the codec negotiated
    /// in the WHEP session.
    pub codec: Option<Codec>,
    /// H265 sprop parameters (`sprop-vps=...;sprop-sps=...;sprop-pps=...`).
    /// Used by the rsmpeg backend to seed parameter-set injection for H265 streams.
    pub sprop_params: Option<String>,
    /// Optional Bearer token for WHIP/WHEP endpoints that require authentication.
    pub token: Option<String>,
}

impl Default for ProbeConfig {
    fn default() -> Self {
        Self {
            whep_url: String::new(),
            timeout: Duration::from_secs(30),
            codec: None,
            sprop_params: None,
            token: None,
        }
    }
}

/// Result of a WHEP probe attempt.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    /// Whether the whole probe succeeded (connected and observed valid media).
    pub success: bool,
    /// Whether the WebRTC peer connection reached the connected state.
    pub connected: bool,
    /// Backend that produced this result.
    pub backend: &'static str,
    /// Observed video codec, if any.
    pub codec: Option<String>,
    /// Negotiated audio codec, if any. Backends that do not decode audio
    /// can still report the codec negotiated for the audio track.
    pub audio_codec: Option<String>,
    /// Observed video width in pixels.
    pub width: u32,
    /// Observed video height in pixels.
    pub height: u32,
    /// Number of successfully decoded/observed video frames.
    pub frame_count: u32,
    /// Probe duration from start to result in milliseconds.
    pub duration_ms: u64,
    /// Number of received video tracks (for reporting backends).
    pub video_tracks: u32,
    /// Number of received audio tracks (for reporting backends).
    pub audio_tracks: u32,
    /// Bytes received on the video inbound RTP stream (browser backend).
    #[serde(default)]
    pub video_bytes_received: u64,
    /// Bytes received on the audio inbound RTP stream (browser backend).
    #[serde(default)]
    pub audio_bytes_received: u64,
    /// Error message when `success` is false.
    pub error: Option<String>,
}

impl ProbeResult {
    /// Create a failed result for a given backend.
    pub fn failed(backend: &'static str, error: impl Into<String>) -> Self {
        Self {
            success: false,
            connected: false,
            backend,
            codec: None,
            audio_codec: None,
            width: 0,
            height: 0,
            frame_count: 0,
            duration_ms: 0,
            video_tracks: 0,
            audio_tracks: 0,
            video_bytes_received: 0,
            audio_bytes_received: 0,
            error: Some(error.into()),
        }
    }
}

/// Backend that can probe a WHEP endpoint and report whether the stream is
/// reachable and decodable/playable.
#[async_trait]
pub trait ProbeBackend: Send + Sync {
    /// Backend identifier, used in reports.
    fn name(&self) -> &'static str;

    /// Probe the configured WHEP endpoint.
    async fn probe(&self, config: &ProbeConfig) -> Result<ProbeResult>;
}
