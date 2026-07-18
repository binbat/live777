use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use std::process::ExitStatus;
use tokio_util::sync::CancellationToken;
use tracing::info;

pub mod core;
mod input;
mod track;
mod webrtc;

use crate::transport;
use crate::utils::shutdown::graceful_shutdown;
use crate::utils::stats::start_stats_monitor;
use cli::create_child;
use libwish::Client;

pub use core::format_ice_stats;
pub use input::InputSource;
pub(crate) use webrtc::log_rtcp_feedback_packet;
pub use webrtc::setup_whip_peer;

pub async fn into(
    ct: CancellationToken,
    target_url: String,
    whip_url: String,
    token: Option<String>,
    command: Option<String>,
) -> Result<()> {
    info!("Starting WHIP session: {}", target_url);

    // Synthetic input: generate frames in-process and publish them directly,
    // bypassing the RTP/RTSP input bridge entirely.
    #[cfg(feature = "rsmpeg")]
    if let Some(publisher) =
        crate::whipsynth::publisher_from_input(&target_url, whip_url.clone(), token.clone())?
    {
        if command.is_some() {
            anyhow::bail!("--command is not supported with a synthetic input");
        }
        let stats = publisher.run(ct).await?;
        info!(
            packets_sent = stats.packets_sent,
            bytes_sent = stats.bytes_sent,
            nack_count = stats.nack_count,
            pli_count = stats.pli_count,
            "Synthetic WHIP session ended"
        );
        return Ok(());
    }

    // Without the rsmpeg feature a synth:// input would otherwise fall
    // through to the RTP path and fail with a misleading SDP-file timeout.
    #[cfg(not(feature = "rsmpeg"))]
    if target_url.starts_with(&format!("{}://", crate::SCHEME_SYNTH)) {
        anyhow::bail!("synthetic input requires the rsmpeg feature");
    }

    let mut client = Client::new(whip_url.clone(), Client::get_auth_header_map(token.clone()));

    let child = Arc::new(create_child(command)?);

    let input_source = input::setup_input_source(ct.clone(), &target_url).await?;
    info!("Input source configured: {:?}", input_source.scheme());

    let (peer, video_sender, audio_sender, stats, peer_state_rx, peer_diagnostics) =
        webrtc::setup_whip_peer(
            ct.clone(),
            &mut client,
            input_source.media_info(),
            target_url.clone(),
        )
        .await?;
    info!("WHIP peer setup completed; waiting for WebRTC connection");

    core::wait_for_peer_connected(
        peer.clone(),
        peer_state_rx.clone(),
        peer_diagnostics.clone(),
    )
    .await?;
    info!("WebRTC peer connection connected");

    start_stats_monitor(ct.clone(), peer.clone(), stats.clone()).await;

    let stats_clone = stats.clone();
    let ct_clone = ct.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let summary = stats_clone.get_summary().await;
                    info!("{}", summary.format());
                }
                _ = ct_clone.cancelled() => {
                    info!("Stats reporter shutting down");
                    let final_summary = stats_clone.get_summary().await;
                    info!("Final Statistics:\n{}", final_summary.format());
                    break;
                }
            }
        }
    });

    transport::connect_input_to_webrtc(
        input_source,
        video_sender.clone(),
        audio_sender.clone(),
        peer.clone(),
    )
    .await?;

    info!("Input connected to WebRTC");

    if child.as_ref().is_some() {
        tokio::select! {
            _ = ct.cancelled() => {
                graceful_shutdown("WHIP", &mut client, peer).await;
                Ok(())
            }
            result = core::wait_for_unexpected_peer_end(peer.clone(), peer_state_rx, peer_diagnostics) => {
                ct.cancel();
                graceful_shutdown("WHIP", &mut client, peer).await;
                result
            }
            status = wait_for_child_exit(child.clone()) => {
                let status = status?;
                info!("Child process exited with status: {:?}", status);
                ct.cancel();
                graceful_shutdown("WHIP", &mut client, peer).await;
                Err(anyhow!("WHIP child process exited before shutdown: {status}"))
            }
        }
    } else {
        tokio::select! {
            _ = ct.cancelled() => {
                graceful_shutdown("WHIP", &mut client, peer).await;
                Ok(())
            }
            result = core::wait_for_unexpected_peer_end(peer.clone(), peer_state_rx, peer_diagnostics) => {
                ct.cancel();
                graceful_shutdown("WHIP", &mut client, peer).await;
                result
            }
        }
    }
}

async fn wait_for_child_exit(child: Arc<Option<cli::ChildGuard>>) -> Result<ExitStatus> {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;

        if let Some(child_guard_wrapper) = child.as_ref() {
            let status = child_guard_wrapper
                .lock()
                .map_err(|_| anyhow!("WHIP child process mutex poisoned"))?
                .try_wait()?;
            if let Some(status) = status {
                return Ok(status);
            }
        }
    }
}
