//! End-to-end test for the default diagnostics smoke flow: the real
//! `whipsynth` binary publishes a synthetic stream into an in-process liveion
//! server and the real `whepprobe` binary verifies it, asserting on the
//! probe's exit code and JSON report. This is the documented out-of-the-box
//! verification path (synthetic WHIP publish -> SFU -> WHEP probe decode) and
//! needs no external encoder, browser, or ICE server.
//!
//! Requires the `rsmpeg` feature: both binaries are gated on it.

#![cfg(feature = "rsmpeg")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::task::JoinHandle;

mod common;
use common::shutdown_signal;

static TRACING_INIT: Once = Once::new();

fn init_test_environment() {
    TRACING_INIT.call_once(|| {
        // The server and both WebRTC children (which inherit the variable) run
        // on this host. Pin ICE candidates to loopback so CI runners cannot
        // choose an unroutable interface.
        unsafe {
            std::env::set_var("LIVE777_WEBRTC_ICE_UDP_ADDRS", "127.0.0.1:0");
        }

        let filter = std::env::var("RUST_LOG")
            .unwrap_or_else(|_| "live777=info,liveion=info,livetwo=info,libwish=info".to_string());
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

/// Locate one of the workspace's binaries built alongside this test run.
macro_rules! bin_path {
    ($name:literal) => {
        // Cargo sets CARGO_BIN_EXE_<name> for integration tests of the package
        // that builds the binary; fall back to the conventional target
        // directory layout.
        option_env!(concat!("CARGO_BIN_EXE_", $name)).map_or_else(
            || {
                let mut path = std::env::current_exe().unwrap();
                path.pop(); // deps/
                path.pop(); // <profile>/
                path.push($name);
                path
            },
            PathBuf::from,
        )
    };
}

fn whipsynth_path() -> PathBuf {
    bin_path!("whipsynth")
}

fn whepprobe_path() -> PathBuf {
    bin_path!("whepprobe")
}

fn tool(path: PathBuf, args: &[&str]) -> Command {
    let mut cmd = Command::new(path);
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// Shared capture buffer: the drain tasks append everything they read, so a
/// panicking test can still inspect the child's output up to that point.
type OutputSink = Arc<Mutex<Vec<u8>>>;

/// A child whose piped stdout and stderr are drained to EOF by background
/// tasks, started right after spawn. Without this the child would block on
/// write(2) once a pipe buffer fills (~64KiB), because nothing reads the pipes
/// until the test collects the output. Everything read lands in the shared
/// sinks, keeping it available to the test even before the drain tasks are
/// joined.
struct SpawnedTool {
    child: tokio::process::Child,
    stdout: JoinHandle<std::io::Result<()>>,
    stderr: JoinHandle<std::io::Result<()>>,
    stdout_sink: OutputSink,
    stderr_sink: OutputSink,
}

/// Drain a child pipe chunk by chunk into `sink`.
fn spawn_drain<R>(mut pipe: R, sink: OutputSink) -> JoinHandle<std::io::Result<()>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut chunk = [0u8; 8192];
        loop {
            let n = pipe.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            sink.lock().unwrap().extend_from_slice(&chunk[..n]);
        }
    })
}

/// Spawn a tool and immediately start draining both pipes.
fn spawn_tool(path: PathBuf, args: &[&str]) -> SpawnedTool {
    let mut child = tool(path, args).spawn().unwrap();
    let stdout_pipe = child.stdout.take().unwrap();
    let stderr_pipe = child.stderr.take().unwrap();
    let stdout_sink = OutputSink::default();
    let stderr_sink = OutputSink::default();
    let stdout = spawn_drain(stdout_pipe, Arc::clone(&stdout_sink));
    let stderr = spawn_drain(stderr_pipe, Arc::clone(&stderr_sink));
    SpawnedTool {
        child,
        stdout,
        stderr,
        stdout_sink,
        stderr_sink,
    }
}

impl SpawnedTool {
    /// Everything drained from the child's stdout and stderr so far.
    fn captured_output(&self) -> (String, String) {
        (
            String::from_utf8_lossy(&self.stdout_sink.lock().unwrap()).into_owned(),
            String::from_utf8_lossy(&self.stderr_sink.lock().unwrap()).into_owned(),
        )
    }

    /// Whether the child has already exited.
    fn has_exited(&mut self) -> bool {
        self.child.try_wait().unwrap().is_some()
    }

    /// Kill the child, wait for it to exit, then join the drain tasks and
    /// return the captured output in the same shape as `Command::output()`.
    /// A child that already exited makes `kill` report InvalidInput; the wait
    /// still collects its final status.
    async fn kill_and_wait(self) -> std::process::Output {
        let SpawnedTool {
            mut child,
            stdout,
            stderr,
            stdout_sink,
            stderr_sink,
        } = self;
        let _ = child.kill().await;
        let status = child.wait().await.unwrap();
        stdout.await.unwrap().unwrap();
        stderr.await.unwrap().unwrap();
        let stdout = std::mem::take(&mut *stdout_sink.lock().unwrap());
        let stderr = std::mem::take(&mut *stderr_sink.lock().unwrap());
        std::process::Output {
            status,
            stdout,
            stderr,
        }
    }
}

/// Start liveion on an ephemeral loopback port and return its address.
async fn start_liveion() -> SocketAddr {
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let mut cfg = liveion::config::Config::default();
    cfg.http.cors = vec!["*".to_string()].into();
    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));
    addr
}

/// Wait until `stream_id` has a Connected publish session.
///
/// The 30s budget outlasts the publisher's own connect timeout, so a
/// struggling publisher surfaces its real error instead of a poll timeout. A
/// publisher that already exited will never connect, so fail at once; either
/// way the panic carries the output captured from the publisher so far.
async fn wait_stream_connected(addr: &SocketAddr, stream_id: &str, publisher: &mut SpawnedTool) {
    for attempt in 0..300 {
        if publisher.has_exited() {
            let (stdout, stderr) = publisher.captured_output();
            panic!(
                "publisher exited before stream '{stream_id}' reached Connected:\nstdout:\n{stdout}\nstderr:\n{stderr}"
            );
        }

        let res = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();
        assert_eq!(http::StatusCode::OK, res.status());

        let body = res.json::<Vec<api::response::Stream>>().await.unwrap();
        if let Some(r) = body.into_iter().find(|i| i.id == stream_id)
            && r.publish
                .sessions
                .iter()
                .any(|s| s.state == api::response::RTCPeerConnectionState::Connected)
        {
            return;
        }

        if attempt == 299 {
            let (stdout, stderr) = publisher.captured_output();
            panic!(
                "stream '{stream_id}' did not reach Connected\npublisher:\nstdout:\n{stdout}\nstderr:\n{stderr}"
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Parse the probe's JSON report. JSON mode keeps logs on stderr so stdout is
/// machine-readable without log-stripping heuristics.
fn parse_probe_json(stdout: &str, stderr: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "whepprobe stdout is not a pure JSON report: {e}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        )
    })
}

/// The default smoke flow closes the loop: whipsynth publishes VP8 video with
/// opus audio, and whepprobe connects, decodes frames, and reports success.
///
/// Multi-threaded runtime: the in-process server handles ICE/DTLS/SRTP for
/// both sessions, and a single-threaded runtime can stall ICE consent checks
/// long enough to drop connections under load.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn whipsynth_vp8_av_stream_probes_successfully() {
    init_test_environment();
    let addr = start_liveion().await;
    let base = format!("http://{addr}");
    let stream = format!("smoke-av-{}", std::process::id());

    // Publish in the background until the cleanup below kills the publisher.
    // `--ice-server ""` disables ICE servers on both peers, so the test sends
    // no STUN/TURN traffic and passes on CI runners without UDP egress: every
    // peer connects over loopback host candidates.
    let mut publisher = spawn_tool(
        whipsynth_path(),
        &[
            "-v",
            "--whip",
            &format!("{base}/whip/{stream}"),
            "--vcodec",
            "vp8",
            "--acodec",
            "opus",
            "--ice-server",
            "",
        ],
    );

    // Probe only once the publish session is up.
    wait_stream_connected(&addr, &stream, &mut publisher).await;

    let mut probe = tool(
        whepprobe_path(),
        &[
            "--whep",
            &format!("{base}/whep/{stream}"),
            "--output",
            "json",
            // whipsynth emits one keyframe per second; leave one full GOP of
            // slack instead of ending the decode window exactly at one GOP.
            "--decode-duration",
            "2",
            "--timeout",
            "30",
            "--ice-server",
            "",
        ],
    );
    // The probe's own timeout is 30s; give the process twice that to exit.
    let probe = tokio::time::timeout(Duration::from_secs(60), probe.output())
        .await
        .expect("whepprobe did not exit within 60s")
        .unwrap();
    let probe_stdout = String::from_utf8_lossy(&probe.stdout).into_owned();
    let probe_stderr = String::from_utf8_lossy(&probe.stderr).into_owned();

    {
        let (pub_stdout, pub_stderr) = publisher.captured_output();
        assert!(
            probe.status.success(),
            "whepprobe failed:\nstdout:\n{probe_stdout}\nstderr:\n{probe_stderr}\npublisher stdout:\n{pub_stdout}\npublisher stderr:\n{pub_stderr}"
        );
    }

    let report = parse_probe_json(&probe_stdout, &probe_stderr);
    assert_eq!(
        Some(true),
        report["success"].as_bool(),
        "probe report:\n{probe_stdout}"
    );
    assert_eq!(
        Some(true),
        report["connected"].as_bool(),
        "probe report:\n{probe_stdout}"
    );
    assert!(
        report["frame_count"].as_u64().unwrap_or(0) > 0,
        "probe decoded no frames:\n{probe_stdout}"
    );
    assert!(
        report["width"].as_u64().unwrap_or(0) > 0,
        "probe report has no width:\n{probe_stdout}"
    );
    assert!(
        report["height"].as_u64().unwrap_or(0) > 0,
        "probe report has no height:\n{probe_stdout}"
    );
    assert_eq!(
        Some("vp8"),
        report["video_codec"].as_str(),
        "probe report:\n{probe_stdout}"
    );
    // The rsmpeg probe does not decode audio; it only reports the codec
    // negotiated once the first audio RTP packet arrives and the bytes it
    // drained. Assert on those reported fields only -- nothing here claims
    // the audio was decoded.
    assert_eq!(
        Some("opus"),
        report["audio_codec"].as_str(),
        "probe report:\n{probe_stdout}"
    );
    assert_eq!(
        Some(1),
        report["audio_tracks"].as_u64(),
        "probe report:\n{probe_stdout}"
    );
    assert!(
        report["audio_bytes_received"].as_u64().unwrap_or(0) > 0,
        "probe received no audio bytes:\n{probe_stdout}"
    );

    // The publisher must have survived the whole probe; a publisher that died
    // mid-run would have silenced the stream underneath it.
    let alive = !publisher.has_exited();
    let (pub_stdout, pub_stderr) = publisher.captured_output();
    assert!(
        alive,
        "publisher exited during the probe:\nstdout:\n{pub_stdout}\nstderr:\n{pub_stderr}"
    );

    // Cleanup: kill the publisher and wait for it and its drain tasks so no
    // child process or tokio task leaks out of the test.
    let publisher = publisher.kill_and_wait().await;
    let pub_stdout = String::from_utf8_lossy(&publisher.stdout);
    let pub_stderr = String::from_utf8_lossy(&publisher.stderr);
    assert!(
        !publisher.status.success(),
        "killed publisher should report a non-success status, got {:?}:\nstdout:\n{pub_stdout}\nstderr:\n{pub_stderr}",
        publisher.status
    );
}

/// The probe reports a structured failure for an endpoint that never delivers
/// media: non-zero exit, parseable JSON, `success: false`. The short timeout
/// keeps the run fast whether the server rejects the subscribe outright or
/// accepts it and the peer connects but stays silent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn whepprobe_reports_failure_without_media() {
    init_test_environment();
    let addr = start_liveion().await;
    let stream = format!("no-media-{}", std::process::id());

    let mut probe = tool(
        whepprobe_path(),
        &[
            "--whep",
            &format!("http://{addr}/whep/{stream}"),
            "--output",
            "json",
            "--decode-duration",
            "1",
            "--timeout",
            "5",
            "--ice-server",
            "",
        ],
    );
    // The probe's own timeout is 5s; anything beyond 30s is a hung process.
    let probe = tokio::time::timeout(Duration::from_secs(30), probe.output())
        .await
        .expect("whepprobe did not exit within 30s")
        .unwrap();
    let probe_stdout = String::from_utf8_lossy(&probe.stdout).into_owned();
    let probe_stderr = String::from_utf8_lossy(&probe.stderr).into_owned();

    assert!(
        !probe.status.success(),
        "whepprobe unexpectedly succeeded against a stream without media:\nstdout:\n{probe_stdout}\nstderr:\n{probe_stderr}"
    );
    let report = parse_probe_json(&probe_stdout, &probe_stderr);
    assert_eq!(
        Some(false),
        report["success"].as_bool(),
        "probe report:\n{probe_stdout}"
    );
}
