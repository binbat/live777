use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    process::{Child, Command},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use super::{Source, SourceHandle};
use crate::profile::MediaProfile;

/// RTSP source: a gst-rtsp-server instance hosting the stream, pulled by
/// livetwo's RTSP client (`whipinto`) and published into liveion via WHIP.
///
/// This covers the gst-as-RTSP-server ingest path; `rtspclientsink` is not
/// packaged on many distros, so the pull topology is used instead. Each
/// track of the profile is hosted as a separate `pay%d` stream, so
/// video-only, audio-only and A/V profiles are all supported.
#[derive(Debug, Clone, Copy)]
pub struct GstRtspServerSource {
    pub profile: MediaProfile,
}

impl GstRtspServerSource {
    pub fn new(profile: MediaProfile) -> Self {
        Self { profile }
    }

    /// Whether the C helper can be built on this host.
    pub fn available() -> bool {
        let pkg = Command::new("pkg-config")
            .args(["--exists", "gstreamer-rtsp-server-1.0"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let gcc = Command::new("gcc")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        pkg && gcc
    }

    /// GStreamer elements required by the hosted pipeline.
    pub fn required_elements(&self) -> Vec<&'static str> {
        let mut elements = Vec::new();
        if let Some(video) = self.profile.video {
            elements.push("videotestsrc");
            elements.push(video.codec.gst_encoder().0);
            elements.push(video.codec.gst_pay());
        }
        if let Some(audio) = self.profile.audio {
            elements.push("audiotestsrc");
            elements.push(audio.gst_encoder().0);
            elements.push(audio.gst_pay());
        }
        elements
    }
}

fn server_binary() -> Result<PathBuf> {
    let out = std::path::Path::new("target/test-rtsp-server");
    if out.exists() {
        return Ok(out.to_path_buf());
    }
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "gcc -O2 -o {} tools/test-rtsp-server.c $(pkg-config --cflags --libs gstreamer-1.0 gstreamer-rtsp-server-1.0)",
            out.display()
        ))
        .status()
        .context("failed to invoke gcc for test-rtsp-server")?;
    if !status.success() {
        anyhow::bail!("failed to build tools/test-rtsp-server.c");
    }
    Ok(out.to_path_buf())
}

impl Source for GstRtspServerSource {
    fn name(&self) -> String {
        format!("gst-rtsp-{}", self.profile.name())
    }

    fn profile(&self) -> MediaProfile {
        self.profile
    }

    fn start_with_audio(
        &self,
        _video_addr: Option<SocketAddr>,
        _audio_addr: Option<SocketAddr>,
    ) -> Result<Box<dyn SourceHandle>> {
        anyhow::bail!("GstRtspServerSource publishes via livetwo RTSP pull; call start_direct")
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
        let binary = server_binary()?;

        let port = crate::runner::reserve_and_release_tcp_port(IpAddr::V4(Ipv4Addr::LOCALHOST));

        // test-launch syntax: every element named pay%d becomes one RTSP
        // stream, so the profile's tracks map to consecutive pay indices.
        let mut chains = Vec::new();
        if let Some(video) = self.profile.video {
            let (encoder, encoder_args) = video.codec.gst_encoder();
            chains.push(format!(
                "videotestsrc is-live=true ! video/x-raw,width={},height={},framerate={}/1 ! {} {} ! {} name=pay{} pt={}",
                video.width,
                video.height,
                video.fps,
                encoder,
                encoder_args,
                video.codec.gst_pay(),
                chains.len(),
                video.codec.payload_type(),
            ));
        }
        if let Some(audio) = self.profile.audio {
            let (encoder, encoder_args) = audio.gst_encoder();
            chains.push(format!(
                "audiotestsrc is-live=true ! {} {} ! {} name=pay{} pt={}",
                encoder,
                encoder_args,
                audio.gst_pay(),
                chains.len(),
                audio.payload_type(),
            ));
        }
        if chains.is_empty() {
            anyhow::bail!("media profile has no tracks");
        }
        let pipeline = format!("( {} )", chains.join(" "));

        let child = Command::new(binary)
            .arg("-p")
            .arg(port.to_string())
            .arg("-m")
            .arg("/gst")
            .arg(&pipeline)
            .spawn()
            .with_context(|| format!("Failed to spawn test-rtsp-server: {pipeline}"))?;

        // Wait until the server actually accepts connections. A fixed sleep
        // here blocked the single-threaded test runtime and flaked on loaded
        // CI hosts; connecting back is a real readiness check and fails fast.
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(100)) {
                Ok(_) => break,
                Err(e) if std::time::Instant::now() >= deadline => {
                    anyhow::bail!("test-rtsp-server did not listen on {addr} within 5s: {e}")
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }

        let ct = CancellationToken::new();
        let whip_ct = ct.clone();
        let rtsp_url = format!("rtsp://127.0.0.1:{port}/gst?transport=tcp");
        let whip_url = whip_url.to_string();
        let whip_handle = tokio::spawn(async move {
            livetwo::whip::into(whip_ct, rtsp_url, whip_url, None, None).await
        });

        Ok(Box::new(GstRtspServerHandle {
            child: Some(child),
            ct,
            whip_handle: Some(whip_handle),
        }))
    }
}

struct GstRtspServerHandle {
    child: Option<Child>,
    ct: CancellationToken,
    whip_handle: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl Drop for GstRtspServerHandle {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
    }
}

#[async_trait::async_trait]
impl SourceHandle for GstRtspServerHandle {
    async fn stop(mut self: Box<Self>) {
        self.ct.cancel();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
        if let Some(handle) = self.whip_handle.take() {
            let _ = handle.await;
        }
    }
}
