use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use playwright_whep::{Browser, HarnessResult, WhepBrowserPlayer};

use super::{PlayResult, Player};

/// WHEP player that uses Playwright to drive a real browser.
#[derive(Debug, Clone, Copy)]
pub struct PlaywrightWhepPlayer {
    pub timeout_seconds: u64,
    pub headless: bool,
    pub browser: Browser,
}

impl PlaywrightWhepPlayer {
    #[allow(dead_code)]
    pub fn webkit() -> Self {
        Self {
            browser: Browser::Webkit,
            ..Default::default()
        }
    }
}

impl Default for PlaywrightWhepPlayer {
    fn default() -> Self {
        Self {
            timeout_seconds: 30,
            headless: true,
            browser: Browser::default(),
        }
    }
}

#[async_trait]
impl Player for PlaywrightWhepPlayer {
    fn name(&self) -> &'static str {
        match self.browser {
            Browser::Chromium => "playwright-chromium",
            Browser::Firefox => "playwright-firefox",
            Browser::Webkit => "playwright-webkit",
        }
    }

    async fn play(&self, whep_url: &str) -> Result<PlayResult> {
        let result = WhepBrowserPlayer::new(whep_url)
            .browser(self.browser)
            .timeout(Duration::from_secs(self.timeout_seconds))
            .headless(self.headless)
            .play()
            .await?;

        let subscribe = match result {
            HarnessResult::Subscribe(r) => r,
            HarnessResult::Both(r) => r.subscribe.context("missing subscribe result")?,
            HarnessResult::Publish(_) => {
                return Err(anyhow::anyhow!("expected subscribe result, got publish"));
            }
        };

        Ok(PlayResult {
            success: subscribe.success,
            connected: subscribe.connected,
            video_width: subscribe.video_width,
            video_height: subscribe.video_height,
            video_tracks: subscribe.video_tracks as u32,
            audio_tracks: subscribe.audio_tracks as u32,
            duration_ms: subscribe.duration_ms,
            error: subscribe.error,
        })
    }
}
