use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Result, anyhow};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;
use tracing::info;

pub struct RtspPullSession {
    pub connection_id: u32,
    pub media_info: rtsp::MediaInfo,
    pub channels: rtsp::channels::InterleavedChannel,
}

pub async fn setup_server_for_pull(
    ct: CancellationToken,
    listen_host: &str,
    port: u16,
    filtered_sdp: String,
) -> Result<(RtspPullSession, UnboundedReceiver<RtspPullSession>)> {
    info!("Starting RTSP server mode for pull");

    let listen_addr_str = crate::utils::host::format_bind_addr(listen_host, port);
    let listen_addr: SocketAddr = listen_addr_str.parse()?;
    let (session_tx, mut session_rx) = unbounded_channel();
    let handler = WhepRtspPullHandler {
        sdp: Arc::new(filtered_sdp.into_bytes()),
        session_tx,
        next_connection_id: Arc::new(AtomicU32::new(1)),
    };
    let server_config = rtsp::ServerConfig {
        listen_addr,
        ..Default::default()
    };

    // Bind synchronously so port-allocation failures surface as an error
    // to the caller immediately — no indefinite hang.
    let listener = tokio::net::TcpListener::bind(&listen_addr_str)
        .await
        .map_err(|e| {
            anyhow!(
                "Failed to bind RTSP pull server on {}: {}",
                listen_addr_str,
                e
            )
        })?;

    let server_ct = ct.clone();
    tokio::spawn(async move {
        if let Err(e) = rtsp::run_rtsp_server(
            listener,
            rtsp::SessionMode::Pull,
            handler,
            server_config,
            server_ct,
        )
        .await
        {
            tracing::error!("RTSP pull output server failed: {}", e);
        }
    });

    let first = tokio::select! {
        _ = ct.cancelled() => {
            return Err(anyhow!("RTSP pull server cancelled before first client connected"));
        }
        first = session_rx.recv() => {
            first.ok_or_else(|| anyhow!("RTSP pull output server stopped before first client connected"))?
        }
    };

    info!(
        "RTSP pull output established: connection_id={}, media_info={:?}",
        first.connection_id, first.media_info
    );

    Ok((first, session_rx))
}

#[derive(Clone)]
struct WhepRtspPullHandler {
    sdp: Arc<Vec<u8>>,
    session_tx: UnboundedSender<RtspPullSession>,
    next_connection_id: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl rtsp::SessionHandler for WhepRtspPullHandler {
    async fn on_announce(&self, _path: String, _sdp: Vec<u8>) -> Result<()> {
        Err(anyhow!("ANNOUNCE is not supported for WHEP RTSP output"))
    }

    async fn on_describe(&self, _path: String) -> Result<Vec<u8>> {
        Ok((*self.sdp).clone())
    }

    async fn on_session(
        &self,
        _path: String,
        mode: rtsp::SessionMode,
        media_info: rtsp::MediaInfo,
        endpoint: rtsp::SessionEndpoint,
        _cancel: CancellationToken,
    ) -> Result<()> {
        if mode != rtsp::SessionMode::Pull {
            return Err(anyhow!("Expected RTSP pull session"));
        }

        let (tx, rtcp_rx) = match endpoint {
            rtsp::SessionEndpoint::Pull(tx, rtcp_rx) => (tx, rtcp_rx),
            _ => return Err(anyhow!("Expected RTSP pull endpoint")),
        };

        // media_info is passed through unchanged so downstream consumers
        // can inspect the client-negotiated transport parameters.  Output
        // routing uses fixed channel numbers (udp_route::VIDEO_RTP etc.),
        // which are applied directly by TcpHandler::with_channels in the
        // transport layer.
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        self.session_tx
            .send(RtspPullSession {
                connection_id,
                media_info,
                channels: (tx, rtcp_rx),
            })
            .map_err(|_| anyhow!("RTSP output session receiver dropped"))?;

        Ok(())
    }
}

pub async fn setup_client_for_pull(
    target_url: &str,
    target_host: &str,
) -> Result<(rtsp::MediaInfo, Option<rtsp::channels::InterleavedChannel>)> {
    info!("Starting RTSP client mode for pull (WHIP)");

    let url = url::Url::parse(target_url)?;
    let use_tcp = url
        .query_pairs()
        .find(|(key, _)| key == "transport")
        .map(|(_, value)| value.to_lowercase() == "tcp")
        .unwrap_or(false);

    info!(
        "RTSP transport mode: {}",
        if use_tcp { "TCP" } else { "UDP" }
    );

    let mut clean_url = url.clone();
    clean_url.set_query(None);
    let clean_url_str = clean_url.to_string();

    let (media_info, channels) = rtsp::setup_rtsp_session(
        &clean_url_str,
        None,
        target_host,
        rtsp::RtspMode::Pull,
        use_tcp,
    )
    .await?;
    info!(
        "RTSP pull input established: media_info={:?}, interleaved={}",
        media_info,
        channels.is_some()
    );

    Ok((media_info, channels))
}

pub async fn setup_client_for_push(
    target_url: &str,
    target_host: &str,
    filtered_sdp: String,
) -> Result<(rtsp::MediaInfo, Option<rtsp::channels::InterleavedChannel>)> {
    info!("Starting RTSP client mode for push");

    let url = url::Url::parse(target_url)?;
    let use_tcp = url
        .query_pairs()
        .find(|(key, _)| key == "transport")
        .map(|(_, value)| value.to_lowercase() == "tcp")
        .unwrap_or(false);

    info!(
        "RTSP transport mode: {}",
        if use_tcp { "TCP" } else { "UDP" }
    );

    let mut clean_url = url.clone();
    clean_url.set_query(None);
    let clean_url_str = clean_url.to_string();

    let (media_info, channels) = rtsp::setup_rtsp_session(
        &clean_url_str,
        Some(filtered_sdp),
        target_host,
        rtsp::RtspMode::Push,
        use_tcp,
    )
    .await?;
    info!(
        "RTSP push output established: media_info={:?}, interleaved={}",
        media_info,
        channels.is_some()
    );

    Ok((media_info, channels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtsp::SessionHandler;

    #[test]
    fn rtsp_server_listen_addr_uses_brackets_for_ipv6() {
        assert_eq!(
            crate::utils::host::format_bind_addr("::", 8554),
            "[::]:8554"
        );
        assert_eq!(
            crate::utils::host::format_bind_addr("::1", 8554),
            "[::1]:8554"
        );
        assert_eq!(
            crate::utils::host::format_bind_addr("127.0.0.1", 8554),
            "127.0.0.1:8554"
        );
    }

    #[tokio::test]
    async fn rtsp_pull_handler_keeps_session_rtcp_receiver() {
        let (session_tx, mut session_rx) = unbounded_channel();
        let handler = WhepRtspPullHandler {
            sdp: Arc::new(Vec::new()),
            session_tx,
            next_connection_id: Arc::new(AtomicU32::new(1)),
        };
        let (rtp_tx, _rtp_rx) =
            tokio::sync::mpsc::channel(rtsp::channels::DEFAULT_CHANNEL_CAPACITY);
        let (rtcp_tx, rtcp_rx) =
            tokio::sync::mpsc::channel(rtsp::channels::DEFAULT_CHANNEL_CAPACITY);
        let cancel = CancellationToken::new();

        handler
            .on_session(
                "stream".to_string(),
                rtsp::SessionMode::Pull,
                rtsp::MediaInfo::default(),
                rtsp::SessionEndpoint::Pull(rtp_tx, rtcp_rx),
                cancel,
            )
            .await
            .unwrap();

        let mut session = session_rx.recv().await.unwrap();
        rtcp_tx
            .send((rtsp::udp_route::VIDEO_RTCP, vec![1, 2, 3]))
            .await
            .unwrap();

        let (channel, data) = session.channels.1.recv().await.unwrap();
        assert_eq!(channel, rtsp::udp_route::VIDEO_RTCP);
        assert_eq!(data, vec![1, 2, 3]);
    }
}
