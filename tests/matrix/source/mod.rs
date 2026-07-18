pub mod ffmpeg;
pub mod gstreamer_vp8;
#[cfg(feature = "rtsp")]
pub mod rtsp_ffmpeg;
#[cfg(feature = "rsmpeg")]
pub mod synth;

use std::net::SocketAddr;
use std::time::Duration;

use crate::profile::MediaProfile;

#[async_trait::async_trait]
pub trait SourceHandle: Send {
    async fn stop(self: Box<Self>);
}

pub trait Source: Send + Sync {
    fn name(&self) -> String;

    /// The media profile this source produces. The runner uses it to allocate
    /// ports, build the input SDP and validate playback.
    fn profile(&self) -> MediaProfile;

    fn start(&self, target_addr: SocketAddr) -> anyhow::Result<Box<dyn SourceHandle>>;
    fn sdp(&self, listen_addr: SocketAddr) -> String;

    /// Start the source with separate video and optional audio destinations.
    ///
    /// Defaults to [`Self::start`] for video-only sources.
    fn start_with_audio(
        &self,
        video_addr: Option<SocketAddr>,
        _audio_addr: Option<SocketAddr>,
    ) -> anyhow::Result<Box<dyn SourceHandle>> {
        match video_addr {
            Some(addr) => self.start(addr),
            None => anyhow::bail!("audio-only profiles must override start_with_audio"),
        }
    }

    /// Build an SDP with separate video and optional audio ports.
    ///
    /// Defaults to [`Self::sdp`] for video-only sources.
    fn sdp_with_audio(
        &self,
        video_addr: Option<SocketAddr>,
        _audio_addr: Option<SocketAddr>,
    ) -> String {
        self.sdp(video_addr.expect("audio-only profiles must override sdp_with_audio"))
    }

    /// Whether this source publishes directly to a WHIP endpoint instead of
    /// emitting RTP to a local address.
    fn publishes_directly(&self) -> bool {
        false
    }

    /// Whether this source publishes via RTSP (ANNOUNCE + RECORD) instead of
    /// WHIP. RTSP sources receive the RTSP URL directly via
    /// [`Self::start_rtsp`].
    #[cfg(feature = "rtsp")]
    fn is_rtsp(&self) -> bool {
        false
    }

    /// Start pushing to the liveion RTSP server.
    ///
    /// Only called when [`Self::is_rtsp`] returns `true`.  `rtsp_url` is the
    /// full RTSP URL, e.g. `rtsp://127.0.0.1:8554/-`.
    #[cfg(feature = "rtsp")]
    fn start_rtsp(&self, rtsp_url: &str) -> anyhow::Result<Box<dyn SourceHandle>> {
        let _ = rtsp_url;
        anyhow::bail!("RTSP publishing not supported by this source")
    }

    /// Start direct WHIP publishing. Only called when [`Self::publishes_directly`]
    /// returns `true`.
    fn start_direct(&self, _whip_url: &str) -> anyhow::Result<Box<dyn SourceHandle>> {
        anyhow::bail!("direct publishing not supported by this source")
    }

    /// Wait until the source is producing frames and it is safe to subscribe.
    ///
    /// The default implementation sleeps briefly to give encoders time to emit
    /// the first keyframe. Individual sources may override this with a more
    /// deterministic readiness check.
    async fn wait_for_ready(&self) {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
