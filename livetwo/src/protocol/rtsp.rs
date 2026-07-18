use anyhow::Result;
use tracing::info;

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
}
