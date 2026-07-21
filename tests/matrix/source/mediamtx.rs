//! mediamtx interop infrastructure (live777#212): a spawned mediamtx
//! instance plus the RTSP pull source built on top of it.
//!
//! mediamtx is the second third-party RTSP server the livetwo RTSP client is
//! tested against (the first is gst-rtsp-server, see `gst_rtsp_server.rs`).

use std::{
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    process::{Child, Command},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use super::{Source, SourceHandle, rtsp_ffmpeg::RtspFfmpegSource};
use crate::profile::MediaProfile;
use crate::runner::RtspTransport;

/// Resolve the mediamtx binary: `$MEDIAMTX_BIN` first, then the
/// `just mediamtx` download location, then PATH.
fn mediamtx_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MEDIAMTX_BIN") {
        return Some(PathBuf::from(path));
    }
    // EXE_SUFFIX is ".exe" on Windows and "" elsewhere.
    let local = PathBuf::from(format!("target/mediamtx{}", std::env::consts::EXE_SUFFIX));
    if local.exists() {
        return Some(local);
    }
    Command::new("mediamtx")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| PathBuf::from("mediamtx"))
}

/// Whether a usable mediamtx binary exists on this host. mediamtx matrix
/// cases skip themselves when this returns false.
pub fn available() -> bool {
    mediamtx_binary().is_some()
}

/// A spawned mediamtx instance with a minimal generated config: RTSP and the
/// control API only, every other protocol disabled. Kills the process on
/// drop so panicking tests cannot leak it.
pub struct MediamtxServer {
    child: Option<Child>,
    // Kept alive for the lifetime of the process: mediamtx reads the config
    // file at startup and re-reads it on SIGHUP.
    _config: tempfile::NamedTempFile,
    pub rtsp_addr: SocketAddr,
    api_addr: SocketAddr,
}

impl MediamtxServer {
    /// Spawn mediamtx.
    ///
    /// Every instance gets a unique even-aligned RTP/RTCP UDP port pair:
    /// mediamtx binds these shared listeners at startup even for TCP-only
    /// operation, so leaving them at the compiled-in defaults (:8000/:8001)
    /// would make concurrent instances fail with "address already in use".
    pub fn spawn() -> Result<Self> {
        let binary = mediamtx_binary().context(
            "mediamtx binary not found (run `just mediamtx`, install it into PATH, or set MEDIAMTX_BIN)",
        )?;

        let rtsp_addr = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            crate::runner::reserve_and_release_tcp_port(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        );
        let api_addr = SocketAddr::new(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            crate::runner::reserve_and_release_tcp_port(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        );

        let mut config = format!(
            "logLevel: warn\n\
             rtspAddress: {rtsp_addr}\n\
             api: yes\n\
             apiAddress: {api_addr}\n\
             rtmp: no\n\
             hls: no\n\
             webrtc: no\n\
             srt: no\n\
             moq: no\n\
             metrics: no\n\
             paths:\n  all:\n"
        );
        // mediamtx serves all UDP RTP/RTCP through one shared port pair and
        // binds it at startup regardless of whether any stream uses UDP, so
        // every instance needs its own pair; the RTP port must be even
        // (RTCP = RTP+1).
        let base = crate::runner::alloc_udp_ports(IpAddr::V4(Ipv4Addr::LOCALHOST), 3);
        let base = if base.is_multiple_of(2) {
            base
        } else {
            base + 1
        };
        config.push_str(&format!(
            "rtpAddress: 127.0.0.1:{base}\nrtcpAddress: 127.0.0.1:{}\n",
            base + 1
        ));

        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(config.as_bytes())?;

        let mut child = Command::new(binary)
            .arg(file.path())
            .spawn()
            .context("Failed to spawn mediamtx")?;

        // ffmpeg does not retry a refused RTSP TCP connect, so wait until the
        // listener is actually up before letting publishers at the URL.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match std::net::TcpStream::connect_timeout(&rtsp_addr, Duration::from_millis(100)) {
                Ok(_) => break,
                Err(e) if std::time::Instant::now() >= deadline => {
                    // Kill and reap the child: dropping it here would leak a
                    // running mediamtx (std Child does not kill on drop).
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("mediamtx did not listen on {rtsp_addr} within 5s: {e}")
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }

        Ok(Self {
            child: Some(child),
            _config: file,
            rtsp_addr,
            api_addr,
        })
    }

    /// Full RTSP URL for `path` (e.g. `/mt`) with the livetwo transport
    /// query parameter applied.
    pub fn rtsp_url(&self, path: &str, transport: RtspTransport) -> String {
        format!(
            "rtsp://{}{}{}",
            self.rtsp_addr,
            path,
            transport.query_param()
        )
    }

    /// Wait until the path has a ready publisher (thin wrapper over the
    /// free [`wait_path_ready`] for callers that own a server).
    pub async fn wait_path_ready(
        &self,
        path: &str,
        ct: &CancellationToken,
        handle: Option<&mut tokio::task::JoinHandle<Result<()>>>,
    ) {
        wait_path_ready(self.api_addr, path, ct, handle).await
    }

    pub async fn stop(mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = tokio::task::spawn_blocking(move || child.wait()).await;
        }
    }
}

impl Drop for MediamtxServer {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
    }
}

/// RTSP source: ffmpeg pushes into a mediamtx instance, livetwo's RTSP
/// client pulls from mediamtx and publishes into liveion via WHIP.
///
/// Covers the mediamtx-as-RTSP-server pull path; the push side is covered by
/// [`crate::runner::run_rtsp_push_mediamtx`].
#[derive(Debug, Clone, Copy)]
pub struct MediamtxPullSource {
    pub profile: MediaProfile,
    transport: Option<RtspTransport>,
}

impl MediamtxPullSource {
    pub fn new(profile: MediaProfile) -> Self {
        Self {
            profile,
            transport: None,
        }
    }

    /// Set the transport of the livetwo pull; defaults to TCP, matching the
    /// gst-rtsp-server source.
    pub fn with_transport(mut self, transport: RtspTransport) -> Self {
        self.transport = Some(transport);
        self
    }
}

impl Source for MediamtxPullSource {
    fn name(&self) -> String {
        format!("mediamtx-pull-{}", self.profile.name())
    }

    fn profile(&self) -> MediaProfile {
        self.profile
    }

    fn start_with_audio(
        &self,
        _video_addr: Option<SocketAddr>,
        _audio_addr: Option<SocketAddr>,
    ) -> Result<Box<dyn SourceHandle>> {
        anyhow::bail!("MediamtxPullSource publishes via livetwo RTSP pull; call start_direct")
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
        let transport = self.transport.unwrap_or(RtspTransport::Tcp);
        let server = MediamtxServer::spawn()?;

        // ffmpeg pushes into mediamtx (always TCP: the publisher transport is
        // not the interop surface under test, the livetwo pull is).
        let push_url = format!("rtsp://{}/mt", server.rtsp_addr);
        let push_handle = RtspFfmpegSource::new(self.profile).start_rtsp(&push_url)?;

        let ct = CancellationToken::new();
        let whip_ct = ct.clone();
        let rtsp_url = server.rtsp_url("/mt", transport);
        let api_addr = server.api_addr;
        let whip_url = whip_url.to_string();
        let whip_handle = tokio::spawn(async move {
            wait_path_ready(api_addr, "mt", &whip_ct, None).await;
            livetwo::whip::into(whip_ct, rtsp_url, whip_url, None, None).await
        });

        Ok(Box::new(MediamtxPullHandle {
            server: Some(server),
            push_handle: Some(push_handle),
            ct,
            whip_handle: Some(whip_handle),
        }))
    }
}

/// Poll the mediamtx API until the path has a ready publisher. A reader
/// that connects before the publisher gets a 404 from mediamtx, so pulls
/// must wait for this first. `handle` is an optional task to watch for an
/// early exit (same diagnostics style as `wait_stream_publish_ready`).
/// Returns early when `ct` is cancelled (teardown during teardown must not
/// block on this poll).
async fn wait_path_ready(
    api_addr: SocketAddr,
    path: &str,
    ct: &CancellationToken,
    mut handle: Option<&mut tokio::task::JoinHandle<Result<()>>>,
) {
    // 300 × 100 ms = 30 s, matching wait_for_publish_connected: 10 s was
    // not enough for ffmpeg to finish ANNOUNCE/RECORD when the host is
    // loaded (e.g. the whole matrix running in parallel).
    for attempt in 0..300 {
        if ct.is_cancelled() {
            return;
        }

        if let Some(h) = handle.as_mut()
            && h.is_finished()
        {
            let result = h.await.unwrap();
            panic!("task exited before mediamtx path '{path}' became ready: {result:?}");
        }

        if let Ok(res) = reqwest::get(format!("http://{api_addr}/v3/paths/list")).await
            && let Ok(body) = res.json::<serde_json::Value>().await
            && body["items"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|p| p["name"] == path && p["ready"] == true)
            })
        {
            return;
        }

        if attempt == 299 {
            panic!("mediamtx path '{path}' did not become ready");
        }
        tokio::select! {
            () = ct.cancelled() => return,
            () = tokio::time::sleep(Duration::from_millis(100)) => {}
        }
    }
}

struct MediamtxPullHandle {
    server: Option<MediamtxServer>,
    push_handle: Option<Box<dyn SourceHandle>>,
    ct: CancellationToken,
    whip_handle: Option<tokio::task::JoinHandle<Result<()>>>,
}

#[async_trait::async_trait]
impl SourceHandle for MediamtxPullHandle {
    async fn stop(mut self: Box<Self>) {
        self.ct.cancel();
        if let Some(push) = self.push_handle.take() {
            push.stop().await;
        }
        if let Some(server) = self.server.take() {
            server.stop().await;
        }
        if let Some(handle) = self.whip_handle.take() {
            let _ = handle.await;
        }
    }

    fn publish_task_mut(&mut self) -> Option<&mut tokio::task::JoinHandle<Result<()>>> {
        self.whip_handle.as_mut()
    }
}
