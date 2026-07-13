pub mod livetwo;
#[cfg(feature = "whepwright")]
pub mod playwright;
#[cfg(feature = "rsmpeg")]
pub mod rsmpeg_receiver;

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
}

#[async_trait::async_trait]
pub trait Player: Send + Sync {
    fn name(&self) -> &'static str;
    async fn play(&self, whep_url: &str) -> anyhow::Result<PlayResult>;
}
