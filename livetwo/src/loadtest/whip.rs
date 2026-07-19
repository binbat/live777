//! WHIP publish loadtest: N concurrent synthetic publishers, each on its own
//! stream (derived from the base URL with a `-N` suffix).

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use super::{LoadtestConfig, LoadtestStats, SessionMetrics, run_sessions};
use crate::whipsynth::{Publisher, PublisherConfig};

/// Append a session index to the last path segment of a WHIP URL.
///
/// `http://localhost:7777/whip/live` with index `3` becomes
/// `http://localhost:7777/whip/live-3`.
pub fn session_whip_url(base_url: &str, index: usize) -> Result<String> {
    let mut url =
        url::Url::parse(base_url).with_context(|| format!("Invalid WHIP URL: {base_url}"))?;

    let mut segments: Vec<String> = url
        .path_segments()
        .map(|it| it.map(|s| s.to_string()).collect())
        .unwrap_or_default();

    if let Some(last) = segments.last_mut() {
        *last = format!("{}-{}", last, index);
    } else {
        segments.push(format!("session-{}", index));
    }

    let new_path = segments.join("/");
    url.set_path(&new_path);
    Ok(url.to_string())
}

/// Run `config.session_count` synthetic WHIP publishers concurrently.
pub async fn run(
    config: &LoadtestConfig,
    publisher_config: PublisherConfig,
    ct: CancellationToken,
) -> Result<LoadtestStats> {
    let urls: Vec<String> = (0..config.session_count)
        .map(|i| session_whip_url(&publisher_config.whip_url, i))
        .collect::<Result<_>>()?;

    let session_ct = ct.child_token();
    run_sessions(config, session_ct.clone(), move |i| {
        let mut session_config = publisher_config.clone();
        session_config.whip_url = urls[i].clone();
        let run_ct = session_ct.child_token();
        async move {
            let stats = Publisher::new(session_config).run(run_ct).await?;
            Ok(SessionMetrics {
                packets: stats.packets_sent,
                bytes: stats.bytes_sent,
                errors: stats.failed_writes,
                nack_count: stats.nack_count,
                pli_count: stats.pli_count,
                connected_duration: stats.connected_duration,
            })
        }
    })
    .await
}
