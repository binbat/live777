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

use tokio::net::TcpListener;
use tokio::process::Command;

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
async fn wait_stream_connected(addr: &SocketAddr, stream_id: &str) {
    for attempt in 0..150 {
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
            attempt < 149,
            "stream '{stream_id}' did not reach Connected"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Extract the verification window count from livewrk's WHEP report.
fn verify_windows_total(stdout: &str) -> u64 {
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("Windows: "))
        .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
        .unwrap_or_else(|| panic!("livewrk output missing verification windows line:\n{stdout}"))
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
    // path segment.
    let whip_single = livewrk(&[
        "whip",
        "--whip",
        &format!("{base}/whip/single"),
        "--sessions",
        "1",
        "--duration",
        "20",
    ])
    .spawn()
    .unwrap();
    let whip_multi = livewrk(&[
        "whip",
        "--whip",
        &format!("{base}/whip/multi"),
        "--sessions",
        "3",
        "--duration",
        "20",
    ])
    .spawn()
    .unwrap();

    // Subscribe only after every stream is publishing.
    for stream in ["single-0", "multi-0", "multi-1", "multi-2"] {
        wait_stream_connected(&addr, stream).await;
    }

    // Two concurrent subscribers: 2 sessions on the single stream and 3
    // sessions on one of the multi streams, both with decode verification.
    let mut whep_single = livewrk(&[
        "whep",
        "--whep",
        &format!("{base}/whep/single-0"),
        "--sessions",
        "2",
        "--duration",
        "8",
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
        "8",
        "--verify-window",
        "2",
    ]);
    let (whep_single, whep_multi) = tokio::join!(whep_single.output(), whep_multi.output());
    let whep_single = whep_single.unwrap();
    let whep_multi = whep_multi.unwrap();

    let stdout_single = String::from_utf8_lossy(&whep_single.stdout);
    assert!(
        whep_single.status.success(),
        "single-stream whep failed:\n{stdout_single}"
    );
    assert!(
        stdout_single.contains("2 connected, 0 failed"),
        "{stdout_single}"
    );
    assert!(
        verify_windows_total(&stdout_single) > 0,
        "verification did not run:\n{stdout_single}"
    );

    let stdout_multi = String::from_utf8_lossy(&whep_multi.stdout);
    assert!(
        whep_multi.status.success(),
        "multi-stream whep failed:\n{stdout_multi}"
    );
    assert!(
        stdout_multi.contains("3 connected, 0 failed"),
        "{stdout_multi}"
    );
    assert!(
        verify_windows_total(&stdout_multi) > 0,
        "verification did not run:\n{stdout_multi}"
    );

    // The publishers exit cleanly once their duration elapses.
    let whip_single = tokio::time::timeout(Duration::from_secs(30), whip_single.wait_with_output())
        .await
        .expect("single-stream publisher did not stop")
        .unwrap();
    let whip_multi = tokio::time::timeout(Duration::from_secs(30), whip_multi.wait_with_output())
        .await
        .expect("multi-stream publisher did not stop")
        .unwrap();

    let stdout_single = String::from_utf8_lossy(&whip_single.stdout);
    assert!(
        whip_single.status.success(),
        "single-stream whip failed:\n{stdout_single}"
    );
    assert!(
        stdout_single.contains("1 connected, 0 failed"),
        "{stdout_single}"
    );

    let stdout_multi = String::from_utf8_lossy(&whip_multi.stdout);
    assert!(
        whip_multi.status.success(),
        "multi-stream whip failed:\n{stdout_multi}"
    );
    assert!(
        stdout_multi.contains("3 connected, 0 failed"),
        "{stdout_multi}"
    );
}

/// Both subcommands exit non-zero when the server is unreachable.
#[tokio::test]
async fn whip_whep_fail_when_server_unreachable() {
    init_test_environment();
    // The discard port refuses loopback connections immediately.
    let mut whep = livewrk(&[
        "whep",
        "--whep",
        "http://127.0.0.1:9/whep/nope",
        "--sessions",
        "2",
        "--duration",
        "5",
    ]);
    let mut whip = livewrk(&[
        "whip",
        "--whip",
        "http://127.0.0.1:9/whip/nope",
        "--sessions",
        "2",
        "--duration",
        "5",
    ]);
    let (whep, whip) = tokio::join!(whep.output(), whip.output());

    let whep = whep.unwrap();
    assert!(
        !whep.status.success(),
        "whep unexpectedly succeeded:\n{}",
        String::from_utf8_lossy(&whep.stdout)
    );
    let whip = whip.unwrap();
    assert!(
        !whip.status.success(),
        "whip unexpectedly succeeded:\n{}",
        String::from_utf8_lossy(&whip.stdout)
    );
}
