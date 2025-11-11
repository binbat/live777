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

    let mut outputs = wait_for_video_outputs(storage_root.as_path()).await?;

    assert!(outputs.manifest.exists(), "manifest.mpd not found");
    let manifest_content = fs::read_to_string(&outputs.manifest)?;
    assert!(
        manifest_content.contains("<MPD"),
        "manifest content invalid"
    );

    let init_segment = outputs
        .init_segment
        .as_ref()
        .expect("init.m4s path missing");
    assert!(init_segment.exists(), "init.m4s not found");

    outputs.video_segments.sort();
    assert!(
        outputs.video_segments.len() >= 2,
        "expected at least two video segments, got {}",
        outputs.video_segments.len()
    );

    for seg_path in outputs.video_segments.iter().take(2) {
        let metadata = fs::metadata(seg_path)?;
        assert!(metadata.len() > 0, "segment {:?} is empty", seg_path);
    }

    assert!(
        outputs.audio_segments.is_empty(),
        "unexpected audio segments: {:?}",
        outputs.audio_segments
    );
    assert!(
        outputs.audio_init_segment.is_none(),
        "unexpected audio_init.m4s present"
    );

    Ok(())
}

#[tokio::test]
async fn test_recorder_generates_av1_segments() -> anyhow::Result<()> {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await?;
    let addr = listener.local_addr()?;

    let storage_dir = TempDir::new()?;
    let storage_root = storage_dir.path().join("records");
    fs::create_dir_all(&storage_root)?;

    let stream_id = "recorder-av1";

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

    let rtp_port: u16 = 5232;
    let sdp_dir = TempDir::new()?;
    let sdp_path = sdp_dir.path().join("input.sdp");
    let sdp_path_str = sdp_path.to_string_lossy().into_owned();

    let ffmpeg_cmd = format!(
        "ffmpeg -re -f lavfi -i testsrc=size=640x360:rate=30 -pix_fmt yuv420p -c:v libaom-av1 -cpu-used 8 -tile-columns 0 -tile-rows 0 -row-mt 1 -lag-in-frames 0 -g 30 -keyint_min 30 -b:v 0 -crf 30 -threads 4 -strict experimental -f rtp \"rtp://{}\" -sdp_file {}",
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

    sleep(Duration::from_secs(30)).await;

    client
        .delete(format!("http://{addr}{}", api::path::streams(stream_id)))
        .send()
        .await?;

    sleep(Duration::from_secs(5)).await;

    whip_future.abort();
    let _ = whip_future.await;

    let mut outputs = wait_for_video_outputs(storage_root.as_path()).await?;

    assert!(outputs.manifest.exists(), "manifest.mpd not found");
    let manifest_content = fs::read_to_string(&outputs.manifest)?;
    assert!(
        manifest_content.contains("codecs=\"av01"),
        "manifest does not advertise AV1 codec"
    );

    let init_segment = outputs
        .init_segment
        .as_ref()
        .expect("init.m4s path missing");
    assert!(init_segment.exists(), "init.m4s not found");

    outputs.video_segments.sort();
    assert!(
        !outputs.video_segments.is_empty(),
        "expected at least one video segment, got {}",
        outputs.video_segments.len()
    );

    for seg_path in outputs.video_segments.iter().take(2) {
        let metadata = fs::metadata(seg_path)?;
        assert!(metadata.len() > 0, "segment {:?} is empty", seg_path);
    }

    assert!(
        outputs.audio_segments.is_empty(),
        "unexpected audio segments: {:?}",
        outputs.audio_segments
    );
    assert!(
        outputs.audio_init_segment.is_none(),
        "unexpected audio_init.m4s present"
    );

    Ok(())
}

#[tokio::test]
async fn test_recorder_generates_opus_audio_segments() -> anyhow::Result<()> {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await?;
    let addr = listener.local_addr()?;

    let storage_dir = TempDir::new()?;
    let storage_root = storage_dir.path().join("records");
    fs::create_dir_all(&storage_root)?;

    let stream_id = "recorder-opus-only";

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

    let rtp_port: u16 = 5212;
    let sdp_dir = TempDir::new()?;
    let sdp_path = sdp_dir.path().join("input.sdp");
    let sdp_path_str = sdp_path.to_string_lossy().into_owned();

    let ffmpeg_cmd = format!(
        "ffmpeg -re -f lavfi -i sine=frequency=1000:sample_rate=48000 -ac 2 -acodec libopus -b:a 128k -application lowdelay -f rtp \"rtp://{}\" -sdp_file {}",
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

    let mut outputs = collect_recording_outputs(storage_root.as_path());

    assert!(outputs.manifest.exists(), "manifest.mpd not found");
    let manifest_content = fs::read_to_string(&outputs.manifest)?;
    assert!(manifest_content.contains("contentType=\"audio\""));

    let audio_init = outputs
        .audio_init_segment
        .as_ref()
        .expect("audio_init.m4s path missing");
    assert!(audio_init.exists(), "audio_init.m4s not found");

    outputs.audio_segments.sort();
    assert!(
        outputs.audio_segments.len() >= 2,
        "expected at least two audio segments, got {}",
        outputs.audio_segments.len()
    );

    for seg_path in outputs.audio_segments.iter().take(2) {
        let metadata = fs::metadata(seg_path)?;
        assert!(metadata.len() > 0, "audio segment {:?} is empty", seg_path);
    }

    assert!(
        outputs.video_segments.is_empty(),
        "unexpected video segments: {:?}",
        outputs.video_segments
    );
    assert!(
        outputs.init_segment.is_none(),
        "unexpected init.m4s present for audio-only recording"
    );

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

#[derive(Debug)]
struct RecordingOutputs {
    manifest: PathBuf,
    video_segments: Vec<PathBuf>,
    audio_segments: Vec<PathBuf>,
    init_segment: Option<PathBuf>,
    audio_init_segment: Option<PathBuf>,
}

fn collect_recording_outputs(root: &Path) -> RecordingOutputs {
    let mut dirs = VecDeque::from([root.to_path_buf()]);
    let mut manifest = None;
    let mut init_segment = None;
    let mut audio_init_segment = None;
    let mut video_segments = Vec::new();
    let mut audio_segments = Vec::new();

    while let Some(dir) = dirs.pop_front() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push_back(path);
                } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    match name {
                        "manifest.mpd" => manifest = Some(path.clone()),
                        "init.m4s" => init_segment = Some(path.clone()),
                        "audio_init.m4s" => audio_init_segment = Some(path.clone()),
                        _ => {
                            if name.starts_with("v_seg_") && name.ends_with(".m4s") {
                                video_segments.push(path.clone());
                            } else if name.starts_with("a_seg_") && name.ends_with(".m4s") {
                                audio_segments.push(path.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    RecordingOutputs {
        manifest: manifest.unwrap_or_else(|| root.join("manifest.mpd")),
        video_segments,
        audio_segments,
        init_segment,
        audio_init_segment,
    }
}

async fn wait_for_video_outputs(root: &Path) -> anyhow::Result<RecordingOutputs> {
    let mut attempts = 0;
    let mut last = None;
    while attempts < 150 {
        let outputs = collect_recording_outputs(root);
        let has_manifest = outputs.manifest.exists();
        let has_init = outputs.init_segment.as_ref().is_some_and(|p| p.exists());
        let has_segment = outputs.video_segments.iter().any(|p| p.exists());
        if has_manifest && has_init && has_segment {
            return Ok(outputs);
        }
        last = Some(outputs);
        attempts += 1;
        sleep(Duration::from_millis(200)).await;
    }

    Err(anyhow::anyhow!(
        "timed out waiting for video outputs: {:?}",
        last
    ))
}
