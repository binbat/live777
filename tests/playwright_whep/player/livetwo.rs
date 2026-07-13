use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use super::{PlayResult, Player};

/// WHEP player that uses `livetwo::whep::from` and verifies the subscribe
/// session state through the liveion HTTP API. It also binds the output UDP
/// port so it can confirm that media packets actually arrived.
#[derive(Debug, Clone, Copy, Default)]
pub struct LivetwoWhepPlayer;

#[async_trait]
impl Player for LivetwoWhepPlayer {
    fn name(&self) -> &'static str {
        "livetwo"
    }

    async fn play(&self, whep_url: &str) -> Result<PlayResult> {
        let (base_url, stream_id) = parse_whep_url(whep_url)?;
        let whep_url = whep_url.to_string();

        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let socket = UdpSocket::bind(SocketAddr::new(ip, 0))
            .await
            .context("Failed to bind output UDP socket")?;
        let output_port = socket
            .local_addr()
            .context("Failed to read output UDP port")?
            .port();
        let output_url = format!("rtp://127.0.0.1?video={output_port}");

        let output_sdp =
            tempfile::NamedTempFile::new().context("Failed to create output SDP temp file")?;
        let output_sdp_path = output_sdp
            .path()
            .to_str()
            .ok_or_else(|| anyhow!("Invalid output SDP path"))?
            .to_string();

        let video_received = Arc::new(AtomicBool::new(false));
        let video_received_reader = video_received.clone();
        let socket_reader = socket;

        // Drain the output socket so we can confirm media actually flowed.
        let socket_handle = tokio::spawn(async move {
            let mut buf = [0u8; 1500];
            loop {
                match socket_reader.recv(&mut buf).await {
                    Ok(len) if len >= 12 => {
                        // Any RTP packet on the video port counts as a video track.
                        video_received_reader.store(true, Ordering::Relaxed);
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        });

        let ct = CancellationToken::new();
        let mut handle_whep = Some(tokio::spawn({
            let ct = ct.clone();
            async move {
                // Keep the output SDP file alive for the lifetime of the WHEP task.
                let _output_sdp = output_sdp;
                livetwo::whep::from(
                    ct,
                    output_url,
                    whep_url.to_string(),
                    Some(output_sdp_path),
                    None,
                    None,
                    None,
                )
                .await
            }
        }));

        let start = tokio::time::Instant::now();
        let mut connected = false;
        let mut last_error = None;

        for _ in 0..300 {
            if !connected {
                let res = reqwest::get(format!("{base_url}{}", api::path::streams("")))
                    .await
                    .context("Failed to query liveion streams")?;

                if res.status() != http::StatusCode::OK {
                    last_error = Some(format!("liveion returned {}", res.status()));
                    break;
                }

                let body = res.json::<Vec<api::response::Stream>>().await?;
                if let Some(stream) = body.into_iter().find(|s| s.id == stream_id)
                    && stream
                        .subscribe
                        .sessions
                        .iter()
                        .any(|s| s.state == api::response::RTCPeerConnectionState::Connected)
                {
                    connected = true;
                }
            }

            if connected && video_received.load(Ordering::Relaxed) {
                break;
            }

            if let Some(handle) = handle_whep.as_ref()
                && handle.is_finished()
            {
                match handle_whep.take().unwrap().await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => last_error = Some(format!("{e:?}")),
                    Err(e) => last_error = Some(format!("{e:?}")),
                }
                break;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        ct.cancel();
        if let Some(handle) = handle_whep.take() {
            let _ = handle.await;
        }
        drop(socket_handle);

        let video_tracks = if video_received.load(Ordering::Relaxed) {
            1
        } else {
            0
        };
        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(PlayResult {
            success: connected && video_tracks > 0,
            connected,
            video_width: 0,
            video_height: 0,
            video_tracks,
            audio_tracks: 0,
            duration_ms,
            error: last_error,
        })
    }
}

fn parse_whep_url(whep_url: &str) -> Result<(String, String)> {
    // Expected form: http://host:port/whep/<stream>
    let parsed = url::Url::parse(whep_url).context("Invalid WHEP URL")?;
    let base = format!(
        "{}://{}",
        parsed.scheme(),
        parsed.host_str().ok_or_else(|| anyhow!("Missing host"))?
    );
    let base = if let Some(port) = parsed.port() {
        format!("{base}:{port}")
    } else {
        base
    };

    let path = parsed.path();
    let stream_id = path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("Failed to parse stream id from WHEP URL"))?
        .to_string();

    Ok((base, stream_id))
}
