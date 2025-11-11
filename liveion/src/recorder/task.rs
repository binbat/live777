use std::sync::Arc;
use std::time::{Duration, Instant};

use super::RecordingInfo;
use crate::recorder::codec::Av1RtpParser;
use crate::recorder::codec::H265RtpParser;
use crate::recorder::codec::h264::H264RtpParser;
use crate::recorder::codec::opus::OpusRtpParser;
use crate::recorder::codec::vp9::Vp9RtpParser;
use crate::recorder::segmenter::Segmenter;
use crate::stream::manager::Manager;
use anyhow::{Result, anyhow};
use bytes::Bytes;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use webrtc::api::media_engine::{MIME_TYPE_AV1, MIME_TYPE_H264, MIME_TYPE_HEVC, MIME_TYPE_VP9};

pub struct RecordingTask {
    pub stream: String,
    pub info: RecordingInfo,
    handle: JoinHandle<()>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl RecordingTask {
    pub async fn spawn(
        manager: Arc<Manager>,
        stream: &str,
        path_prefix_override: Option<String>,
    ) -> Result<Self> {
        let stream_name = stream.to_string();

        // Get storage Operator
        let op = {
            let guard = crate::recorder::STORAGE.read().await;
            match guard.as_ref() {
                Some(op) => {
                    tracing::debug!(
                        "[recorder] obtained storage operator for stream {}",
                        stream_name
                    );
                    op.clone()
                }
                None => {
                    let err_msg =
                        format!("storage operator not initialized for stream {stream_name}");
                    tracing::error!("[recorder] {}", err_msg);
                    return Err(anyhow!(err_msg));
                }
            }
        };

        // Directory prefix, allow override; default to /<stream_id>/<record_id>
        // record_id unix timestamp(10)
        let generated_record_id = chrono::Utc::now().timestamp();
        let (path_prefix, override_provided) = if let Some(p) = path_prefix_override {
            (p, true)
        } else {
            (format!("{}/{}", stream_name, generated_record_id), false)
        };

        let derived_record_id = path_prefix
            .rsplit('/')
            .find(|segment| {
                !segment.is_empty()
                    && segment.len() >= 10
                    && segment.chars().all(|c| c.is_ascii_digit())
            })
            .and_then(|s| s.parse::<i64>().ok());

        let record_id = if override_provided {
            derived_record_id.unwrap_or(0)
        } else {
            generated_record_id
        };

        tracing::info!(
            "[recorder] initializing recording for stream {} with path prefix: {}",
            stream_name,
            path_prefix
        );

        // Initialize Segmenter
        let segmenter = match Segmenter::new(op, stream_name.clone(), path_prefix.clone()).await {
            Ok(seg) => {
                tracing::debug!(
                    "[recorder] segmenter initialized for stream {} at path {}",
                    stream_name,
                    path_prefix
                );
                seg
            }
            Err(e) => {
                tracing::error!(
                    "[recorder] failed to initialize segmenter for stream {}: {}",
                    stream_name,
                    e
                );
                return Err(e);
            }
        };

        // Obtain PeerForward from Manager
        let peer_forward_opt = manager.get_forward(&stream_name).await;

        let forward = peer_forward_opt.ok_or(anyhow!("stream forward not found"))?;

        // Subscribe to track change notifications to avoid polling
        let mut track_change_rx = forward.subscribe_tracks_change();

        // Wait for at least one media track (video preferred, audio fallback)
        let mut codec_mime_opt: Option<String> = None;
        let mut video_receiver_opt = None;
        let mut audio_receiver_opt = None;

        loop {
            if codec_mime_opt.is_none() {
                codec_mime_opt = forward.first_video_codec().await;
            }

            if codec_mime_opt.is_some() && video_receiver_opt.is_none() {
                video_receiver_opt = forward.subscribe_video_rtp().await;
            }

            if audio_receiver_opt.is_none() {
                audio_receiver_opt = forward.subscribe_audio_rtp().await;
            }

            let have_video = codec_mime_opt.is_some() && video_receiver_opt.is_some();
            let have_audio = audio_receiver_opt.is_some();

            if have_video || have_audio {
                break;
            }

            tracing::debug!(
                "[recorder] waiting for media tracks of stream {}",
                stream_name
            );
            if track_change_rx.recv().await.is_err() {
                return Err(anyhow!("forward closed while waiting for media tracks"));
            }
        }

        if let Some(codec) = codec_mime_opt.as_ref() {
            tracing::info!(
                "[recorder] stream {} use video codec {}",
                stream_name,
                codec
            );
        } else {
            tracing::info!("[recorder] stream {} is audio-only (Opus)", stream_name);
        }

        if audio_receiver_opt.is_some() {
            tracing::info!("[recorder] stream {} audio track detected", stream_name);
        }

        tracing::info!("[recorder] subscribed RTP for stream {}", stream_name);

        let stream_name_cloned = stream_name.clone();
        let forward_clone = forward.clone();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let handle = tokio::spawn(async move {
            let mut segmenter = segmenter;
            let mut video_rx_opt = video_receiver_opt;
            let mut audio_rx_opt = audio_receiver_opt;
            let mut codec_mime_opt = codec_mime_opt;

            let mut parser_h264 = H264RtpParser::new();
            let mut parser_h265 = H265RtpParser::new();
            let mut parser_av1 = Av1RtpParser::new();
            let mut parser_vp9 = Vp9RtpParser::new();
            let mut prev_ts_video: Option<u32> = None;

            let mut parser_audio = OpusRtpParser::new();
            let mut prev_ts_audio: Option<u32> = None;

            let mut frame_cnt_video: u64 = 0;
            let mut frame_cnt_audio: u64 = 0;
            let mut last_log = Instant::now();

            // Timer for checking keyframe request (video only)
            let mut keyframe_check_interval = tokio::time::interval(Duration::from_secs(1));
            let mut track_change_rx = forward_clone.subscribe_tracks_change();

            // Track PLI request success for logging
            let mut last_pli_log = Instant::now();

            loop {
                tokio::select! {
                    biased;
                    _ = &mut shutdown_rx => {
                        tracing::info!("[recorder] received stop signal for stream {}", stream_name_cloned);
                        break;
                    },
                    _ = keyframe_check_interval.tick(), if video_rx_opt.is_some() => {
                        if segmenter.should_request_keyframe()
                            && let Some(video_track) = forward_clone.first_video_track().await {
                            let ssrc = video_track.ssrc();
                            if let Err(e) = forward_clone.send_rtcp_to_publish(
                                crate::forward::rtcp::RtcpMessage::PictureLossIndication,
                                ssrc,
                            ).await {
                                tracing::warn!("[recorder] {} failed to send PLI: {:?}", stream_name_cloned, e);
                            } else {
                                // Record the PLI request in the backoff mechanism
                                segmenter.record_pli_request();

                                // Log PLI statistics periodically
                                if last_pli_log.elapsed() >= Duration::from_secs(30) {
                                    tracing::info!("[recorder] {} PLI stats: {}", stream_name_cloned, segmenter.pli_stats());
                                    last_pli_log = Instant::now();
                                } else {
                                    tracing::debug!("[recorder] {} sent PLI request for keyframe", stream_name_cloned);
                                }
                            }
                        }
                    },

                    result = async {
                        match video_rx_opt.as_mut() {
                            Some(rx) => rx.recv().await.ok(),
                            None => std::future::pending().await,
                        }
                    }, if video_rx_opt.is_some() => {
                        match result {
                            Some(packet) => {
                                let pkt_ts = packet.header.timestamp;

                                if codec_mime_opt.is_none() {
                                    codec_mime_opt = forward_clone.first_video_codec().await;
                                }

                                let Some(codec_mime) = codec_mime_opt.as_ref() else {
                                    continue;
                                };

                                if codec_mime.eq_ignore_ascii_case(MIME_TYPE_H264)
                                    && let Ok(Some((frame, _))) = parser_h264.push_packet(&packet)
                                {
                                    let duration_ticks: u32 = if let Some(prev) = prev_ts_video { pkt_ts.wrapping_sub(prev) } else { 3_000 };
                                    prev_ts_video = Some(pkt_ts);
                                    if let Err(e) = segmenter.push_h264(Bytes::from(frame), duration_ticks).await {
                                        tracing::warn!("[recorder] {} failed to process H264 frame (storage error?): {}", stream_name_cloned, e);
                                    }
                                    frame_cnt_video += 1;
                                } else if codec_mime.eq_ignore_ascii_case(MIME_TYPE_HEVC)
                                    && let Ok(Some((frame, is_keyframe))) = parser_h265.push_packet(&packet)
                                {
                                    let duration_ticks: u32 = if let Some(prev) = prev_ts_video { pkt_ts.wrapping_sub(prev) } else { 3_000 };
                                    prev_ts_video = Some(pkt_ts);
                                    if let Err(e) = segmenter.push_h265(frame.freeze(), duration_ticks).await {
                                        tracing::warn!("[recorder] {} failed to process H265 frame: {}", stream_name_cloned, e);
                                    } else if is_keyframe {
                                        tracing::trace!("[recorder] {} processed H265 keyframe", stream_name_cloned);
                                    }
                                    frame_cnt_video += 1;
                                } else if codec_mime.eq_ignore_ascii_case(MIME_TYPE_AV1)
                                    && let Ok(Some(frame)) = parser_av1.push_packet(&packet)
                                {
                                    let duration_ticks: u32 = if let Some(prev) = prev_ts_video { pkt_ts.wrapping_sub(prev) } else { 3_000 };
                                    prev_ts_video = Some(pkt_ts);
                                    //println!("[recorder][test] {} processed AV1 frame", stream_name_cloned);
                                    if let Err(e) = segmenter.push_av1(frame.freeze(), duration_ticks).await {
                                        tracing::warn!("[recorder] {} failed to process AV1 frame: {}", stream_name_cloned, e);
                                    }
                                    frame_cnt_video += 1;
                                } else if codec_mime.eq_ignore_ascii_case(MIME_TYPE_VP9)
                                    && let Ok(Some(frame)) = parser_vp9.push_packet(&packet)
                                {
                                    let duration_ticks: u32 = if let Some(prev) = prev_ts_video { pkt_ts.wrapping_sub(prev) } else { 3_000 };
                                    prev_ts_video = Some(pkt_ts);
                                    if let Err(e) = segmenter.push_vp9(Bytes::from(frame), duration_ticks).await {
                                        tracing::warn!("[recorder] {} failed to process VP9 frame: {}", stream_name_cloned, e);
                                    }
                                    frame_cnt_video += 1;
                                }
                            }
                            None => {
                                video_rx_opt = None;
                                prev_ts_video = None;
                            }
                        }
                    },

                    result = async {
                        match audio_rx_opt.as_mut() {
                            Some(rx) => rx.recv().await.ok(),
                            None => std::future::pending().await,
                        }
                    }, if audio_rx_opt.is_some() => {
                        match result {
                            Some(packet) => {
                                let (payload, pkt_ts) = match parser_audio.push_packet(&packet) {
                                    Ok(v) => v,
                                    Err(_) => continue,
                                };
                                let duration_ticks: u32 = if let Some(prev) = prev_ts_audio {
                                    pkt_ts.wrapping_sub(prev)
                                } else {
                                    960
                                };
                                prev_ts_audio = Some(pkt_ts);
                                if let Err(e) = segmenter.push_opus(Bytes::from(payload), duration_ticks).await {
                                    tracing::warn!("[recorder] {} failed to process Opus frame (storage error?): {}", stream_name_cloned, e);
                                }
                                frame_cnt_audio += 1;
                            }
                            None => {
                                audio_rx_opt = None;
                                prev_ts_audio = None;
                            }
                        }
                    },

                    change = async { track_change_rx.recv().await.is_ok() }, if video_rx_opt.is_none() => {
                        if !change && audio_rx_opt.is_none() {
                            break;
                        }

                        if codec_mime_opt.is_none() {
                            codec_mime_opt = forward_clone.first_video_codec().await;
                            if let Some(codec) = codec_mime_opt.as_ref() {
                                tracing::info!(
                                    "[recorder] stream {} detected video codec {}",
                                    stream_name_cloned,
                                    codec
                                );
                            }
                        }

                        if codec_mime_opt.is_some()
                            && video_rx_opt.is_none()
                            && let Some(rx) = forward_clone.subscribe_video_rtp().await
                        {
                            tracing::info!(
                                "[recorder] stream {} video track became available",
                                stream_name_cloned
                            );
                            video_rx_opt = Some(rx);
                        }
                    }
                }

                if video_rx_opt.is_none() && audio_rx_opt.is_none() {
                    break;
                }

                if last_log.elapsed() >= Duration::from_secs(5) {
                    match (video_rx_opt.is_some(), audio_rx_opt.is_some()) {
                        (true, true) => {
                            tracing::info!(
                                "[recorder] stream {} received {} video frames and {} audio packets in last 5s",
                                stream_name_cloned,
                                frame_cnt_video,
                                frame_cnt_audio
                            );
                            frame_cnt_audio = 0;
                        }
                        (true, false) => {
                            tracing::info!(
                                "[recorder] stream {} received {} video frames in last 5s",
                                stream_name_cloned,
                                frame_cnt_video
                            );
                            frame_cnt_audio = 0;
                        }
                        (false, true) => {
                            tracing::info!(
                                "[recorder] stream {} received {} audio packets in last 5s",
                                stream_name_cloned,
                                frame_cnt_audio
                            );
                            frame_cnt_audio = 0;
                        }
                        (false, false) => {}
                    }
                    frame_cnt_video = 0;
                    last_log = Instant::now();
                }
            }

            if let Err(e) = segmenter.flush().await {
                tracing::debug!("[recorder] {} flush error: {}", stream_name_cloned, e);
            }
        });

        let info = RecordingInfo {
            record_dir: path_prefix,
            record_id,
        };

        Ok(Self {
            stream: stream_name,
            info,
            handle,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub async fn stop(mut self) {
        let stream = std::mem::take(&mut self.stream);
        tracing::info!("[recorder] stopping recording for stream {}", stream);

        if let Some(tx) = self.shutdown_tx.take()
            && tx.send(()).is_err()
        {
            tracing::debug!(
                "[recorder] stop signal dropped for stream {} (task already ended)",
                stream
            );
        }

        match self.handle.await {
            Ok(()) => {
                tracing::info!("[recorder] recording task for stream {} completed", stream);
            }
            Err(e) => {
                if e.is_cancelled() {
                    tracing::warn!(
                        "[recorder] recording task for stream {} cancelled before completion",
                        stream
                    );
                } else {
                    tracing::error!(
                        "[recorder] recording task for stream {} exited with error: {}",
                        stream,
                        e
                    );
                }
            }
        }
    }
}
