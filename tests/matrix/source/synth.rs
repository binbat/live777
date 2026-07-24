use std::{net::SocketAddr, time::Duration};

use anyhow::Result;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{Source, SourceHandle};
use crate::profile::{MediaProfile, VideoCodec};

/// Synthetic WHIP source implemented with `livetwo::whipsynth`, the same
/// publisher used by the `livetwo synth` / `whipsynth` CLI.
#[derive(Debug, Clone, Copy)]
pub struct SynthSource {
    pub profile: MediaProfile,
}

impl SynthSource {
    pub fn new(profile: MediaProfile) -> Self {
        Self { profile }
    }

    /// Return H265 sprop parameters for this source's resolution and frame
    /// rate, if applicable.
    pub fn sprop_params(&self) -> Option<String> {
        let video = self.profile.video?;
        if video.codec != VideoCodec::H265 {
            return None;
        }
        livetwo::source::extract_h265_sprop(video.width, video.height, video.fps)
    }
}

impl Source for SynthSource {
    fn name(&self) -> String {
        format!("synth-{}", self.profile.name())
    }

    fn profile(&self) -> MediaProfile {
        self.profile
    }

    fn start_with_audio(
        &self,
        _video_addr: Option<SocketAddr>,
        _audio_addr: Option<SocketAddr>,
    ) -> Result<Box<dyn SourceHandle>> {
        anyhow::bail!("SynthSource uses direct WHIP publishing; call start_direct")
    }

    fn sdp_with_audio(
        &self,
        _video_addr: Option<SocketAddr>,
        _audio_addr: Option<SocketAddr>,
    ) -> String {
        String::new()
    }

    fn publishes_directly(&self) -> bool {
        true
    }

    fn start_direct(&self, whip_url: &str) -> Result<Box<dyn SourceHandle>> {
        let Some(video) = self.profile.video else {
            anyhow::bail!("SynthSource requires a video track in its media profile");
        };

        let ct = CancellationToken::new();
        let run_ct = ct.clone();

        let config = livetwo::whipsynth::PublisherConfig {
            whip_url: whip_url.to_owned(),
            token: None,
            video_codec: video.codec.to_livetwo(),
            audio_codec: self.profile.audio.map(|a| a.to_livetwo()),
            width: video.width,
            height: video.height,
            fps: video.fps,
            // Tests only need a few seconds of media; a longer duration wastes
            // time and keeps the publisher alive after teardown.
            duration: Some(Duration::from_secs(10)),
            // Loopback test: host candidates suffice, no ICE servers needed.
            ice_servers: Vec::new(),
        };

        let handle = tokio::spawn(async move {
            let publisher = livetwo::whipsynth::Publisher::new(config);
            if let Err(e) = publisher.run(run_ct).await {
                tracing::error!(error = ?e, "synth publisher error");
            }
        });

        Ok(Box::new(SynthHandle { ct, handle }))
    }
}

struct SynthHandle {
    ct: CancellationToken,
    handle: JoinHandle<()>,
}

#[async_trait::async_trait]
impl SourceHandle for SynthHandle {
    async fn stop(self: Box<Self>) {
        self.ct.cancel();
        if let Err(e) = self.handle.await {
            tracing::error!(error = ?e, "synth publisher task failed during stop");
        }
    }
}
