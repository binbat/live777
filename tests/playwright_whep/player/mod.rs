pub mod livetwo;
#[cfg(feature = "whepwright")]
pub mod playwright;

#[derive(Debug, Default)]
#[allow(dead_code)]
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
