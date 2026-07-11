use anyhow::Result;

use async_trait::async_trait;
use cli::Codec;
use livetwo::probe::ProbeResult;
#[cfg(feature = "rsmpeg")]
use std::time::Duration;

#[cfg(feature = "rsmpeg")]
use livetwo::probe::{ProbeBackend, ProbeConfig, rsmpeg::RsmpegProbe};

use super::{PlayResult, Player};

/// WHEP player that receives RTP via `livetwo::probe::rsmpeg::RsmpegProbe` and
/// decodes it with rsmpeg/FFmpeg.
///
/// This is the most self-contained baseline: both source and player are in-process
/// FFmpeg, so browser/ICE/container issues are excluded.
#[derive(Debug, Clone)]
pub struct RsmpegWhepReceiver {
    pub timeout_seconds: u64,
    /// Expected video codec. When `None` the probe defaults to VP8.
    pub codec: Option<Codec>,
    /// H265 sprop parameters (`sprop-vps=...;sprop-sps=...;sprop-pps=...`).
    pub sprop_params: Option<String>,
}

impl Default for RsmpegWhepReceiver {
    fn default() -> Self {
        Self {
            timeout_seconds: 10,
            codec: None,
            sprop_params: None,
        }
    }
}

impl RsmpegWhepReceiver {
    pub fn with_codec_and_sprop(codec: Codec, sprop_params: String) -> Self {
        Self {
            timeout_seconds: 10,
            codec: Some(codec),
            sprop_params: Some(sprop_params),
        }
    }
}

#[async_trait]
impl Player for RsmpegWhepReceiver {
    fn name(&self) -> &'static str {
        "rsmpeg-receiver"
    }

    #[cfg(feature = "rsmpeg")]
    async fn play(&self, whep_url: &str) -> Result<PlayResult> {
        let config = ProbeConfig {
            whep_url: whep_url.to_string(),
            timeout: Duration::from_secs(self.timeout_seconds),
            codec: self.codec,
            sprop_params: self.sprop_params.clone(),
            token: None,
        };

        let backend = RsmpegProbe {
            decode_duration: Duration::from_secs(self.timeout_seconds.min(10)),
        };

        let result = backend.probe(&config).await?;
        Ok(PlayResult::from(result))
    }

    #[cfg(not(feature = "rsmpeg"))]
    async fn play(&self, _whep_url: &str) -> Result<PlayResult> {
        anyhow::bail!("rsmpeg receiver requires the rsmpeg feature to be enabled")
    }
}

impl From<ProbeResult> for PlayResult {
    fn from(result: ProbeResult) -> Self {
        Self {
            success: result.success,
            connected: result.connected,
            video_width: result.width,
            video_height: result.height,
            video_tracks: result.video_tracks,
            audio_tracks: result.audio_tracks,
            duration_ms: result.duration_ms,
            error: result.error,
        }
    }
}
