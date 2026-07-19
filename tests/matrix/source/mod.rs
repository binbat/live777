pub mod ffmpeg;
pub mod gst_rtp;
#[cfg(feature = "rtsp")]
pub mod gst_rtsp_server;
pub mod gst_whip;
#[cfg(feature = "rtsp")]
pub mod rtsp_ffmpeg;
#[cfg(feature = "rsmpeg")]
pub mod synth;

use std::net::SocketAddr;
use std::process::Child;
use std::time::Duration;

use crate::profile::MediaProfile;

#[async_trait::async_trait]
pub trait SourceHandle: Send {
    async fn stop(self: Box<Self>);
}

/// A source backed by a spawned child process (ffmpeg, gst-launch, ...).
/// Kills the process on drop so panicking tests cannot leak it.
pub struct ProcessHandle {
    child: Option<Child>,
}

impl ProcessHandle {
    pub fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
    }
}

#[async_trait::async_trait]
impl SourceHandle for ProcessHandle {
    async fn stop(mut self: Box<Self>) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = tokio::task::spawn_blocking(move || child.wait()).await;
        }
    }
}

pub trait Source: Send + Sync {
    fn name(&self) -> String;

    /// The media profile this source produces. The runner uses it to allocate
    /// ports, build the input SDP and validate playback.
    fn profile(&self) -> MediaProfile;

    /// Start the source with separate video and optional audio destinations.
    ///
    /// This is the only start entry point: sources that cannot emit an audio
    /// track must fail here when `audio_addr` is `Some`, so a multi-track
    /// profile can never silently degrade to video-only.
    fn start_with_audio(
        &self,
        video_addr: Option<SocketAddr>,
        audio_addr: Option<SocketAddr>,
    ) -> anyhow::Result<Box<dyn SourceHandle>>;

    /// Build an SDP with separate video and optional audio ports.
    fn sdp_with_audio(
        &self,
        video_addr: Option<SocketAddr>,
        audio_addr: Option<SocketAddr>,
    ) -> String;

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

    /// Start pushing to the liveion RTSP server with an explicit transport.
    /// Defaults to [`Self::start_rtsp`] (transport-agnostic sources).
    #[cfg(feature = "rtsp")]
    fn start_rtsp_with_transport(
        &self,
        rtsp_url: &str,
        _transport: crate::runner::RtspTransport,
    ) -> anyhow::Result<Box<dyn SourceHandle>> {
        self.start_rtsp(rtsp_url)
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
