//! Browser WebRTC test harness using Playwright.
//!
//! This crate drives a real browser (Chromium / Firefox / WebKit) with
//! [Playwright](https://playwright.dev), makes it perform a WHIP publish
//! and/or WHEP subscribe negotiation through a minimal bundled test page, and
//! reports whether the stream was successfully published and/or played.
//! It is intended for automated tests that need to verify browser WebRTC
//! behaviour end-to-end.
//!
//! # Example
//!
//! ```no_run
//! use std::time::Duration;
//! use playwright_whep::{WhepBrowserPlayer, Browser, Mode, PublishSource};
//!
//! # async fn example() -> anyhow::Result<()> {
//! use playwright_whep::HarnessResult;
//!
//! // Subscribe-only (original behaviour).
//! let result = WhepBrowserPlayer::new("http://localhost:7777/whep/live")
//!     .browser(Browser::Chromium)
//!     .timeout(Duration::from_secs(30))
//!     .headless(true)
//!     .play()
//!     .await?;
//!
//! if let HarnessResult::Subscribe(r) = result {
//!     assert!(r.success);
//!     assert!(r.connected);
//!     assert!(r.video_width > 0);
//! }
//! # Ok(())
//! # }
//! ```

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

/// Target browser for the browser WebRTC test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Browser {
    #[default]
    Chromium,
    Firefox,
    Webkit,
}

impl Browser {
    fn as_str(&self) -> &'static str {
        match self {
            Browser::Chromium => "chromium",
            Browser::Firefox => "firefox",
            Browser::Webkit => "webkit",
        }
    }
}

/// Which direction the browser should exercise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Only subscribe via WHEP (default, original behaviour).
    #[default]
    Subscribe,
    /// Only publish via WHIP.
    Publish,
    /// Publish and subscribe on the same page.
    Both,
}

impl Mode {
    fn as_str(&self) -> &'static str {
        match self {
            Mode::Subscribe => "subscribe",
            Mode::Publish => "publish",
            Mode::Both => "both",
        }
    }
}

/// Video source to use when publishing from the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PublishSource {
    /// Chromium's fake camera device (requires `--use-fake-device-for-media-stream`).
    #[default]
    FakeCamera,
    /// Canvas-generated animation with a synthetic audio tone. Used when real
    /// camera access is not available or not desired.
    Canvas,
}

impl PublishSource {
    fn as_str(&self) -> &'static str {
        match self {
            PublishSource::FakeCamera => "fake",
            PublishSource::Canvas => "canvas",
        }
    }
}

/// Result reported by the browser after attempting WHEP playback.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhepPlayResult {
    /// Whether the whole flow succeeded and video frames were rendered.
    pub success: bool,
    /// Whether the WebRTC peer connection reached the connected state.
    pub connected: bool,
    /// Rendered video width in pixels (0 if not available).
    #[serde(default)]
    pub video_width: u32,
    /// Rendered video height in pixels (0 if not available).
    #[serde(default)]
    pub video_height: u32,
    /// Number of received video tracks.
    #[serde(default)]
    pub video_tracks: usize,
    /// Number of received audio tracks.
    #[serde(default)]
    pub audio_tracks: usize,
    /// Playback duration from offer creation to frame render in milliseconds.
    #[serde(default)]
    pub duration_ms: u64,
    /// Number of inbound-rtp statistics reports observed.
    #[serde(default)]
    pub inbound_rtp_count: usize,
    /// Bytes received on the video inbound RTP stream.
    #[serde(default)]
    pub video_bytes_received: u64,
    /// Bytes received on the audio inbound RTP stream.
    #[serde(default)]
    pub audio_bytes_received: u64,
    /// Error message when `success` is false.
    pub error: Option<String>,
}

/// Result reported by the browser after attempting WHIP publishing.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhipPublishResult {
    /// Whether publishing completed successfully and the peer connection connected.
    pub success: bool,
    /// Whether the WebRTC peer connection reached the connected state.
    pub connected: bool,
    /// Number of sent audio tracks.
    #[serde(default)]
    pub audio_tracks: usize,
    /// Number of sent video tracks.
    #[serde(default)]
    pub video_tracks: usize,
    /// Negotiated audio codec, if any.
    #[serde(default)]
    pub audio_codec: String,
    /// Negotiated video codec, if any.
    #[serde(default)]
    pub video_codec: String,
    /// Publish duration in milliseconds.
    #[serde(default)]
    pub duration_ms: u64,
    /// Error message when `success` is false.
    pub error: Option<String>,
}

/// Combined result when running both publish and subscribe on the same page.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BothResult {
    /// Whether both publish and subscribe succeeded.
    pub success: bool,
    /// Publish half of the result.
    pub publish: Option<WhipPublishResult>,
    /// Subscribe half of the result.
    pub subscribe: Option<WhepPlayResult>,
    /// Error message when `success` is false.
    pub error: Option<String>,
}

/// Result reported by the debugger automation page.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HarnessResult {
    Both(BothResult),
    Subscribe(WhepPlayResult),
    Publish(WhipPublishResult),
}

/// Envelope returned by the Node.js Playwright runner.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerOutput<T> {
    /// Mode that was requested from the runner.
    pub mode: String,
    /// Typed result for that mode.
    pub result: T,
}

/// Builder for a browser-based WebRTC test harness.
#[derive(Debug, Clone)]
pub struct WhepBrowserPlayer {
    subscribe_url: Option<String>,
    publish_url: Option<String>,
    stream_id: Option<String>,
    mode: Mode,
    source: PublishSource,
    codec: Option<String>,
    audio_codec: Option<String>,
    layer: Option<String>,
    token: Option<String>,
    browser: Browser,
    channel: Option<String>,
    timeout: Duration,
    headless: bool,
    node_path: PathBuf,
    /// Directory used to resolve the Playwright module and as the runner's
    /// working directory. Defaults to the current working directory at the
    /// time `play()` is called.
    playwright_dir: Option<PathBuf>,
}

impl WhepBrowserPlayer {
    /// Create a new subscriber for the given WHEP endpoint URL.
    pub fn new(whep_url: impl Into<String>) -> Self {
        Self {
            subscribe_url: Some(whep_url.into()),
            publish_url: None,
            stream_id: None,
            mode: Mode::Subscribe,
            source: PublishSource::default(),
            codec: None,
            audio_codec: None,
            layer: None,
            token: None,
            browser: Browser::default(),
            channel: None,
            timeout: Duration::from_secs(30),
            headless: true,
            node_path: PathBuf::from("node"),
            playwright_dir: None,
        }
    }

    /// Create a new publisher for the given WHIP endpoint URL.
    pub fn publish(whip_url: impl Into<String>) -> Self {
        Self {
            subscribe_url: None,
            publish_url: Some(whip_url.into()),
            stream_id: None,
            mode: Mode::Publish,
            source: PublishSource::default(),
            codec: None,
            audio_codec: None,
            layer: None,
            token: None,
            browser: Browser::default(),
            channel: None,
            timeout: Duration::from_secs(30),
            headless: true,
            node_path: PathBuf::from("node"),
            playwright_dir: None,
        }
    }

    /// Create a combined publish+subscribe harness.
    ///
    /// The browser will publish to `whip_url` and subscribe to `whep_url`.
    /// Both endpoints must use the same stream id.
    pub fn both(whip_url: impl Into<String>, whep_url: impl Into<String>) -> Self {
        Self {
            subscribe_url: Some(whep_url.into()),
            publish_url: Some(whip_url.into()),
            stream_id: None,
            mode: Mode::Both,
            source: PublishSource::default(),
            codec: None,
            audio_codec: None,
            layer: None,
            token: None,
            browser: Browser::default(),
            channel: None,
            timeout: Duration::from_secs(30),
            headless: true,
            node_path: PathBuf::from("node"),
            playwright_dir: None,
        }
    }

    /// Set the stream id shared by publish and subscribe.
    pub fn stream_id(mut self, stream_id: impl Into<String>) -> Self {
        self.stream_id = Some(stream_id.into());
        self
    }

    /// Set the test mode.
    pub fn mode(mut self, mode: Mode) -> Self {
        self.mode = mode;
        self
    }

    /// Set the publish video source.
    pub fn source(mut self, source: PublishSource) -> Self {
        self.source = source;
        self
    }

    /// Set the target browser to use (default: Chromium).
    pub fn browser(mut self, browser: Browser) -> Self {
        self.browser = browser;
        self
    }

    /// Set the browser channel to use, e.g. `chrome` or `msedge`.
    ///
    /// Only meaningful when the browser is Chromium. Playwright will launch the
    /// installed Google Chrome / Microsoft Edge instead of the bundled Chromium.
    pub fn channel(mut self, channel: impl Into<String>) -> Self {
        let channel = channel.into();
        self.channel = if channel.is_empty() { None } else { Some(channel) };
        self
    }

    /// Set the maximum time to wait for the WebRTC flow (default: 30s).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Run the browser in headless mode (default: true).
    pub fn headless(mut self, headless: bool) -> Self {
        self.headless = headless;
        self
    }

    /// Set the preferred video codec for publishing or subscribing.
    pub fn codec(mut self, codec: impl Into<String>) -> Self {
        self.codec = Some(codec.into());
        self
    }

    /// Set the preferred audio codec for publishing.
    pub fn audio_codec(mut self, codec: impl Into<String>) -> Self {
        self.audio_codec = Some(codec.into());
        self
    }

    /// Set the simulcast/SVC layer for publishing.
    pub fn layer(mut self, layer: impl Into<String>) -> Self {
        self.layer = Some(layer.into());
        self
    }

    /// Set the Bearer token used for WHIP/WHEP authentication.
    pub fn token(mut self, token: impl Into<String>) -> Self {
        let token = token.into();
        self.token = if token.is_empty() { None } else { Some(token) };
        self
    }

    /// Path to the Node.js executable (default: `node`).
    pub fn node_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.node_path = path.into();
        self
    }

    /// Directory used to resolve the Playwright module and as the runner's
    /// working directory (default: current working directory at `play()` time).
    pub fn playwright_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.playwright_dir = Some(dir.into());
        self
    }

    /// Launch the browser, perform the requested WebRTC flow, and return the result.
    pub async fn play(self) -> Result<HarnessResult> {
        let playwright_dir = self
            .playwright_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

        let playwright_module = resolve_playwright_module(&self.node_path, &playwright_dir)
            .await
            .with_context(|| {
                format!(
                    "Failed to resolve Playwright module under {}. \
                     Make sure Playwright is installed (e.g. `pnpm add -D playwright`).",
                    playwright_dir.display()
                )
            })?;

        let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
        let assets_dir = temp_dir.path();

        write_asset(
            assets_dir,
            "run-playwright.mjs",
            include_str!("../static/run-playwright.mjs"),
        )
        .await?;
        write_asset(
            assets_dir,
            "player.html",
            include_str!("../static/player.html"),
        )
        .await?;

        let runner_path = assets_dir.join("run-playwright.mjs");

        let mut cmd = Command::new(&self.node_path);
        cmd.arg(&runner_path)
            .arg("--mode")
            .arg(self.mode.as_str())
            .arg("--browser")
            .arg(self.browser.as_str())
            .arg("--timeout")
            .arg(self.timeout.as_millis().to_string())
            .arg("--headless")
            .arg(if self.headless { "true" } else { "false" })
            .arg("--static-root")
            .arg(assets_dir)
            .arg("--source")
            .arg(self.source.as_str())
            .env("PLAYWRIGHT_MODULE_PATH", &playwright_module)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(&playwright_dir);

        if let Some(url) = &self.subscribe_url {
            cmd.arg("--whep-url").arg(url);
        }
        if let Some(url) = &self.publish_url {
            cmd.arg("--whip-url").arg(url);
        }
        if let Some(id) = &self.stream_id {
            cmd.arg("--stream-id").arg(id);
        }
        if let Some(codec) = &self.codec {
            cmd.arg("--vcodec").arg(codec);
        }
        if let Some(codec) = &self.audio_codec {
            cmd.arg("--acodec").arg(codec);
        }
        if let Some(layer) = &self.layer {
            cmd.arg("--layer").arg(layer);
        }
        if let Some(token) = &self.token {
            cmd.arg("--token").arg(token);
        }
        if let Some(channel) = &self.channel {
            cmd.arg("--channel").arg(channel);
        }

        debug!(?cmd, "Spawning Playwright runner");

        let mut child = cmd.spawn().context("Failed to spawn Playwright runner")?;
        let stdout = child
            .stdout
            .take()
            .expect("stdout was configured to be piped");
        let stderr = child
            .stderr
            .take()
            .expect("stderr was configured to be piped");

        let stdout_reader = BufReader::new(stdout);
        let stderr_reader = BufReader::new(stderr);

        let mut last_stdout_line = String::new();
        let mut stdout_lines = stdout_reader.lines();
        let mut stderr_lines = stderr_reader.lines();

        let child_id = child.id();

        let wait_fut = async {
            loop {
                tokio::select! {
                    line = stdout_lines.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                debug!(target: "playwright-whep::runner::stdout", "{line}");
                                if !line.trim().is_empty() {
                                    last_stdout_line = line;
                                }
                            }
                            Ok(None) => break,
                            Err(e) => return Err(anyhow!("Failed to read runner stdout: {e}")),
                        }
                    }
                    line = stderr_lines.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if !line.trim().is_empty() {
                                    warn!(target: "playwright-whep::runner::stderr", "{line}");
                                }
                            }
                            Ok(None) => {}
                            Err(e) => return Err(anyhow!("Failed to read runner stderr: {e}")),
                        }
                    }
                }
            }

            let status = child.wait().await.context("Failed to wait for runner")?;
            Ok::<_, anyhow::Error>(status)
        };

        let total_timeout = self.timeout + Duration::from_secs(15);
        let status = match timeout(total_timeout, wait_fut).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                graceful_kill_child(child_id, &mut child).await;
                return Err(anyhow!(
                    "Playwright runner timed out after {:?}",
                    total_timeout
                ));
            }
        };

        if !status.success() {
            return Err(anyhow!(
                "Playwright runner exited with status {status}. Last stdout line: {last_stdout_line}",
            ));
        }

        if last_stdout_line.trim().is_empty() {
            return Err(anyhow!(
                "Playwright runner produced no JSON output on stdout"
            ));
        }

        let result = match self.mode {
            Mode::Subscribe => {
                let out: RunnerOutput<WhepPlayResult> = serde_json::from_str(&last_stdout_line)
                    .with_context(|| {
                        format!("Failed to parse runner output: {last_stdout_line}")
                    })?;
                HarnessResult::Subscribe(out.result)
            }
            Mode::Publish => {
                let out: RunnerOutput<WhipPublishResult> = serde_json::from_str(&last_stdout_line)
                    .with_context(|| {
                        format!("Failed to parse runner output: {last_stdout_line}")
                    })?;
                HarnessResult::Publish(out.result)
            }
            Mode::Both => {
                let out: RunnerOutput<BothResult> = serde_json::from_str(&last_stdout_line)
                    .with_context(|| {
                        format!("Failed to parse runner output: {last_stdout_line}")
                    })?;
                HarnessResult::Both(out.result)
            }
        };

        Ok(result)
    }
}

async fn write_asset(dir: &Path, name: &str, content: &str) -> Result<()> {
    let path = dir.join(name);
    tokio::fs::write(&path, content)
        .await
        .with_context(|| format!("Failed to write asset {}", path.display()))?;
    Ok(())
}

/// Attempt to terminate the runner gracefully and fall back to a force kill
/// if it does not exit in time. On Unix we send SIGTERM first so the runner's
/// `finally` block has a chance to close the browser; on other platforms we
/// kill the process directly.
async fn graceful_kill_child(child_id: Option<u32>, child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child_id {
        unsafe {
            let _ = libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        // Give the runner a few seconds to shut down the browser cleanly.
        match timeout(Duration::from_secs(5), child.wait()).await {
            Ok(Ok(_)) => return,
            _ => {}
        }
    }
    let _ = child.start_kill();
}

async fn resolve_playwright_module(node_path: &Path, dir: &Path) -> Result<PathBuf> {
    let output = Command::new(node_path)
        .arg("-e")
        .arg("console.log(require.resolve('playwright'))")
        .current_dir(dir)
        .output()
        .await
        .context("Failed to run Node.js to resolve Playwright")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Node.js failed to resolve Playwright: {stderr}"));
    }

    let path = String::from_utf8(output.stdout)
        .context("Invalid UTF-8 from Node.js")?
        .trim()
        .to_string();

    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_default_is_chromium() {
        assert_eq!(Browser::default(), Browser::Chromium);
    }

    #[test]
    fn browser_as_str() {
        assert_eq!(Browser::Chromium.as_str(), "chromium");
        assert_eq!(Browser::Firefox.as_str(), "firefox");
        assert_eq!(Browser::Webkit.as_str(), "webkit");
    }

    #[test]
    fn mode_default_is_subscribe() {
        let player = WhepBrowserPlayer::new("http://localhost/whep/live");
        assert_eq!(player.mode, Mode::Subscribe);
    }
}
