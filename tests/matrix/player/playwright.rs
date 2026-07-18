use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use playwright_whep::{Browser, HarnessResult, WhepBrowserPlayer};

use super::{PlayResult, Player};
use crate::profile::MediaProfile;

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

    #[allow(dead_code)]
    pub fn firefox() -> Self {
        Self {
            browser: Browser::Firefox,
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

    async fn play(&self, whep_url: &str, profile: &MediaProfile) -> Result<PlayResult> {
        // The browser resolves playback as soon as video frames render.
        // Audio RTP can start flowing slightly after video, so on a fast
        // render the stats snapshot may still show zero audio bytes. Give
        // audio profiles one fresh retry: by then the publish side has been
        // streaming for seconds and audio is flowing from the first frame.
        for attempt in 0..2 {
            let result = self.play_once(whep_url).await?;

            let audio_missing =
                profile.audio.is_some() && result.connected && result.audio_tracks == 0;
            if audio_missing && attempt == 0 {
                tracing::warn!(
                    "browser reported no audio bytes for an audio profile; retrying playback"
                );
                continue;
            }
            return Ok(result);
        }
        unreachable!()
    }
}

impl PlaywrightWhepPlayer {
    async fn play_once(&self, whep_url: &str) -> Result<PlayResult> {
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
            // The browser offer always includes an audio m-line, so the
            // negotiated transceiver count reports phantom audio on
            // video-only streams. Count audio only when media bytes
            // actually flowed.
            audio_tracks: if subscribe.audio_bytes_received > 0 {
                subscribe.audio_tracks as u32
            } else {
                0
            },
            duration_ms: subscribe.duration_ms,
            error: subscribe.error,
            ..Default::default()
        })
    }
}
