#![cfg(feature = "whepwright")]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::process::{Child, Command};

mod common;
use common::shutdown_signal;

macro_rules! bin_path {
    ($name:literal) => {
        option_env!(concat!("CARGO_BIN_EXE_", $name)).map_or_else(
            || {
                let mut path = std::env::current_exe().unwrap();
                path.pop();
                path.pop();
                path.push($name);
                path
            },
            PathBuf::from,
        )
    };
}

async fn start_liveion() -> SocketAddr {
    unsafe {
        std::env::set_var("LIVE777_WEBRTC_ICE_UDP_ADDRS", "127.0.0.1:0");
    }
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let mut config = liveion::config::Config::default();
    config.http.cors = vec!["*".to_string()].into();
    tokio::spawn(liveion::serve(config, listener, shutdown_signal()));
    addr
}

async fn wait_for_publisher(addr: SocketAddr, stream: &str, publisher: &mut Child) {
    for _ in 0..300 {
        assert!(
            publisher.try_wait().unwrap().is_none(),
            "FFmpeg exited before connecting"
        );
        let response = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
            .await
            .unwrap();
        let streams = response.json::<Vec<api::response::Stream>>().await.unwrap();
        if streams.iter().any(|item| {
            item.id == stream
                && item.publish.sessions.iter().any(|session| {
                    session.state == api::response::RTCPeerConnectionState::Connected
                })
        }) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("publisher did not connect within 30 seconds");
}

async fn run_whepwright(args: &[&str]) -> Output {
    let mut command = Command::new(bin_path!("whepwright"));
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(60), command.output())
        .await
        .expect("whepwright did not exit within 60 seconds")
        .unwrap()
}

fn parse_json(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!(
            "whepwright stdout is not pure JSON: {error}\nstdout:\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn artifact_path<'a>(report: &'a serde_json::Value, name: &str) -> &'a Path {
    report["artifacts"][name]
        .as_str()
        .unwrap_or_else(|| panic!("missing artifacts.{name}: {report}"))
        .as_ref()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn success_emits_pure_json_and_cleans_diagnostics() {
    let addr = start_liveion().await;
    let stream = format!("whepwright-success-{}", std::process::id());
    let mut publisher = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "warning",
            "-re",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=15",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-pix_fmt",
            "yuv420p",
            "-g",
            "15",
            "-keyint_min",
            "15",
            "-b:v",
            "1M",
            "-f",
            "whip",
            &format!("http://{addr}/whip/{stream}"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    wait_for_publisher(addr, &stream, &mut publisher).await;

    let temp = tempfile::tempdir().unwrap();
    let diagnostics = temp.path().join("diagnostics");
    let output = run_whepwright(&[
        "--whep",
        &format!("http://{addr}/whep/{stream}"),
        "--output",
        "json",
        "--timeout",
        "30",
        "--diagnostics-dir",
        diagnostics.to_str().unwrap(),
    ])
    .await;
    let report = parse_json(&output);
    assert!(
        output.status.success(),
        "whepwright failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report["success"], true);
    assert_eq!(report["connected"], true);
    assert!(report["width"].as_u64().unwrap_or(0) > 0);
    assert_eq!(report["artifacts"]["retained"], false);
    let run_directory = PathBuf::from(report["artifacts"]["directory"].as_str().unwrap());
    assert!(run_directory.starts_with(&diagnostics));
    assert!(!run_directory.exists(), "success diagnostics were retained");

    publisher.kill().await.unwrap();
    publisher.wait().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failure_retains_sanitized_diagnostics_and_trace() {
    let addr = start_liveion().await;
    let temp = tempfile::tempdir().unwrap();
    let diagnostics = temp.path().join("diagnostics");
    let token = "whepwright-test-secret";
    let output = run_whepwright(&[
        "--whep",
        &format!(
            "http://{addr}/whep/whepwright-no-media-{}",
            std::process::id()
        ),
        "--output",
        "json",
        "--timeout",
        "1",
        "--trace",
        "--diagnostics-dir",
        diagnostics.to_str().unwrap(),
    ])
    .await;
    let report = parse_json(&output);
    assert!(
        !output.status.success(),
        "no-media playback unexpectedly passed"
    );
    assert_eq!(report["success"], false);
    assert_eq!(report["artifacts"]["retained"], true);

    let run_directory = PathBuf::from(report["artifacts"]["directory"].as_str().unwrap());
    assert!(run_directory.starts_with(&diagnostics));
    assert!(run_directory.is_dir());
    let screenshot = std::fs::read(artifact_path(&report, "screenshot")).unwrap();
    assert!(screenshot.starts_with(b"\x89PNG\r\n\x1a\n"));
    let trace_path = artifact_path(&report, "trace");
    let trace = std::fs::read(trace_path).unwrap();
    assert!(trace.starts_with(b"PK"));
    let extracted = temp.path().join("unpacked-trace");
    let scan = Command::new("python3")
        .arg("-c")
        .arg(
            "import pathlib, sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2]); token = sys.argv[3].encode(); leaked = [str(p) for p in pathlib.Path(sys.argv[2]).rglob('*') if p.is_file() and token in p.read_bytes()]; print('\\n'.join(leaked)); sys.exit(bool(leaked))",
        )
        .arg(trace_path)
        .arg(&extracted)
        .arg(token)
        .output()
        .await
        .unwrap();
    assert!(
        scan.status.success(),
        "trace contains the token:\n{}",
        String::from_utf8_lossy(&scan.stdout)
    );

    let console = std::fs::read_to_string(artifact_path(&report, "console_log")).unwrap();
    assert!(
        console.contains("[console:"),
        "console events were not captured"
    );
    assert!(
        console.contains("[runner]"),
        "runner events were not captured"
    );
    assert!(!console.contains(token), "console log leaked the token");

    let sdp_ice_path = artifact_path(&report, "sdp_ice");
    let sdp_ice_text = std::fs::read_to_string(sdp_ice_path).unwrap();
    let sdp_ice: serde_json::Value = serde_json::from_str(&sdp_ice_text).unwrap();
    assert!(
        sdp_ice["stateTimeline"]
            .as_array()
            .is_some_and(|v| !v.is_empty())
    );
    assert!(sdp_ice["candidateTypeCounts"].is_object());
    assert!(sdp_ice.get("selectedPair").is_some());
    assert!(sdp_ice["rtp"].is_array());
    assert!(!sdp_ice_text.contains(token));
    assert!(!sdp_ice_text.contains("v=0"));
    assert!(!sdp_ice_text.contains("a=candidate"));
}

#[tokio::test]
async fn trace_with_token_is_rejected_without_starting_a_run() {
    let temp = tempfile::tempdir().unwrap();
    let diagnostics = temp.path().join("diagnostics");
    let output = run_whepwright(&[
        "--whep",
        "http://127.0.0.1:9/whep/unused",
        "--output",
        "json",
        "--trace",
        "--token",
        "whepwright-test-secret",
        "--diagnostics-dir",
        diagnostics.to_str().unwrap(),
    ])
    .await;
    let report = parse_json(&output);
    assert!(!output.status.success());
    assert_eq!(report["success"], false);
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .contains("cannot be used with a token")
    );
    assert_eq!(report["artifacts"]["retained"], false);
    let run_directory = PathBuf::from(report["artifacts"]["directory"].as_str().unwrap());
    assert!(!run_directory.exists());
    assert!(!diagnostics.exists());
}
