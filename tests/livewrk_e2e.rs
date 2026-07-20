//! End-to-end test for the livewrk load-testing tool: real `livewrk` binary
//! subprocesses publish and subscribe synthetic streams against an
//! in-process liveion server, exercising the CLI, exit codes and the
//! rotating decode verification.
//!
//! Requires the `rsmpeg` feature: the `whip` subcommand and WHEP decode
//! verification depend on it.

#![cfg(feature = "rsmpeg")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Once;
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
        // Both WebRTC peers run locally, in this process and in the spawned
        // livewrk children (which inherit the variable). Pin ICE candidates
        // to loopback so CI runners cannot choose an unroutable interface.
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

/// Locate the livewrk binary built alongside this test run.
fn livewrk_path() -> PathBuf {
    // Cargo sets this for integration tests of the package that builds the
    // binary; fall back to the conventional target directory layout.
    option_env!("CARGO_BIN_EXE_livewrk").map_or_else(
        || {
            let mut path = std::env::current_exe().unwrap();
            path.pop(); // deps/
            path.pop(); // <profile>/
            path.push("livewrk");
            path
        },
        PathBuf::from,
    )
}

fn livewrk(args: &[&str]) -> Command {
    let mut cmd = Command::new(livewrk_path());
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    cmd
}

/// A livewrk child whose piped stdout and stderr are drained to EOF by
/// background tasks, started right after spawn. Without this the child would
/// block on write(2) once a pipe buffer fills (~64KiB), because nothing reads
/// the pipes until the test collects the output.
struct SpawnedLivewrk {
    child: tokio::process::Child,
    stdout: JoinHandle<std::io::Result<Vec<u8>>>,
    stderr: JoinHandle<std::io::Result<Vec<u8>>>,
}

/// Spawn livewrk and immediately start draining both pipes.
fn spawn_livewrk(args: &[&str]) -> SpawnedLivewrk {
    let mut child = livewrk(args).spawn().unwrap();
    let mut stdout_pipe = child.stdout.take().unwrap();
    let mut stderr_pipe = child.stderr.take().unwrap();
    let stdout = tokio::spawn(async move {
        let mut buf = Vec::new();
        stdout_pipe.read_to_end(&mut buf).await?;
        Ok(buf)
    });
    let stderr = tokio::spawn(async move {
        let mut buf = Vec::new();
        stderr_pipe.read_to_end(&mut buf).await?;
        Ok(buf)
    });
    SpawnedLivewrk {
        child,
        stdout,
        stderr,
    }
}

impl SpawnedLivewrk {
    /// Wait for the child to exit, then join the drain tasks and return the
    /// captured output in the same shape as `Command::output()`.
    async fn wait(self) -> std::process::Output {
        let SpawnedLivewrk {
            mut child,
            stdout,
            stderr,
        } = self;
        let status = child.wait().await.unwrap();
        let stdout = stdout.await.unwrap().unwrap();
        let stderr = stderr.await.unwrap().unwrap();
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
    cfg.http.cors = true;
    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));
    addr
}

/// Wait until `stream_id` has a Connected publish session.
///
/// The 30s budget outlasts the publisher's own 15s connect timeout, so a
/// struggling publisher surfaces its real error instead of a poll timeout.
async fn wait_stream_connected(addr: &SocketAddr, stream_id: &str) {
    for attempt in 0..300 {
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

        assert!(
            attempt < 299,
            "stream '{stream_id}' did not reach Connected"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Extract the verification window count from livewrk's WHEP report.
fn verify_windows_total(stdout: &str, stderr: &str) -> u64 {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("Windows: "))
        .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| {
            panic!(
                "livewrk output missing verification windows line:\nstdout:\n{stdout}\nstderr:\n{stderr}"
            )
        })
}

/// Extract the total packet count from livewrk's stats report.
fn packets_total(stdout: &str, stderr: &str) -> u64 {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("Packets: "))
        .and_then(|rest| {
            rest.split_whitespace()
                .next()?
                .trim_end_matches(',')
                .parse()
                .ok()
        })
        .unwrap_or_else(|| {
            panic!("livewrk output missing packets line:\nstdout:\n{stdout}\nstderr:\n{stderr}")
        })
}

/// Two publishers (one stream and three streams) and two subscribers with
/// decode verification, all as livewrk subprocesses against one liveion.
///
/// Multi-threaded runtime: the in-process server handles ICE/DTLS/SRTP for
/// all nine sessions, and a single-threaded runtime can stall ICE consent
/// checks long enough to drop publisher connections under load.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn whip_whep_publish_and_subscribe_with_verification() {
    init_test_environment();
    let addr = start_liveion().await;
    let base = format!("http://{addr}");

    // Publisher A: one stream (`single-0`); publisher B: three streams
    // (`multi-0/1/2`). The whip session index is appended to the URL's last
    // path segment. `--stun-server ""` disables STUN, keeping the test
    // hermetic on CI runners without UDP egress.
    let whip_single = spawn_livewrk(&[
        "whip",
        "--whip",
        &format!("{base}/whip/single"),
        "--sessions",
        "1",
        "--duration",
        "20",
        "--stun-server",
        "",
    ]);
    let whip_multi = spawn_livewrk(&[
        "whip",
        "--whip",
        &format!("{base}/whip/multi"),
        "--sessions",
        "3",
        "--duration",
        "20",
        "--stun-server",
        "",
    ]);

    // Subscribe only after every stream is publishing.
    for stream in ["single-0", "multi-0", "multi-1", "multi-2"] {
        wait_stream_connected(&addr, stream).await;
    }

    // Two concurrent subscribers: 2 sessions on the single stream and 3
    // sessions on one of the multi streams, both with decode verification.
    // The 12s duration covers session setup and codec announcement plus one
    // full 2s window: windows cut short by shutdown are not counted, and a
    // run with zero completed windows now exits non-zero.
    let mut whep_single = livewrk(&[
        "whep",
        "--whep",
        &format!("{base}/whep/single-0"),
        "--sessions",
        "2",
        "--duration",
        "12",
        "--verify-window",
        "2",
    ]);
    let mut whep_multi = livewrk(&[
        "whep",
        "--whep",
        &format!("{base}/whep/multi-1"),
        "--sessions",
        "3",
        "--duration",
        "12",
        "--verify-window",
        "2",
    ]);
    let (whep_single, whep_multi) = tokio::join!(whep_single.output(), whep_multi.output());
    let whep_single = whep_single.unwrap();
    let whep_multi = whep_multi.unwrap();

    let stdout_single = String::from_utf8_lossy(&whep_single.stdout);
    let stderr_single = String::from_utf8_lossy(&whep_single.stderr);
    assert!(
        whep_single.status.success(),
        "single-stream whep failed:\nstdout:\n{stdout_single}\nstderr:\n{stderr_single}"
    );
    assert!(
        stdout_single.contains("2 connected, 0 failed"),
        "stdout:\n{stdout_single}\nstderr:\n{stderr_single}"
    );
    assert!(
        verify_windows_total(&stdout_single, &stderr_single) > 0,
        "verification did not run:\nstdout:\n{stdout_single}\nstderr:\n{stderr_single}"
    );

    let stdout_multi = String::from_utf8_lossy(&whep_multi.stdout);
    let stderr_multi = String::from_utf8_lossy(&whep_multi.stderr);
    assert!(
        whep_multi.status.success(),
        "multi-stream whep failed:\nstdout:\n{stdout_multi}\nstderr:\n{stderr_multi}"
    );
    assert!(
        stdout_multi.contains("3 connected, 0 failed"),
        "stdout:\n{stdout_multi}\nstderr:\n{stderr_multi}"
    );
    assert!(
        verify_windows_total(&stdout_multi, &stderr_multi) > 0,
        "verification did not run:\nstdout:\n{stdout_multi}\nstderr:\n{stderr_multi}"
    );

    // The publishers exit cleanly once their duration elapses. Their pipes
    // have been drained since spawn, so collect exit status and output.
    let whip_single = tokio::time::timeout(Duration::from_secs(30), whip_single.wait())
        .await
        .expect("single-stream publisher did not stop");
    let whip_multi = tokio::time::timeout(Duration::from_secs(30), whip_multi.wait())
        .await
        .expect("multi-stream publisher did not stop");

    let stdout_single = String::from_utf8_lossy(&whip_single.stdout);
    let stderr_single = String::from_utf8_lossy(&whip_single.stderr);
    assert!(
        whip_single.status.success(),
        "single-stream whip failed:\nstdout:\n{stdout_single}\nstderr:\n{stderr_single}"
    );
    assert!(
        stdout_single.contains("1 connected, 0 failed"),
        "stdout:\n{stdout_single}\nstderr:\n{stderr_single}"
    );
    assert!(
        packets_total(&stdout_single, &stderr_single) > 0,
        "single-stream publisher sent no media:\nstdout:\n{stdout_single}\nstderr:\n{stderr_single}"
    );

    let stdout_multi = String::from_utf8_lossy(&whip_multi.stdout);
    let stderr_multi = String::from_utf8_lossy(&whip_multi.stderr);
    assert!(
        whip_multi.status.success(),
        "multi-stream whip failed:\nstdout:\n{stdout_multi}\nstderr:\n{stderr_multi}"
    );
    assert!(
        stdout_multi.contains("3 connected, 0 failed"),
        "stdout:\n{stdout_multi}\nstderr:\n{stderr_multi}"
    );
    assert!(
        packets_total(&stdout_multi, &stderr_multi) > 0,
        "multi-stream publisher sent no media:\nstdout:\n{stdout_multi}\nstderr:\n{stderr_multi}"
    );
}

/// Both subcommands exit non-zero when the server is unreachable.
#[tokio::test]
async fn whip_whep_fail_when_server_unreachable() {
    init_test_environment();
    // Bind-then-drop hands us a loopback port that refuses connections, the
    // same trick `start_liveion` uses to find a free port.
    let port = {
        let listener =
            std::net::TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
                .unwrap();
        listener.local_addr().unwrap().port()
    };
    let mut whep = livewrk(&[
        "whep",
        "--whep",
        &format!("http://127.0.0.1:{port}/whep/nope"),
        "--sessions",
        "2",
        "--duration",
        "5",
    ]);
    let mut whip = livewrk(&[
        "whip",
        "--whip",
        &format!("http://127.0.0.1:{port}/whip/nope"),
        "--sessions",
        "2",
        "--duration",
        "5",
        "--stun-server",
        "",
    ]);
    let (whep, whip) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(30), whep.output()),
        tokio::time::timeout(Duration::from_secs(30), whip.output())
    );

    let whep = whep
        .expect("livewrk whep did not exit within 30s against an unreachable server")
        .unwrap();
    assert!(
        !whep.status.success(),
        "whep unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&whep.stdout),
        String::from_utf8_lossy(&whep.stderr)
    );
    let whip = whip
        .expect("livewrk whip did not exit within 30s against an unreachable server")
        .unwrap();
    assert!(
        !whip.status.success(),
        "whip unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&whip.stdout),
        String::from_utf8_lossy(&whip.stderr)
    );
}
