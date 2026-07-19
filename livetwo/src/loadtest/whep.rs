//! WHEP subscribe loadtest: N concurrent subscribers on one stream, exercising
//! the SFU's fan-out path. Media is forwarded to per-session UDP ports so the
//! full forwarding path is exercised.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use super::{LoadtestConfig, LoadtestStats, SessionMetrics, run_sessions};

/// Parameters shared by all subscribe sessions.
#[derive(Debug, Clone)]
pub struct WhepLoadParams {
    /// WHEP endpoint of the published stream, e.g. `http://localhost:7777/whep/live`.
    /// A publisher must be running on that stream (e.g. `loadtest whip`).
    pub whep_url: String,
    pub token: Option<String>,
}

/// Run `config.session_count` WHEP subscribers against `params.whep_url`.
pub async fn run(
    config: &LoadtestConfig,
    params: WhepLoadParams,
    ct: CancellationToken,
) -> Result<LoadtestStats> {
    let session_ct = ct.child_token();
    run_sessions(config, session_ct.clone(), move |_| {
        let params = params.clone();
        let run_ct = session_ct.child_token();
        async move {
            let port = portpicker::pick_unused_port()
                .context("no free UDP port for the subscribe output")?;
            let output = format!(
                "rtp://{}",
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
            );

            let start = std::time::Instant::now();
            crate::whep::from(
                run_ct,
                output,
                params.whep_url.clone(),
                None,
                params.token.clone(),
                None,
                None,
            )
            .await?;

            Ok(SessionMetrics {
                connected_duration: start.elapsed(),
                ..Default::default()
            })
        }
    })
    .await
}
