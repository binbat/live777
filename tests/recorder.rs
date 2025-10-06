#![cfg(feature = "recorder")]

use std::collections::VecDeque;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use api::response::{RTCPeerConnectionState, Stream as StreamInfo};
use liveion::config::Config;
use reqwest::Client;
use storage::StorageConfig;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::sleep;

mod common;
use common::shutdown_signal;

#[tokio::test]
async fn test_recorder_generates_h264_segments() -> anyhow::Result<()> {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await?;
    let addr = listener.local_addr()?;

    let storage_dir = TempDir::new()?;
    let storage_root = storage_dir.path().join("records");
    fs::create_dir_all(&storage_root)?;

    let stream_id = "recorder-test";

    let mut cfg = Config::default();
    cfg.recorder.auto_streams = vec![stream_id.to_string()];
    cfg.recorder.rotate_daily = false;
    cfg.recorder.storage = StorageConfig::Fs {
        root: storage_root.to_string_lossy().into_owned(),
    };

    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));

    let client = Client::new();
    client
        .post(format!("http://{addr}{}", api::path::streams(stream_id)))
        .send()
        .await?
        .error_for_status()?;

    let rtp_port: u16 = 5202;
    let sdp_dir = TempDir::new()?;
    let sdp_path = sdp_dir.path().join("input.sdp");
    let sdp_path_str = sdp_path.to_string_lossy().into_owned();

    let ffmpeg_cmd = format!(
        "ffmpeg -re -f lavfi -i testsrc=size=640x480:rate=30 -vcodec libx264 -profile:v baseline -level 3.0 -pix_fmt yuv420p -g 30 -keyint_min 30 -b:v 1000k -minrate 1000k -maxrate 1000k -bufsize 1000k -preset ultrafast -tune zerolatency -f rtp \"rtp://{}\" -sdp_file {}",
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), rtp_port),
        sdp_path_str
    );

    let whip_future = tokio::spawn(livetwo::whip::into(
        sdp_path_str.clone(),
        format!("http://{addr}{}", api::path::whip(stream_id)),
        None,
        Some(ffmpeg_cmd),
    ));

    wait_for_publish_connected(&client, addr, stream_id).await?;

    sleep(Duration::from_secs(25)).await;

    client
        .delete(format!("http://{addr}{}", api::path::streams(stream_id)))
        .send()
        .await?;

    sleep(Duration::from_secs(5)).await;

    whip_future.abort();
    let _ = whip_future.await;

    let (manifest_path, mut segments) = collect_recording_outputs(storage_root.as_path());

    assert!(manifest_path.exists(), "manifest.mpd not found");
    let manifest_content = fs::read_to_string(&manifest_path)?;
    assert!(
        manifest_content.contains("<MPD"),
        "manifest content invalid"
    );

    segments.sort();
    assert!(
        segments.len() >= 2,
        "expected at least two video segments, got {}",
        segments.len()
    );

    for seg_path in segments.iter().take(2) {
        let metadata = fs::metadata(seg_path)?;
        assert!(metadata.len() > 0, "segment {:?} is empty", seg_path);
    }

    Ok(())
}

async fn wait_for_publish_connected(
    client: &Client,
    addr: SocketAddr,
    stream_id: &str,
) -> anyhow::Result<()> {
    for _ in 0..200 {
        let res = client
            .get(format!("http://{addr}{}", api::path::streams("")))
            .send()
            .await?
            .error_for_status()?;

        let body = res.json::<Vec<StreamInfo>>().await?;
        if body.into_iter().any(|stream| {
            stream.id == stream_id
                && stream
                    .publish
                    .sessions
                    .first()
                    .is_some_and(|s| s.state == RTCPeerConnectionState::Connected)
        }) {
            return Ok(());
        }

        sleep(Duration::from_millis(200)).await;
    }

    anyhow::bail!("publisher never connected")
}

fn collect_recording_outputs(root: &Path) -> (PathBuf, Vec<PathBuf>) {
    let mut dirs = VecDeque::from([root.to_path_buf()]);
    let mut manifest = None;
    let mut segments = Vec::new();

    while let Some(dir) = dirs.pop_front() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push_back(path);
                } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name == "manifest.mpd" {
                        manifest = Some(path.clone());
                    } else if name.starts_with("seg_") && name.ends_with(".m4s") {
                        segments.push(path.clone());
                    }
                }
            }
        }
    }

    (
        manifest.unwrap_or_else(|| root.join("manifest.mpd")),
        segments,
    )
}
