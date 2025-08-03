use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::recorder::codec::h264::H264RtpParser;
use crate::recorder::codec::opus::OpusRtpParser;
use crate::recorder::segmenter::Segmenter;
use crate::stream::manager::Manager;
use anyhow::{anyhow, Result};
use bytes::Bytes;
use chrono::{Datelike, Utc};
use tokio::task::JoinHandle;
use webrtc::api::media_engine::MIME_TYPE_H264;

#[derive(Debug)]
pub struct RecordingTask {
    #[allow(dead_code)]
    pub stream: String,
    handle: JoinHandle<()>,
}

impl RecordingTask {
    pub async fn spawn(manager: Arc<Manager>, stream: &str) -> Result<Self> {
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

        // Generate directory prefix /<stream>/<yyyy>/<MM>/<DD>
        let now = Utc::now();
        let path_prefix = format!(
            "{}/{:04}/{:02}/{:02}",
            stream_name,
            now.year(),
            now.month(),
            now.day()
        );

        tracing::info!(
            "[recorder] initializing recording for stream {} with path prefix: {}",
            stream_name,
            path_prefix
        );

        // Initialize Segmenter
        let mut segmenter = match Segmenter::new(op, stream_name.clone(), path_prefix.clone()).await
        {
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

        // Wait for video track and obtain codec mime type without busy polling
        let codec_mime = loop {
            if let Some(c) = forward.first_video_codec().await {
                break c;
            }
            tracing::debug!(
                "[recorder] waiting for video codec of stream {}",
                stream_name
            );
            // Await next track-change notification; error indicates the channel is closed
            if track_change_rx.recv().await.is_err() {
                return Err(anyhow!("forward closed while waiting for video codec"));
            }
        };

        // Subscribe to video RTP after we know codec
        let mut rtp_receiver = loop {
            if let Some(rx) = forward.subscribe_video_rtp().await {
                break rx;
            }
            tracing::debug!(
                "[recorder] waiting for video track of stream {}",
                stream_name
            );
            if track_change_rx.recv().await.is_err() {
                return Err(anyhow!("forward closed while waiting for video track"));
            }
        };

        // Also subscribe to audio RTP (Opus) if available
        let audio_receiver_opt = forward.subscribe_audio_rtp().await;

        tracing::info!(
            "[recorder] stream {} use video codec {}",
            stream_name,
            codec_mime
        );
        if audio_receiver_opt.is_some() {
            tracing::info!("[recorder] stream {} audio track detected", stream_name);
        }

        tracing::info!("[recorder] subscribed RTP for stream {}", stream_name);

        let stream_name_cloned = stream_name.clone();
        let forward_clone = forward.clone();
        let handle = tokio::spawn(async move {
            let is_h264 = codec_mime.eq_ignore_ascii_case(MIME_TYPE_H264);
            let mut parser_video = H264RtpParser::new();
            let mut prev_ts_video: Option<u32> = None;

            let mut parser_audio = OpusRtpParser::new();
            let mut prev_ts_audio: Option<u32> = None;

            let mut frame_cnt_video: u64 = 0;
            let mut frame_cnt_audio: u64 = 0;
            let mut last_log = Instant::now();

            // Timer for checking keyframe request
            let mut keyframe_check_interval = tokio::time::interval(Duration::from_secs(1));

            // Unified loop that handles video RTP (mandatory) and audio RTP (optional).
            let mut audio_rx_opt = audio_receiver_opt;
            loop {
                tokio::select! {
                    // Periodic keyframe (PLI) check.
                    _ = keyframe_check_interval.tick() => {
                        if segmenter.should_request_keyframe() {
                            if let Some(video_track) = forward_clone.first_video_track().await {
                                let ssrc = video_track.ssrc();
                                if let Err(e) = forward_clone.send_rtcp_to_publish(
                                    crate::forward::rtcp::RtcpMessage::PictureLossIndication,
                                    ssrc,
                                ).await {
                                    tracing::warn!("[recorder] {} failed to send PLI: {:?}", stream_name_cloned, e);
                                } else {
                                    tracing::debug!("[recorder] {} sent PLI request for keyframe", stream_name_cloned);
                                }
                            }
                        }
                    },

                    // Handle video RTP packets.
                    result = rtp_receiver.recv() => {
                        let packet = match result {
                            Ok(packet) => packet,
                            Err(_) => break,
                        };

                        if !is_h264 {
                            continue;
                        }

                        let pkt_ts = packet.header.timestamp;
                        if let Ok(Some((frame, is_idr))) = parser_video.push_packet((*packet).clone()) {
                            let duration_ticks: u32 = if let Some(prev) = prev_ts_video {
                                pkt_ts.wrapping_sub(prev)
                            } else {
                                3_000 // assume 30fps for first frame
                            };

                            prev_ts_video = Some(pkt_ts);
                            if let Err(e) = segmenter.push_h264(Bytes::from(frame), is_idr, duration_ticks).await {
                                tracing::warn!("[recorder] {} failed to process H264 frame (storage error?): {}", stream_name_cloned, e);
                            }
                            frame_cnt_video += 1;
                        }
                    },

                    // Handle audio RTP packets when an audio receiver exists.
                    result = audio_rx_opt.as_mut().unwrap().recv(), if audio_rx_opt.is_some() => {
                        match result {
                            Ok(packet) => {
                                let (payload, pkt_ts) = match parser_audio.push_packet((*packet).clone()) {
                                    Ok(v) => v,
                                    Err(_) => continue,
                                };
                                let duration_ticks: u32 = if let Some(prev) = prev_ts_audio {
                                    pkt_ts.wrapping_sub(prev)
                                } else {
                                    960 // Opus 48kHz with 20ms frame size
                                };
                                prev_ts_audio = Some(pkt_ts);
                                if let Err(e) = segmenter.push_opus(Bytes::from(payload), duration_ticks).await {
                                    tracing::warn!("[recorder] {} failed to process Opus frame (storage error?): {}", stream_name_cloned, e);
                                }
                                frame_cnt_audio += 1;
                            }
                            Err(_) => {
                                // Audio channel closed — disable audio processing.
                                audio_rx_opt = None;
                            }
                        }
                    }
                }

                // Log statistics every 5 seconds.
                if last_log.elapsed() >= Duration::from_secs(5) {
                    if audio_rx_opt.is_some() {
                        tracing::info!(
                            "[recorder] stream {} received {} video frames and {} audio packets in last 5s",
                            stream_name_cloned,
                            frame_cnt_video,
                            frame_cnt_audio
                        );
                        frame_cnt_audio = 0;
                    } else {
                        tracing::info!(
                            "[recorder] stream {} received {} video frames in last 5s",
                            stream_name_cloned,
                            frame_cnt_video
                        );
                    }
                    frame_cnt_video = 0;
                    last_log = Instant::now();
                }
            }
        });

        Ok(Self {
            stream: stream_name,
            handle,
        })
    }

    pub fn stop(self) {
        self.handle.abort();
    }
}
