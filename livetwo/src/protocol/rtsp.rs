use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Notify;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;
use tracing::info;

pub async fn setup_server_for_push(
    _ct: CancellationToken,
    listen_host: &str,
    port: u16,
) -> Result<(
    rtsp::MediaInfo,
    Option<rtsp::channels::InterleavedChannel>,
    UnboundedReceiver<rtsp::PortUpdate>,
)> {
    info!("Starting RTSP server mode for push");

    let listen_addr = format!("{}:{}", listen_host, port);

    let (media_info, channels, port_update_rx) =
        rtsp::setup_rtsp_server_session(&listen_addr, Vec::new(), rtsp::SessionMode::Push, false)
            .await?;
    info!(
        "RTSP push input established: media_info={:?}, interleaved={}",
        media_info,
        channels.is_some()
    );

    Ok((media_info, channels, port_update_rx))
}

pub async fn setup_server_for_pull(
    _ct: CancellationToken,
    listen_host: &str,
    port: u16,
    filtered_sdp: String,
    _notify: Arc<Notify>,
) -> Result<(
    rtsp::MediaInfo,
    Option<rtsp::channels::InterleavedChannel>,
    UnboundedReceiver<rtsp::PortUpdate>,
)> {
    info!("Starting RTSP server mode for pull");

    let listen_addr = format!("{}:{}", listen_host, port);
    let sdp_bytes = filtered_sdp.into_bytes();

    let (media_info, channels, port_update_rx) =
        rtsp::setup_rtsp_server_session(&listen_addr, sdp_bytes, rtsp::SessionMode::Pull, true)
            .await?;
    info!(
        "RTSP pull output established: media_info={:?}, interleaved={}",
        media_info,
        channels.is_some()
    );

    Ok((media_info, channels, port_update_rx))
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
