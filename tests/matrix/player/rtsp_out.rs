use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::{PlayResult, Player, parse_whep_url, wait_subscribe_connected};
use crate::probe;
use crate::profile::MediaProfile;
use crate::runner::RtspTransport;

/// WHEP player that uses `livetwo::whep::from` with an `rtsp-listen://`
/// output (the whepfrom RTSP server mode) and validates the stream by
/// pulling from that server with ffprobe.
#[derive(Debug, Clone, Copy)]
pub struct RtspOutPlayer {
    pub transport: RtspTransport,
}

impl RtspOutPlayer {
    pub fn new(transport: RtspTransport) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl Player for RtspOutPlayer {
    fn name(&self) -> &'static str {
        match self.transport {
            RtspTransport::Udp => "rtsp-out-udp",
            RtspTransport::Tcp => "rtsp-out-tcp",
        }
    }

    async fn play(&self, whep_url: &str, profile: &MediaProfile) -> Result<PlayResult> {
        let (base_url, stream_id) = parse_whep_url(whep_url)?;
        let whep_url = whep_url.to_string();

        // Reserve a port for the whepfrom RTSP server; released before
        // whepfrom binds it (TOCTOU trade-off, same as the RTSP input path).
        let rtsp_port = crate::runner::reserve_and_release_tcp_port(std::net::IpAddr::V4(
            std::net::Ipv4Addr::LOCALHOST,
        ));
        let output_url = format!("rtsp-listen://127.0.0.1:{rtsp_port}/stream");

        let ct = CancellationToken::new();
        let mut handle_whep = Some(tokio::spawn({
            let ct = ct.clone();
            async move { livetwo::whep::from(ct, output_url, whep_url, None, None, None, None).await }
        }));

        let start = tokio::time::Instant::now();
        let (connected, last_error) =
            wait_subscribe_connected(&base_url, &stream_id, &mut handle_whep).await;

        if !connected {
            ct.cancel();
            if let Some(handle) = handle_whep.take() {
                let _ = handle.await;
            }
            return Ok(PlayResult {
                success: false,
                connected: false,
                duration_ms: start.elapsed().as_millis() as u64,
                error: last_error.or_else(|| Some("subscribe did not connect".to_string())),
                ..Default::default()
            });
        }

        // Wait for the whepfrom RTSP server to accept connections.
        let rtsp_addr = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, rtsp_port));
        let mut server_up = false;
        for _ in 0..50 {
            match tokio::net::TcpStream::connect(rtsp_addr).await {
                Ok(_) => {
                    server_up = true;
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        if !server_up {
            ct.cancel();
            if let Some(handle) = handle_whep.take() {
                let _ = handle.await;
            }
            return Ok(PlayResult {
                success: false,
                connected: true,
                duration_ms: start.elapsed().as_millis() as u64,
                error: Some(format!(
                    "whepfrom RTSP server did not listen on {rtsp_addr}"
                )),
                ..Default::default()
            });
        }

        // Give the forwarder a moment to deliver media to the RTSP clients.
        tokio::time::sleep(Duration::from_millis(500)).await;

        let rtsp_url = format!("rtsp://127.0.0.1:{rtsp_port}/stream");
        let mut probe_args: Vec<&str> = self.transport.ffprobe_args().to_vec();
        probe_args.extend(["-i", rtsp_url.as_str()]);
        let probe_result = probe::run(&probe_args).await;

        ct.cancel();
        if let Some(handle) = handle_whep.take() {
            let _ = handle.await;
        }

        let duration_ms = start.elapsed().as_millis() as u64;
        match probe_result {
            Ok(probe_result) => Ok(probe::into_play_result(
                probe_result,
                profile,
                true,
                duration_ms,
            )),
            Err(e) => Ok(PlayResult {
                success: false,
                connected: true,
                duration_ms,
                error: Some(format!("ffprobe failed: {e:?}")),
                ..Default::default()
            }),
        }
    }
}
