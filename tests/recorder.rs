#![cfg(feature = "recorder")]

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

const PUBLISH_WAIT_TIMEOUT_SECS: u64 = 40;
const RECORDING_DURATION_SECS: u64 = 15;
const POST_RECORDING_WAIT_SECS: u64 = 8;
const OUTPUT_COLLECTION_TIMEOUT_SECS: u64 = 30;

#[tokio::test]
async fn test_recorder_generates_h264_segments() -> anyhow::Result<()> {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await?;
    let addr = listener.local_addr()?;

    let storage_dir = TempDir::new()?;
    let storage_root = storage_dir.path().join("records");
    fs::create_dir_all(&storage_root)?;

    let stream_id = "h264-test";

    let mut cfg = Config::default();
    cfg.recorder.auto_streams = vec![stream_id.to_string()];
    cfg.recorder.max_recording_seconds = 0;
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

    sleep(Duration::from_secs(RECORDING_DURATION_SECS)).await;

    client
        .delete(format!("http://{addr}{}", api::path::streams(stream_id)))
        .send()
        .await?;

    sleep(Duration::from_secs(POST_RECORDING_WAIT_SECS)).await;

    whip_future.abort();
    let _ = whip_future.await;

    let mut outputs = wait_for_video_outputs(&storage_root, stream_id).await?;

    assert!(outputs.manifest.exists(), "manifest.mpd not found");
    let manifest_content = fs::read_to_string(&outputs.manifest)?;
    assert!(
        manifest_content.contains("<MPD"),
        "manifest content invalid"
    );
    assert!(
        manifest_content.contains("video"),
        "manifest should contain video track"
    );

    let init_segment = outputs
        .video_init_segment
        .as_ref()
        .expect("v_init.m4s path missing");
    assert!(init_segment.exists(), "v_init.m4s not found");
    let init_size = fs::metadata(init_segment)?.len();
    assert!(init_size > 100, "v_init.m4s too small: {} bytes", init_size);

    outputs.video_segments.sort();
    assert!(
        !outputs.video_segments.is_empty(),
        "expected at least one video segment, got {}",
        outputs.video_segments.len()
    );

    for seg_path in &outputs.video_segments {
        let metadata = fs::metadata(seg_path)?;
        assert!(
            metadata.len() > 1000,
            "segment {:?} too small: {} bytes",
            seg_path.file_name(),
            metadata.len()
        );
    }

    assert!(
        outputs.audio_segments.is_empty(),
        "unexpected audio segments: {:?}",
        outputs.audio_segments
    );
    assert!(
        outputs.audio_init_segment.is_none(),
        "unexpected a_init.m4s present"
    );

    Ok(())
}

#[tokio::test]
async fn test_recorder_generates_vp9_segments() -> anyhow::Result<()> {
    let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let listener = TcpListener::bind(SocketAddr::new(ip, 0)).await?;
    let addr = listener.local_addr()?;

    let storage_dir = TempDir::new()?;
    let storage_root = storage_dir.path().join("records");
    fs::create_dir_all(&storage_root)?;

    let stream_id = "vp9-test";

    let mut cfg = Config::default();
    cfg.recorder.auto_streams = vec![stream_id.to_string()];
    cfg.recorder.max_recording_seconds = 0;
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
        "ffmpeg -re -f lavfi -i testsrc=size=640x360:rate=30 -pix_fmt yuv420p -c:v libvpx-vp9 -b:v 1000k -minrate 1000k -maxrate 1000k -g 30 -keyint_min 30 -speed 8 -tile-columns 0 -frame-parallel 0 -threads 4 -deadline realtime -strict experimental -f rtp \"rtp://{}\" -sdp_file {}",
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

    sleep(Duration::from_secs(RECORDING_DURATION_SECS)).await;

    client
        .delete(format!("http://{addr}{}", api::path::streams(stream_id)))
        .send()
        .await?;

    sleep(Duration::from_secs(POST_RECORDING_WAIT_SECS)).await;

    whip_future.abort();
    let _ = whip_future.await;

    let mut outputs = wait_for_video_outputs(&storage_root, stream_id).await?;

    assert!(outputs.manifest.exists(), "manifest.mpd not found");
    let manifest_content = fs::read_to_string(&outputs.manifest)?;
    assert!(
        manifest_content.contains("video"),
        "manifest should contain video track"
    );

    let init_segment = outputs
        .video_init_segment
        .as_ref()
        .expect("v_init.m4s path missing");
    assert!(init_segment.exists(), "v_init.m4s not found");
    let init_size = fs::metadata(init_segment)?.len();
    assert!(init_size > 100, "v_init.m4s too small: {} bytes", init_size);

    outputs.video_segments.sort();
    assert!(
        !outputs.video_segments.is_empty(),
        "expected at least one video segment, got {}",
        outputs.video_segments.len()
    );

    for seg_path in &outputs.video_segments {
        let metadata = fs::metadata(seg_path)?;
        assert!(
            metadata.len() > 500,
            "segment {:?} too small: {} bytes",
            seg_path.file_name(),
            metadata.len()
        );
    }

    assert!(
        outputs.audio_segments.is_empty(),
        "unexpected audio segments: {:?}",
        outputs.audio_segments
    );
    assert!(
        outputs.audio_init_segment.is_none(),
        "unexpected a_init.m4s present"
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

    let stream_id = "opus-test";

    let mut cfg = Config::default();
    cfg.recorder.auto_streams = vec![stream_id.to_string()];
    cfg.recorder.max_recording_seconds = 0;
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

    sleep(Duration::from_secs(RECORDING_DURATION_SECS)).await;

    client
        .delete(format!("http://{addr}{}", api::path::streams(stream_id)))
        .send()
        .await?;

    sleep(Duration::from_secs(POST_RECORDING_WAIT_SECS)).await;

    whip_future.abort();
    let _ = whip_future.await;

    let mut outputs = wait_for_audio_outputs(&storage_root, stream_id).await?;

    assert!(outputs.manifest.exists(), "manifest.mpd not found");
    let manifest_content = fs::read_to_string(&outputs.manifest)?;
    assert!(
        manifest_content.contains("contentType=\"audio\"") || manifest_content.contains("audio"),
        "manifest should contain audio track"
    );

    let audio_init = outputs
        .audio_init_segment
        .as_ref()
        .expect("a_init.m4s path missing");
    assert!(audio_init.exists(), "a_init.m4s not found");
    let init_size = fs::metadata(audio_init)?.len();
    assert!(init_size > 50, "a_init.m4s too small: {} bytes", init_size);

    outputs.audio_segments.sort();
    assert!(
        !outputs.audio_segments.is_empty(),
        "expected at least one audio segment, got {}",
        outputs.audio_segments.len()
    );

    for seg_path in &outputs.audio_segments {
        let metadata = fs::metadata(seg_path)?;
        assert!(
            metadata.len() > 100,
            "audio segment {:?} too small: {} bytes",
            seg_path.file_name(),
            metadata.len()
        );
    }

    assert!(
        outputs.video_segments.is_empty(),
        "unexpected video segments: {:?}",
        outputs.video_segments
    );
    assert!(
        outputs.video_init_segment.is_none(),
        "unexpected v_init.m4s present for audio-only recording"
    );

    Ok(())
}

async fn wait_for_publish_connected(
    client: &Client,
    addr: SocketAddr,
    stream_id: &str,
) -> anyhow::Result<()> {
    let max_attempts = (PUBLISH_WAIT_TIMEOUT_SECS * 1000) / 200;
    for attempt in 0..max_attempts {
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
            tracing::info!(
                "[test] publisher connected for stream '{}' after {} attempts",
                stream_id,
                attempt + 1
            );
            return Ok(());
        }

        sleep(Duration::from_millis(200)).await;
    }

    anyhow::bail!(
        "publisher never connected for stream '{}' within {} seconds",
        stream_id,
        PUBLISH_WAIT_TIMEOUT_SECS
    )
}

#[derive(Debug)]
struct RecordingOutputs {
    manifest: PathBuf,
    video_segments: Vec<PathBuf>,
    audio_segments: Vec<PathBuf>,
    video_init_segment: Option<PathBuf>,
    audio_init_segment: Option<PathBuf>,
    #[allow(dead_code)]
    recording_dir: PathBuf,
}

/// Collect recording outputs for a specific stream from storage root
fn collect_recording_outputs(root: &Path, stream_id: &str) -> RecordingOutputs {
    let stream_dir = root.join(stream_id);

    let mut manifest = None;
    let mut video_init_segment = None;
    let mut audio_init_segment = None;
    let mut video_segments = Vec::new();
    let mut audio_segments = Vec::new();
    let mut recording_dir = stream_dir.clone();

    // Find the timestamp subdirectory (latest one if multiple exist)
    if stream_dir.exists()
        && let Ok(entries) = fs::read_dir(&stream_dir)
    {
        let mut timestamp_dirs: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let path = e.path();
                path.is_dir()
                    && path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.chars().all(|c| c.is_ascii_digit()))
                        .unwrap_or(false)
            })
            .collect();

        timestamp_dirs.sort_by_key(|e| e.file_name());

        if let Some(last_dir) = timestamp_dirs.last() {
            recording_dir = last_dir.path();

            if let Ok(files) = fs::read_dir(&recording_dir) {
                for entry in files.flatten() {
                    let path = entry.path();
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        match name {
                            "manifest.mpd" => manifest = Some(path),
                            "v_init.m4s" => video_init_segment = Some(path),
                            "a_init.m4s" => audio_init_segment = Some(path),
                            _ => {
                                if name.starts_with("v_seg_") && name.ends_with(".m4s") {
                                    video_segments.push(path);
                                } else if name.starts_with("a_seg_") && name.ends_with(".m4s") {
                                    audio_segments.push(path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    RecordingOutputs {
        manifest: manifest.unwrap_or_else(|| recording_dir.join("manifest.mpd")),
        video_segments,
        audio_segments,
        video_init_segment,
        audio_init_segment,
        recording_dir,
    }
}
async fn wait_for_video_outputs(root: &Path, stream_id: &str) -> anyhow::Result<RecordingOutputs> {
    let max_attempts = (OUTPUT_COLLECTION_TIMEOUT_SECS * 1000) / 200;
    let mut last = None;

    for attempt in 0..max_attempts {
        let outputs = collect_recording_outputs(root, stream_id);
        let has_manifest = outputs.manifest.exists();
        let has_init = outputs
            .video_init_segment
            .as_ref()
            .is_some_and(|p| p.exists());
        let has_segment =
            !outputs.video_segments.is_empty() && outputs.video_segments.iter().all(|p| p.exists());

        if has_manifest && has_init && has_segment {
            tracing::info!(
                "[test] found video outputs for '{}' after {} attempts: manifest={}, init={}, segments={}",
                stream_id,
                attempt + 1,
                has_manifest,
                has_init,
                outputs.video_segments.len()
            );
            return Ok(outputs);
        }

        last = Some(outputs);
        sleep(Duration::from_millis(200)).await;
    }

    Err(anyhow::anyhow!(
        "timed out waiting for video outputs for stream '{}' within {} seconds: {:?}",
        stream_id,
        OUTPUT_COLLECTION_TIMEOUT_SECS,
        last
    ))
}

async fn wait_for_audio_outputs(root: &Path, stream_id: &str) -> anyhow::Result<RecordingOutputs> {
    let max_attempts = (OUTPUT_COLLECTION_TIMEOUT_SECS * 1000) / 200;
    let mut last = None;

    for attempt in 0..max_attempts {
        let outputs = collect_recording_outputs(root, stream_id);
        let has_manifest = outputs.manifest.exists();
        let has_init = outputs
            .audio_init_segment
            .as_ref()
            .is_some_and(|p| p.exists());
        let has_segment =
            !outputs.audio_segments.is_empty() && outputs.audio_segments.iter().all(|p| p.exists());

        if has_manifest && has_init && has_segment {
            tracing::info!(
                "[test] found audio outputs for '{}' after {} attempts: manifest={}, init={}, segments={}",
                stream_id,
                attempt + 1,
                has_manifest,
                has_init,
                outputs.audio_segments.len()
            );
            return Ok(outputs);
        }

        last = Some(outputs);
        sleep(Duration::from_millis(200)).await;
    }

    Err(anyhow::anyhow!(
        "timed out waiting for audio outputs for stream '{}' within {} seconds: {:?}",
        stream_id,
        OUTPUT_COLLECTION_TIMEOUT_SECS,
        last
    ))
}
