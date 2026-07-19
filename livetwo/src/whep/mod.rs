mod channel;
mod output;
mod webrtc;

use anyhow::{Result, anyhow};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use ::webrtc::peer_connection::{PeerConnection, RTCPeerConnectionState};
use cli::create_child;
use libwish::Client;
use tokio::sync::{Notify, watch};

use crate::transport;
use crate::utils;
use crate::utils::shutdown::graceful_shutdown;
use crate::utils::stats::start_stats_monitor;
use rtsp::constants::media_type;

pub use output::OutputTarget;
pub use webrtc::setup_whep_peer;

const OUTPUT_CHANNEL_CAPACITY: usize = 512;

pub async fn from(
    ct: CancellationToken,
    target_url: String,
    whep_url: String,
    sdp_file: Option<String>,
    token: Option<String>,
    command: Option<String>,
    channel_url: Option<String>,
) -> Result<()> {
    from_with_state(
        ct,
        target_url,
        whep_url,
        sdp_file,
        token,
        command,
        channel_url,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn from_with_state(
    ct: CancellationToken,
    target_url: String,
    whep_url: String,
    sdp_file: Option<String>,
    token: Option<String>,
    command: Option<String>,
    channel_url: Option<String>,
    state_tx: Option<watch::Sender<RTCPeerConnectionState>>,
) -> Result<()> {
    info!("Starting WHEP session: {}", target_url);

    // Use bounded channels so a slow consumer cannot cause unbounded memory
    // growth. The track handlers drop packets when the channel is full. A
    // capacity of 512 gives high-bitrate streams more headroom than the
    // previous 128 without allowing unbounded buffering.
    const MEDIA_CHANNEL_CAPACITY: usize = 512;
    let (video_send, mut video_recv) =
        tokio::sync::mpsc::channel::<Vec<u8>>(MEDIA_CHANNEL_CAPACITY);
    let (audio_send, mut audio_recv) =
        tokio::sync::mpsc::channel::<Vec<u8>>(MEDIA_CHANNEL_CAPACITY);
    let codec_info = Arc::new(tokio::sync::Mutex::new(rtsp::CodecInfo::new()));

    let mut client = Client::new(whep_url.clone(), Client::get_auth_header_map(token.clone()));

    let (peer, answer, stats, dc_recv_rx, dc_send_tx) = webrtc::setup_whep_peer(
        ct.clone(),
        &mut client,
        video_send,
        audio_send,
        codec_info.clone(),
        state_tx,
        None,
    )
    .await?;
    info!("WebRTC peer connection established");

    // Start DataChannel <-> UDP forwarding if channel_url is configured
    if let Some(url) = channel_url {
        debug!("Starting DataChannel <-> UDP forwarding: {}", url);
        channel::spawn_channel(url, dc_recv_rx, dc_send_tx).await?;
    }

    start_stats_monitor(ct.clone(), peer.clone(), stats.clone()).await;

    let stats_clone = stats.clone();
    let ct_clone = ct.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let summary: crate::utils::stats::StatsSummary = stats_clone.get_summary().await;
                    info!("{}", summary.format());
                }
                _ = ct_clone.cancelled() => {
                    info!("Stats reporter shutting down");
                    let final_summary: crate::utils::stats::StatsSummary = stats_clone.get_summary().await;
                    info!("Final Statistics:\n{}", final_summary.format());
                    break;
                }
            }
        }
    });

    let (expected_video, expected_audio) = expected_media_kinds(&answer.sdp);
    let codec_info = tokio::select! {
        _ = ct.cancelled() => {
            graceful_shutdown("WHEP", &mut client, peer).await;
            return Ok(());
        }
        result = wait_for_codec_info(
            codec_info.clone(),
            &target_url,
            &whep_url,
            expected_video,
            expected_audio,
        ) => result?,
    };
    debug!("Codec info: {:?}", codec_info);

    let (video_broadcast_tx, _) = broadcast::channel::<Vec<u8>>(1000);
    let (audio_broadcast_tx, _) = broadcast::channel::<Vec<u8>>(1000);

    let video_broadcast_tx = Arc::new(video_broadcast_tx);
    let audio_broadcast_tx = Arc::new(audio_broadcast_tx);

    let video_broadcast_tx_clone = video_broadcast_tx.clone();
    let ct_clone = ct.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(data) = video_recv.recv() => {
                    let _ = video_broadcast_tx_clone.send(data);
                }
                _ = ct_clone.cancelled() => {
                    info!("Video broadcast forwarder shutting down");
                    break;
                }
            }
        }
    });

    let audio_broadcast_tx_clone = audio_broadcast_tx.clone();
    let ct_clone = ct.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(data) = audio_recv.recv() => {
                    let _ = audio_broadcast_tx_clone.send(data);
                }
                _ = ct_clone.cancelled() => {
                    info!("Audio broadcast forwarder shutting down");
                    break;
                }
            }
        }
    });

    let notify = Arc::new(Notify::new());
    let target_url_clone = target_url.clone();
    let answer_sdp = answer.sdp.clone();
    let codec_info_clone = codec_info.clone();
    let notify_clone = notify.clone();
    let sdp_file_clone = sdp_file.clone();

    let ct_clone = ct.clone();
    let output_handle = tokio::spawn(async move {
        output::setup_output_target(
            ct_clone.clone(),
            &target_url_clone,
            &answer_sdp,
            sdp_file_clone,
            &codec_info_clone,
            notify_clone,
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(500)).await;

    let child = Arc::new(create_child(command)?);

    let output_target = output_handle.await??;
    info!("Output target configured: {:?}", output_target.scheme());

    let _transport_handle = start_initial_transport_task(
        ct.clone(),
        1,
        video_broadcast_tx.subscribe(),
        audio_broadcast_tx.subscribe(),
        output_target,
        peer.clone(),
    );

    if child.as_ref().is_some() {
        let child_clone = child.clone();
        let ct_clone = ct.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        if let Some(child_mutex) = child_clone.as_ref()
                            && let Ok(mut child_guard) = child_mutex.lock()
                                && let Ok(Some(status)) = child_guard.try_wait() {
                                    info!("Child process exited with status: {:?}", status);
                                    ct_clone.cancel();
                                    break;
                                }


                    }
                    _ = ct_clone.cancelled() => {
                        info!("Child monitor shutting down");
                        break;
                    }
                }
            }
        });
    }

    ct.cancelled().await;
    graceful_shutdown("WHEP", &mut client, peer).await;

    Ok(())
}

/// Derive the negotiated track kinds from the WHEP answer SDP, so the codec
/// wait below can require every negotiated track instead of the first one
/// that happens to arrive.
fn expected_media_kinds(answer_sdp: &str) -> (bool, bool) {
    let mut video = false;
    let mut audio = false;
    for line in answer_sdp.lines() {
        let line = line.trim_start();
        if line.starts_with("m=video") {
            video = true;
        } else if line.starts_with("m=audio") {
            audio = true;
        }
    }
    (video, audio)
}

async fn wait_for_codec_info(
    codec_info: Arc<tokio::sync::Mutex<rtsp::CodecInfo>>,
    target_url: &str,
    whep_url: &str,
    expected_video: bool,
    expected_audio: bool,
) -> Result<rtsp::CodecInfo> {
    const CODEC_WAIT_ATTEMPTS: usize = 300;

    let input = utils::parse_input_url(target_url)?;
    let has_video_param = input.query_pairs().any(|(k, _)| k == media_type::VIDEO);
    let has_audio_param = input.query_pairs().any(|(k, _)| k == media_type::AUDIO);
    let has_any_media_param = has_video_param || has_audio_param;

    for _ in 0..CODEC_WAIT_ATTEMPTS {
        let info = codec_info.lock().await.clone();
        let video_ready = info.video_codec.is_some();
        let audio_ready = info.audio_codec.is_some();

        if codec_wait_satisfied(
            video_ready,
            audio_ready,
            has_any_media_param,
            has_video_param,
            has_audio_param,
            expected_video,
            expected_audio,
        ) {
            return Ok(info);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let info = codec_info.lock().await.clone();
    Err(anyhow!(
        "No WHEP media codec observed after {}ms; target_url={target_url}, whep_url={whep_url}, last_codec_info={info:?}",
        CODEC_WAIT_ATTEMPTS * 100
    ))
}

fn codec_wait_satisfied(
    video_ready: bool,
    audio_ready: bool,
    has_any_media_param: bool,
    has_video_param: bool,
    has_audio_param: bool,
    expected_video: bool,
    expected_audio: bool,
) -> bool {
    if has_any_media_param {
        // Explicit output filters are authoritative: wait for every requested
        // codec so filter_sdp can keep the requested media section(s).
        (!has_video_param || video_ready) && (!has_audio_param || audio_ready)
    } else if expected_video || expected_audio {
        // With no explicit media params, the negotiated answer tells us which
        // tracks should be forwarded. Returning partial codec info here would
        // permanently drop the later track from the filtered output SDP.
        (!expected_video || video_ready) && (!expected_audio || audio_ready)
    } else {
        // Defensive fallback for an answer SDP we cannot classify.
        video_ready || audio_ready
    }
}

fn start_initial_transport_task(
    ct: CancellationToken,
    connection_id: u32,
    mut video_rx: broadcast::Receiver<Vec<u8>>,
    mut audio_rx: broadcast::Receiver<Vec<u8>>,
    output_target: OutputTarget,
    peer: Arc<dyn PeerConnection>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("Transport task #{} started", connection_id);

        let (video_tx, video_rx_bounded) = mpsc::channel(OUTPUT_CHANNEL_CAPACITY);
        let (audio_tx, audio_rx_bounded) = mpsc::channel(OUTPUT_CHANNEL_CAPACITY);

        let ct_clone = ct.clone();
        let video_forwarder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = video_rx.recv() => {
                        match result {
                            Ok(data) => {
                                if video_tx.send(data).await.is_err() {
                                    info!("Connection #{} video channel closed", connection_id);
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(
                                    "Connection #{} video lagged by {} messages",
                                    connection_id, n
                                );
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                info!("Connection #{} video broadcast closed", connection_id);
                                break;
                            }
                        }
                    }
                    _ = ct_clone.cancelled() => {
                        info!("Connection #{} video forwarder shutting down", connection_id);
                        break;
                    }
                }
            }
        });

        let ct_clone = ct.clone();
        let audio_forwarder = tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = audio_rx.recv() => {
                        match result {
                            Ok(data) => {
                                if audio_tx.send(data).await.is_err() {
                                    info!("Connection #{} audio channel closed", connection_id);
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(
                                    "Connection #{} audio lagged by {} messages",
                                    connection_id, n
                                );
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                info!("Connection #{} audio broadcast closed", connection_id);
                                break;
                            }
                        }
                    }
                    _ = ct_clone.cancelled() => {
                        info!("Connection #{} audio forwarder shutting down", connection_id);
                        break;
                    }
                }
            }
        });

        transport::connect_webrtc_to_output(
            video_rx_bounded,
            audio_rx_bounded,
            output_target,
            peer,
        )
        .await;

        let _ = tokio::join!(video_forwarder, audio_forwarder);

        info!("Transport task #{} stopped", connection_id);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_wait_requires_every_negotiated_track_without_output_filters() {
        assert!(!codec_wait_satisfied(
            true, false, false, false, false, true, true,
        ));
        assert!(!codec_wait_satisfied(
            false, true, false, false, false, true, true,
        ));
        assert!(codec_wait_satisfied(
            true, true, false, false, false, true, true,
        ));
    }

    #[test]
    fn codec_wait_uses_single_negotiated_track_without_waiting_for_missing_kind() {
        assert!(codec_wait_satisfied(
            true, false, false, false, false, true, false,
        ));
        assert!(codec_wait_satisfied(
            false, true, false, false, false, false, true,
        ));
    }

    #[test]
    fn codec_wait_honors_explicit_output_filters() {
        assert!(codec_wait_satisfied(
            true, false, true, true, false, true, true,
        ));
        assert!(!codec_wait_satisfied(
            false, true, true, true, false, true, true,
        ));
        assert!(codec_wait_satisfied(
            false, true, true, false, true, true, true,
        ));
        assert!(!codec_wait_satisfied(
            true, false, true, false, true, true, true,
        ));
    }

    #[test]
    fn codec_wait_falls_back_to_any_codec_when_answer_has_no_media_kinds() {
        assert!(codec_wait_satisfied(
            true, false, false, false, false, false, false,
        ));
        assert!(codec_wait_satisfied(
            false, true, false, false, false, false, false,
        ));
        assert!(!codec_wait_satisfied(
            false, false, false, false, false, false, false,
        ));
    }
}
