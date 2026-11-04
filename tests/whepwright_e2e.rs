#![cfg(all(feature = "whepwright", feature = "rsmpeg"))]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::process::{Child, Command};

mod common;
use common::shutdown_signal;

async fn wait_for_publisher(
    addr: SocketAddr,
    stream: &str,
    publisher: &mut Child,
    token: Option<&str>,
) {
    for _ in 0..300 {
        assert!(
            publisher.try_wait().unwrap().is_none(),
            "publisher exited before connecting"
        );
        let mut request =
            reqwest::Client::new().get(format!("http://{addr}{}", api::path::streams("")));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
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

async fn start_publisher(addr: SocketAddr, stream: &str, token: Option<&str>) -> Child {
    start_publisher_with_codec(addr, stream, token, "h264").await
}

async fn start_publisher_with_codec(
    addr: SocketAddr,
    stream: &str,
    token: Option<&str>,
    codec: &str,
) -> Child {
    let mut command = Command::new(crate::bin_path!("whipsynth"));
    command
        .args([
            "-v",
            "--whip",
            &format!("http://{addr}/whip/{stream}"),
            "--vcodec",
            codec,
            "--width",
            "320",
            "--height",
            "240",
            "--fps",
            "15",
            "--ice-server",
            "",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    if let Some(token) = token {
        command.args(["--token", token]);
    }
    let mut publisher = command.spawn().unwrap();
    wait_for_publisher(addr, stream, &mut publisher, token).await;
    publisher
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WhepAuthRequest {
    authorized: bool,
    status: u16,
}

#[derive(Clone)]
struct AuthProxyState {
    upstream: SocketAddr,
    expected_authorization: String,
    requests: std::sync::Arc<std::sync::Mutex<Vec<WhepAuthRequest>>>,
    client: reqwest::Client,
}

async fn observe_whep_auth(
    axum::extract::State(state): axum::extract::State<AuthProxyState>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let (parts, body) = request.into_parts();
    let is_whep_post =
        parts.method == axum::http::Method::POST && parts.uri.path().starts_with("/whep/");
    let authorized = parts
        .headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == state.expected_authorization);
    let path = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path(), |path| path.as_str());
    let url = format!("http://{}{}", state.upstream, path);
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let upstream = state
        .client
        .request(parts.method, url)
        .headers(parts.headers)
        .body(body)
        .send()
        .await
        .unwrap();
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let body = upstream.bytes().await.unwrap();
    if is_whep_post {
        state.requests.lock().unwrap().push(WhepAuthRequest {
            authorized,
            status: status.as_u16(),
        });
    }
    let mut response = axum::response::Response::new(axum::body::Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    response
}

async fn start_auth_observing_proxy(
    upstream: SocketAddr,
    token: &str,
) -> (
    SocketAddr,
    std::sync::Arc<std::sync::Mutex<Vec<WhepAuthRequest>>>,
) {
    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let state = AuthProxyState {
        upstream,
        expected_authorization: format!("Bearer {token}"),
        requests: requests.clone(),
        client: reqwest::Client::new(),
    };
    let app = axum::Router::new()
        .fallback(axum::routing::any(observe_whep_auth))
        .with_state(state);
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .unwrap();
    });
    (addr, requests)
}

#[cfg(unix)]
async fn terminate_whepwright_tree_unix(child_id: Option<u32>) -> String {
    // `run_whepwright` makes the runner its own process-group leader, so a
    // single group SIGKILL also takes down the browser processes it started.
    // The library's `terminate_runner` keeps the exhaustive process-table
    // walk; the test only needs to not leave a stuck run behind.
    let Some(child_id) = child_id else {
        return "whepwright PID was unavailable; process-tree cleanup was not possible".to_string();
    };
    let target = format!("-{child_id}");
    match Command::new("kill")
        .args(["-KILL", target.as_str()])
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            format!("killed the whepwright process group {child_id}")
        }
        Ok(output) => format!(
            "kill {target} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => format!("failed to run kill {target}: {error}"),
    }
}

#[cfg(windows)]
async fn terminate_whepwright_tree_windows(child_id: Option<u32>) -> String {
    let Some(child_id) = child_id else {
        return "whepwright PID was unavailable; process-tree cleanup was not possible".to_string();
    };
    let pid = child_id.to_string();
    match Command::new("taskkill")
        .args(["/PID", pid.as_str(), "/T", "/F"])
        .output()
        .await
    {
        Ok(output) => format!(
            "taskkill exited with {}; stdout: {}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(error) => format!("failed to run taskkill for whepwright process tree: {error}"),
    }
}

async fn terminate_timed_out_whepwright(child_id: Option<u32>) -> String {
    #[cfg(unix)]
    return terminate_whepwright_tree_unix(child_id).await;
    #[cfg(windows)]
    return terminate_whepwright_tree_windows(child_id).await;
    #[cfg(not(any(unix, windows)))]
    {
        let _ = child_id;
        "process-tree cleanup is unsupported on this platform".to_string()
    }
}

async fn run_whepwright(args: &[&str]) -> Output {
    let mut command = Command::new(crate::bin_path!("whepwright"));
    command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    command.process_group(0);
    let child = command.spawn().unwrap();
    let child_id = child.id();
    match tokio::time::timeout(Duration::from_secs(60), child.wait_with_output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => {
            let cleanup = terminate_timed_out_whepwright(child_id).await;
            panic!("failed to collect whepwright output: {error}; {cleanup}");
        }
        Err(_) => {
            let cleanup = terminate_timed_out_whepwright(child_id).await;
            panic!("whepwright did not exit within 60 seconds; {cleanup}");
        }
    }
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
    let addr = common::start_liveion(None).await;
    let stream = format!("whepwright-success-{}", std::process::id());
    let mut publisher = start_publisher(addr, &stream, None).await;

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
async fn auth_token_is_present_on_first_whep_request_and_redacted() {
    let token = "whepwright-liveion-auth-secret";
    let addr = common::start_liveion(Some(token)).await;
    let unauthorized = reqwest::get(format!("http://{addr}{}", api::path::streams("")))
        .await
        .unwrap();
    assert_eq!(unauthorized.status().as_u16(), 401);

    let (proxy_addr, requests) = start_auth_observing_proxy(addr, token).await;
    let stream = format!("whepwright-auth-{}", std::process::id());
    let mut publisher = start_publisher_with_codec(addr, &stream, Some(token), "vp8").await;
    let temp = tempfile::tempdir().unwrap();
    let diagnostics = temp.path().join("diagnostics");
    let output = run_whepwright(&[
        "--whep",
        &format!("http://{proxy_addr}/whep/{stream}"),
        "--output",
        "json",
        "--timeout",
        "30",
        "--token",
        token,
        "--diagnostics-dir",
        diagnostics.to_str().unwrap(),
    ])
    .await;
    let report = parse_json(&output);
    assert!(
        output.status.success(),
        "authenticated whepwright failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(report["success"], true);
    assert_eq!(report["connected"], true);
    assert!(report["width"].as_u64().unwrap_or(0) > 0);

    let requests = requests.lock().unwrap().clone();
    assert_eq!(
        requests.len(),
        1,
        "expected exactly one WHEP POST, got {requests:?}"
    );
    assert!(
        requests[0].authorized,
        "first WHEP POST was not authenticated"
    );
    assert!(
        (200..300).contains(&requests[0].status),
        "first WHEP POST failed: {requests:?}"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(token));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(token));

    publisher.kill().await.unwrap();
    publisher.wait().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failure_retains_diagnostics_and_trace() {
    let addr = common::start_liveion(None).await;
    let temp = tempfile::tempdir().unwrap();
    let diagnostics = temp.path().join("diagnostics");
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
    let trace = std::fs::read(artifact_path(&report, "trace")).unwrap();
    assert!(trace.starts_with(b"PK"));

    let console = std::fs::read_to_string(artifact_path(&report, "console_log")).unwrap();
    assert!(
        console.contains("[console:"),
        "console events were not captured"
    );
    assert!(
        console.contains("[runner]"),
        "runner events were not captured"
    );

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
    assert!(!sdp_ice_text.contains("v=0"));
    assert!(!sdp_ice_text.contains("a=candidate"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failure_redacts_url_token_from_console_and_sdp_ice() {
    let addr = common::start_liveion(None).await;
    let temp = tempfile::tempdir().unwrap();
    let diagnostics = temp.path().join("diagnostics");
    let token = "whepwright-test-secret";
    let output = run_whepwright(&[
        "--whep",
        &format!(
            "http://{addr}/whep/whepwright-token-query-{}?token={token}",
            std::process::id()
        ),
        "--output",
        "json",
        "--timeout",
        "1",
        "--token",
        token,
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
    assert!(report["artifacts"]["trace"].is_null());

    let run_directory = PathBuf::from(report["artifacts"]["directory"].as_str().unwrap());
    assert!(run_directory.starts_with(&diagnostics));
    assert!(run_directory.is_dir());
    assert!(!run_directory.join("trace.zip").exists());
    let console = std::fs::read_to_string(artifact_path(&report, "console_log")).unwrap();
    assert!(console.contains("Starting WHEP subscribe:"));
    assert!(
        console.contains("?token=[REDACTED]"),
        "the URL token did not reach the browser console: {console}"
    );
    assert!(!console.contains(token), "console log leaked the token");

    let sdp_ice_text = std::fs::read_to_string(artifact_path(&report, "sdp_ice")).unwrap();
    let _: serde_json::Value = serde_json::from_str(&sdp_ice_text).unwrap();
    assert!(
        !sdp_ice_text.contains(token),
        "SDP/ICE log leaked the token"
    );
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
