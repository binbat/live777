use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::{PlayResult, Player};

/// WHEP player that uses `livetwo::whep::from` and verifies the subscribe
/// session state through the liveion HTTP API.
#[derive(Debug, Clone, Copy, Default)]
pub struct LivetwoWhepPlayer;

#[async_trait]
impl Player for LivetwoWhepPlayer {
    fn name(&self) -> &'static str {
        "livetwo"
    }

    async fn play(&self, whep_url: &str) -> Result<PlayResult> {
        let (base_url, stream_id) = parse_whep_url(whep_url)?;

        let ip = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let output_port = pick_udp_port(ip);
        let output_url = format!("rtp://127.0.0.1?video={output_port}");

        let output_sdp =
            tempfile::NamedTempFile::new().context("Failed to create output SDP temp file")?;
        let output_sdp_path = output_sdp
            .path()
            .to_str()
            .ok_or_else(|| anyhow!("Invalid output SDP path"))?
            .to_string();

        let ct = CancellationToken::new();
        let mut handle_whep = Some(tokio::spawn(livetwo::whep::from(
            ct.clone(),
            output_url,
            whep_url.to_string(),
            Some(output_sdp_path),
            None,
            None,
            None,
        )));

        let mut connected = false;
        let mut last_error = None;

        for _ in 0..300 {
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

        Ok(PlayResult {
            success: connected,
            connected,
            video_width: 0,
            video_height: 0,
            video_tracks: 0,
            audio_tracks: 0,
            duration_ms: 0,
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

fn pick_udp_port(ip: IpAddr) -> u16 {
    let socket = UdpSocket::bind(SocketAddr::new(ip, 0)).expect("Failed to reserve UDP port");
    socket
        .local_addr()
        .expect("Failed to read temporary UDP port")
        .port()
}
