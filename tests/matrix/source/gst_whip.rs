use std::{net::SocketAddr, process::Command};

use anyhow::{Context, Result};

use super::{ProcessHandle, Source, SourceHandle};
use crate::profile::MediaProfile;

/// Synthetic WHIP source implemented with GStreamer's `whipsink` element
/// (gst-plugins-rs webrtchttp) pushing directly to the WHIP endpoint.
///
/// Video-only profiles for now: the whipsink request-pad wiring for
/// audio+video is left to a follow-up.
#[derive(Debug, Clone, Copy)]
pub struct GstWhipSource {
    pub profile: MediaProfile,
}

impl GstWhipSource {
    pub fn new(profile: MediaProfile) -> Self {
        Self { profile }
    }

    /// Elements required by this source, for [`crate::runner::require_gst`].
    pub fn required_elements(&self) -> Vec<&'static str> {
        let mut elements = vec!["videotestsrc", "whipsink", "udpsink"];
        if let Some(video) = self.profile.video {
            elements.push(super::gst_rtp::gst_video_encoder(video.codec).0);
            elements.push(super::gst_rtp::gst_payloader(video.codec));
        }
        elements
    }
}

impl Source for GstWhipSource {
    fn name(&self) -> String {
        format!("gst-whip-{}", self.profile.name())
    }

    fn profile(&self) -> MediaProfile {
        self.profile
    }

    fn start_with_audio(
        &self,
        _video_addr: Option<SocketAddr>,
        _audio_addr: Option<SocketAddr>,
    ) -> Result<Box<dyn SourceHandle>> {
        anyhow::bail!("GstWhipSource uses direct WHIP publishing; call start_direct")
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
            anyhow::bail!("GstWhipSource requires a video track in its media profile");
        };
        if self.profile.audio.is_some() {
            anyhow::bail!("GstWhipSource is video-only for now");
        }

        let chain = super::gst_rtp::video_chain_sink(
            video.codec,
            video.width,
            video.height,
            video.fps,
            &format!("whipsink whip-endpoint={whip_url}"),
        );
        let child = Command::new("gst-launch-1.0")
            .arg("-q")
            .args(chain.split_whitespace())
            .spawn()
            .with_context(|| format!("Failed to spawn gst-launch-1.0: {chain}"))?;
        Ok(Box::new(ProcessHandle::new(child)))
    }
}
