pub mod livetwo;
#[cfg(feature = "whepwright")]
pub mod playwright;
#[cfg(feature = "rsmpeg")]
pub mod rsmpeg_receiver;

use crate::profile::MediaProfile;

#[derive(Debug, Default)]
pub struct PlayResult {
    pub success: bool,
    pub connected: bool,
    pub error: Option<String>,
    pub video_width: u32,
    pub video_height: u32,
    pub video_tracks: u32,
    pub audio_tracks: u32,
    pub duration_ms: u64,
    /// Codec names reported by the player/validator for each received stream,
    /// e.g. `["vp8", "opus"]`. Empty when the player does not probe codecs.
    pub codecs: Vec<String>,
    /// Audio channel count of the first audio stream, when probed.
    pub audio_channels: u32,
}

#[async_trait::async_trait]
pub trait Player: Send + Sync {
    fn name(&self) -> &'static str;

    /// Play `whep_url` and validate the received media against `profile`.
    async fn play(&self, whep_url: &str, profile: &MediaProfile) -> anyhow::Result<PlayResult>;
}
