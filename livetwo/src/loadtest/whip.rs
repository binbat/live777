//! WHIP publish loadtest: N concurrent synthetic publishers, each on its own
//! stream (derived from the base URL with a `-N` suffix).

use anyhow::{Context, Result, anyhow};
use tokio_util::sync::CancellationToken;

use super::{LoadtestConfig, LoadtestStats, SessionMetrics, SessionOutcome, run_sessions};
use crate::whipsynth::{PublishOutcome, Publisher, PublisherConfig};

/// Append a session index to the last path segment of a WHIP URL.
///
/// `http://localhost:7777/whip/live` with index `3` becomes
/// `http://localhost:7777/whip/live-3`.
pub fn session_whip_url(base_url: &str, index: usize) -> Result<String> {
    let mut url =
        url::Url::parse(base_url).with_context(|| format!("Invalid WHIP URL: {base_url}"))?;

    // Empty segments (e.g. from a trailing slash) must not become the "last
    // segment", or the index would be appended to an empty name.
    let mut segments: Vec<String> = url
        .path_segments()
        .map(|it| {
            it.filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
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
            let whip_url = session_config.whip_url.clone();
            match Publisher::new(session_config).run(run_ct).await {
                Ok(PublishOutcome::Completed(stats)) => {
                    let metrics = SessionMetrics {
                        packets: stats.packets_sent,
                        bytes: stats.bytes_sent,
                        errors: stats.failed_writes,
                        nack_count: stats.nack_count,
                        pli_count: stats.pli_count,
                        connected_duration: stats.connected_duration,
                    };
                    // A connected synthetic publisher that sent nothing means
                    // the pipeline is broken (mirrors the WHEP-side zero-packet
                    // check).
                    if stats.packets_sent == 0 {
                        (
                            metrics,
                            Err(anyhow!(
                                "WHIP publisher connected but sent no packets to {whip_url}"
                            )),
                        )
                    } else {
                        (metrics, Ok(SessionOutcome::Connected))
                    }
                }
                // Cancelled before connecting: nothing was published, so there
                // is nothing to aggregate.
                Ok(PublishOutcome::Cancelled) => {
                    (SessionMetrics::default(), Ok(SessionOutcome::Cancelled))
                }
                // The publisher does not expose partial stats on failure, so
                // there is nothing to aggregate for a failed publish session.
                Err(e) => (SessionMetrics::default(), Err(e)),
            }
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_whip_url_appends_index_to_last_segment() {
        let url = session_whip_url("http://localhost:7777/whip/load", 3).unwrap();
        assert_eq!(url, "http://localhost:7777/whip/load-3");
    }

    #[test]
    fn session_whip_url_ignores_trailing_slash() {
        let url = session_whip_url("http://localhost:7777/whip/live/", 0).unwrap();
        assert_eq!(url, "http://localhost:7777/whip/live-0");
    }

    #[test]
    fn session_whip_url_preserves_query() {
        let url = session_whip_url("http://localhost:7777/whip/load?token=abc", 2).unwrap();
        assert_eq!(url, "http://localhost:7777/whip/load-2?token=abc");
    }

    #[test]
    fn session_whip_url_root_path_uses_fallback() {
        let url = session_whip_url("http://localhost:7777/", 1).unwrap();
        assert_eq!(url, "http://localhost:7777/session-1");
    }
}
