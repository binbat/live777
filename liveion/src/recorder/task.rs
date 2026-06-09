use std::sync::Arc;
use std::time::{Duration, Instant};

use super::RecordingInfo;
use crate::recorder::codec::Av1RtpParser;
use crate::recorder::codec::H265RtpParser;
use crate::recorder::codec::h264::H264RtpParser;
use crate::recorder::codec::opus::OpusRtpParser;
use crate::recorder::codec::vp9::Vp9RtpParser;
use crate::recorder::segmenter::{RecordingMediaOutcome, Segmenter};
use crate::stream::manager::Manager;
use anyhow::{Result, anyhow};
use api::recorder::RecordingStatus;
use bytes::Bytes;
use chrono::Utc;
use opendal::Operator;
use rtc::peer_connection::configuration::media_engine::{
    MIME_TYPE_AV1, MIME_TYPE_H264, MIME_TYPE_HEVC, MIME_TYPE_VP9,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub struct RecordingTask {
    pub stream: String,
    pub info: RecordingInfo,
    started_at: Instant,
    base_dir_override: Option<String>,
    op: Operator,
    handle: JoinHandle<RecordingMediaOutcome>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

pub struct RecordingStopOutcome {
    pub status: RecordingStatus,
    pub end_ts: i64,
    pub duration_ms: i32,
}

impl RecordingTask {
    pub async fn spawn(
        manager: Arc<Manager>,
        stream: &str,
        path_prefix_override: Option<String>,
        uploader: Option<Arc<crate::recorder::uploader::UploadManager>>,
        local_dir: Option<String>,
    ) -> Result<Self> {
        let stream_name = stream.to_string();
        let base_dir_override = path_prefix_override;

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
        let (path_prefix, override_provided) = if let Some(ref p) = base_dir_override {
            (p.clone(), true)
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
        let mut segmenter = match Segmenter::new(
            op.clone(),
            stream_name.clone(),
            path_prefix.clone(),
            uploader,
            local_dir,
        )
        .await
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

        // Wait for at least one media track (video preferred, audio fallback)
        let mut video_track_info_opt = None;
        let mut codec_mime_opt: Option<String> = None;
        let mut video_receiver_opt = None;
        let mut audio_receiver_opt = None;

        loop {
            if video_track_info_opt.is_none() {
                video_track_info_opt = forward.first_video_track_info().await;
                if let Some(info) = video_track_info_opt.as_ref() {
                    codec_mime_opt = Some(info.codec_mime.clone());
                }
            }

            if video_track_info_opt.is_some() && video_receiver_opt.is_none() {
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
            if !is_supported_video_codec(codec) {
                tracing::warn!(
                    "[recorder] stream {} video codec {} is not supported by recorder; recording will only succeed if audio is written",
                    stream_name,
                    codec
                );
            }
        } else {
            tracing::info!("[recorder] stream {} is audio-only (Opus)", stream_name);
        }

        if video_receiver_opt.is_some() {
            let info = match video_track_info_opt.take() {
                Some(info) => Some(info),
                None => forward.first_video_track_info().await,
            };
            if let Some(info) = info {
                codec_mime_opt = Some(info.codec_mime.clone());
                segmenter.expect_video_track(
                    info.codec_mime.clone(),
                    info.payload_type,
                    info.ssrc,
                    Some(info.fmtp.as_str()),
                );
                segmenter.configure_video_from_track_metadata(&info.codec_mime, None, None);
                tracing::info!(
                    "[recorder] stream {} video track detected codec={} payload_type={:?} ssrc={:?}",
                    stream_name,
                    info.codec_mime,
                    info.payload_type,
                    info.ssrc
                );
            }
        }

        if audio_receiver_opt.is_some() {
            tracing::info!("[recorder] stream {} audio track detected", stream_name);
        }

        if audio_receiver_opt.is_some()
            && let Some(info) = forward.first_audio_track_info().await
        {
            let crate::forward::AudioTrackInfo {
                clock_rate,
                channels,
                codec_mime,
                fmtp,
            } = info;
            let fmtp_opt = if fmtp.trim().is_empty() {
                None
            } else {
                Some(fmtp.as_str())
            };
            segmenter.configure_audio_track(clock_rate, channels, codec_mime, fmtp_opt);
        }

        tracing::info!(
            "[recorder] subscribed RTP for stream {} video={} audio={}",
            stream_name,
            video_receiver_opt.is_some(),
            audio_receiver_opt.is_some()
        );

        let stream_name_cloned = stream_name.clone();
        let forward_clone = forward.clone();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let manifest_path = format!("{}/manifest.mpd", path_prefix);
        let op_for_task = op.clone();

        let handle = tokio::spawn(async move {
            let mut segmenter = segmenter;
            let mut video_rx_opt = video_receiver_opt;
            let mut audio_rx_opt = audio_receiver_opt;
            let mut codec_mime_opt = codec_mime_opt;
            let mut unsupported_video_codec_logged = false;
            let mut missing_video_codec_logged = false;

            let mut parser_h264 = H264RtpParser::new();
            let mut parser_h265 = H265RtpParser::new();
            let mut parser_av1 = Av1RtpParser::new();
            let mut parser_vp9 = Vp9RtpParser::new();
            let mut prev_ts_video: Option<u32> = None;

            let mut parser_audio = OpusRtpParser::new();
            let mut prev_ts_audio: Option<u32> = None;

            let mut frame_cnt_video: u64 = 0;
            let mut frame_cnt_audio: u64 = 0;
            let mut rtp_cnt_video: u64 = 0;
            let mut parser_pending_cnt_video: u64 = 0;
            let mut parser_error_cnt_video: u64 = 0;
            let mut last_log = Instant::now();

            // Timer for checking keyframe request (video only)
            let mut keyframe_check_interval = tokio::time::interval(Duration::from_secs(1));
            let mut track_change_rx = forward_clone.subscribe_tracks_change();

            // Track PLI request success for logging
            let mut last_pli_log = Instant::now();
            if video_rx_opt.is_some() {
                request_keyframe(&forward_clone, &stream_name_cloned, &mut segmenter).await;
            }

            loop {
                tokio::select! {
                    biased;
                    _ = &mut shutdown_rx => {
                        tracing::info!("[recorder] received stop signal for stream {}", stream_name_cloned);
                        break;
                    },
                    _ = keyframe_check_interval.tick(), if video_rx_opt.is_some() => {
                        if segmenter.should_request_keyframe() {
                            request_keyframe(&forward_clone, &stream_name_cloned, &mut segmenter).await;
                            if last_pli_log.elapsed() >= Duration::from_secs(30) {
                                tracing::info!("[recorder] {} PLI stats: {}", stream_name_cloned, segmenter.pli_stats());
                                last_pli_log = Instant::now();
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
                                rtp_cnt_video += 1;
                                let pkt_ts = packet.header.timestamp;
                                segmenter.record_video_rtp_packet(
                                    packet.header.marker,
                                    pkt_ts,
                                    packet.payload.len(),
                                );

                                if codec_mime_opt.is_none() {
                                    codec_mime_opt = forward_clone
                                        .first_video_track_info()
                                        .await
                                        .map(|info| info.codec_mime);
                                }

                                let Some(codec_mime) = codec_mime_opt.as_ref() else {
                                    if !missing_video_codec_logged {
                                        tracing::warn!(
                                            "[recorder] {} received video RTP but video codec metadata is unavailable; payload_type={} ssrc={}",
                                            stream_name_cloned,
                                            packet.header.payload_type,
                                            packet.header.ssrc
                                        );
                                        missing_video_codec_logged = true;
                                    }
                                    continue;
                                };

                                let duration_ticks: u32 = if let Some(prev) = prev_ts_video {
                                    pkt_ts.wrapping_sub(prev)
                                } else {
                                    3_000
                                };

                                let mut frame_written = false;
                                let mut attempted_video_parser = false;
                                if codec_mime.eq_ignore_ascii_case(MIME_TYPE_H264) {
                                    attempted_video_parser = true;
                                    match parser_h264.push_packet(&packet) {
                                        Ok(Some((frame, _))) => {
                                            prev_ts_video = Some(pkt_ts);
                                            if let Err(e) = segmenter.push_h264(Bytes::from(frame), duration_ticks).await {
                                                tracing::warn!("[recorder] {} failed to process H264 frame (storage error?): {}", stream_name_cloned, e);
                                            }
                                            frame_written = true;
                                        }
                                        Ok(None) => parser_pending_cnt_video += 1,
                                        Err(e) => {
                                            parser_error_cnt_video += 1;
                                            log_video_parser_error(&stream_name_cloned, codec_mime, parser_error_cnt_video, rtp_cnt_video, &packet, &e);
                                        }
                                    }
                                } else if codec_mime.eq_ignore_ascii_case(MIME_TYPE_HEVC) {
                                    attempted_video_parser = true;
                                    match parser_h265.push_packet(&packet) {
                                        Ok(Some((frame, is_keyframe))) => {
                                            prev_ts_video = Some(pkt_ts);
                                            if let Err(e) = segmenter.push_h265(frame.freeze(), duration_ticks).await {
                                                tracing::warn!("[recorder] {} failed to process H265 frame: {}", stream_name_cloned, e);
                                            } else if is_keyframe {
                                                tracing::trace!("[recorder] {} processed H265 keyframe", stream_name_cloned);
                                            }
                                            frame_written = true;
                                        }
                                        Ok(None) => parser_pending_cnt_video += 1,
                                        Err(e) => {
                                            parser_error_cnt_video += 1;
                                            log_video_parser_error(&stream_name_cloned, codec_mime, parser_error_cnt_video, rtp_cnt_video, &packet, &e);
                                        }
                                    }
                                } else if codec_mime.eq_ignore_ascii_case(MIME_TYPE_AV1) {
                                    attempted_video_parser = true;
                                    match parser_av1.push_packet(&packet) {
                                        Ok(Some(frame)) => {
                                            prev_ts_video = Some(pkt_ts);
                                            if let Err(e) = segmenter.push_av1(frame.freeze(), duration_ticks).await {
                                                tracing::warn!("[recorder] {} failed to process AV1 frame: {}", stream_name_cloned, e);
                                            }
                                            frame_written = true;
                                        }
                                        Ok(None) => parser_pending_cnt_video += 1,
                                        Err(e) => {
                                            parser_error_cnt_video += 1;
                                            log_video_parser_error(&stream_name_cloned, codec_mime, parser_error_cnt_video, rtp_cnt_video, &packet, &e);
                                        }
                                    }
                                } else if codec_mime.eq_ignore_ascii_case(MIME_TYPE_VP9) {
                                    attempted_video_parser = true;
                                    match parser_vp9.push_packet(&packet) {
                                        Ok(Some(frame)) => {
                                            prev_ts_video = Some(pkt_ts);
                                            if let Err(e) = segmenter.push_vp9(Bytes::from(frame), duration_ticks).await {
                                                tracing::warn!("[recorder] {} failed to process VP9 frame: {}", stream_name_cloned, e);
                                            }
                                            frame_written = true;
                                        }
                                        Ok(None) => parser_pending_cnt_video += 1,
                                        Err(e) => {
                                            parser_error_cnt_video += 1;
                                            log_video_parser_error(&stream_name_cloned, codec_mime, parser_error_cnt_video, rtp_cnt_video, &packet, &e);
                                        }
                                    }
                                } else if !unsupported_video_codec_logged {
                                    tracing::warn!(
                                        "[recorder] {} unsupported video codec {}; skip video packets",
                                        stream_name_cloned,
                                        codec_mime
                                    );
                                    unsupported_video_codec_logged = true;
                                }

                                if frame_written {
                                    frame_cnt_video += 1;
                                } else if attempted_video_parser
                                    && packet.header.marker
                                    && parser_error_cnt_video == 0
                                    && should_log_video_parser_counter(parser_pending_cnt_video)
                                {
                                    tracing::warn!(
                                        "[recorder] {} {} parser has not emitted a frame at marker; rtp_packets={} pending_packets={} last_ts={} payload_type={} payload_len={}",
                                        stream_name_cloned,
                                        codec_mime,
                                        rtp_cnt_video,
                                        parser_pending_cnt_video,
                                        pkt_ts,
                                        packet.header.payload_type,
                                        packet.payload.len()
                                    );
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
                            if let Some(info) = forward_clone.first_video_track_info().await {
                                codec_mime_opt = Some(info.codec_mime);
                            }
                            if let Some(codec) = codec_mime_opt.as_ref() {
                                tracing::info!(
                                    "[recorder] stream {} detected video codec {}",
                                    stream_name_cloned,
                                    codec
                                );
                                if !is_supported_video_codec(codec) {
                                    tracing::warn!(
                                        "[recorder] stream {} video codec {} is not supported by recorder",
                                        stream_name_cloned,
                                        codec
                                    );
                                }
                            }
                        }

                        if codec_mime_opt.is_some()
                            && video_rx_opt.is_none()
                            && let Some(rx) = forward_clone.subscribe_video_rtp().await
                        {
                            if let Some(info) = forward_clone.first_video_track_info().await {
                                codec_mime_opt = Some(info.codec_mime.clone());
                                segmenter.expect_video_track(
                                    info.codec_mime.clone(),
                                    info.payload_type,
                                    info.ssrc,
                                    Some(info.fmtp.as_str()),
                                );
                                segmenter.configure_video_from_track_metadata(&info.codec_mime, None, None);
                            }
                            tracing::info!(
                                "[recorder] stream {} video track became available",
                                stream_name_cloned
                            );
                            video_rx_opt = Some(rx);
                            request_keyframe(&forward_clone, &stream_name_cloned, &mut segmenter).await;
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
                return RecordingMediaOutcome::Failed;
            }

            let outcome = segmenter.media_outcome();
            match op_for_task.exists(&manifest_path).await {
                Ok(true) => outcome,
                Ok(false) => {
                    tracing::warn!(
                        "[recorder] {} no media manifest written at {}; mark recording as failed",
                        stream_name_cloned,
                        manifest_path
                    );
                    RecordingMediaOutcome::Failed
                }
                Err(e) => {
                    tracing::warn!(
                        "[recorder] {} failed to verify media manifest at {}: {}; mark recording as failed",
                        stream_name_cloned,
                        manifest_path,
                        e
                    );
                    RecordingMediaOutcome::Failed
                }
            }
        });

        let info = RecordingInfo {
            record_dir: path_prefix,
            record_id,
            start_ts_micros: Utc::now().timestamp_micros(),
        };

        Ok(Self {
            stream: stream_name,
            info,
            started_at: Instant::now(),
            base_dir_override,
            op,
            handle,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    pub async fn stop(mut self) -> RecordingStopOutcome {
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

        let status = match self.handle.await {
            Ok(RecordingMediaOutcome::Complete) => {
                let manifest_path = format!("{}/manifest.mpd", self.info.record_dir);
                match self.op.exists(&manifest_path).await {
                    Ok(true) => {
                        tracing::info!("[recorder] recording task for stream {} completed", stream);
                        RecordingStatus::Completed
                    }
                    Ok(false) => {
                        tracing::warn!(
                            "[recorder] recording task for stream {} ended without manifest {}; mark failed",
                            stream,
                            manifest_path
                        );
                        RecordingStatus::Failed
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[recorder] recording task for stream {} could not verify manifest {}: {}; mark failed",
                            stream,
                            manifest_path,
                            e
                        );
                        RecordingStatus::Failed
                    }
                }
            }
            Ok(RecordingMediaOutcome::Degraded) => {
                tracing::warn!(
                    "[recorder] recording task for stream {} completed degraded; expected video was missing from output",
                    stream
                );
                RecordingStatus::Failed
            }
            Ok(RecordingMediaOutcome::Failed) => {
                tracing::warn!(
                    "[recorder] recording task for stream {} completed without media samples",
                    stream
                );
                RecordingStatus::Failed
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
                RecordingStatus::Failed
            }
        };

        let end_ts = Utc::now().timestamp_micros();
        let duration_ms = self.started_at.elapsed().as_millis().min(i32::MAX as u128) as i32;

        RecordingStopOutcome {
            status,
            end_ts,
            duration_ms,
        }
    }
}

async fn request_keyframe(
    forward: &crate::forward::PeerForward,
    stream_name: &str,
    segmenter: &mut Segmenter,
) {
    let Some(video_track) = forward.first_video_track().await else {
        tracing::warn!(
            "[recorder] {} cannot request keyframe: video track unavailable",
            stream_name
        );
        return;
    };
    let Some(ssrc) = video_track.ssrcs().await.into_iter().next() else {
        tracing::warn!(
            "[recorder] {} cannot request keyframe: video SSRC unavailable",
            stream_name
        );
        return;
    };

    match forward
        .send_rtcp_to_publish(
            crate::forward::rtcp::RtcpMessage::PictureLossIndication,
            ssrc,
        )
        .await
    {
        Ok(()) => {
            segmenter.record_pli_request();
            tracing::debug!(
                "[recorder] {} sent codec-agnostic PLI request for source ssrc {}",
                stream_name,
                ssrc
            );
        }
        Err(e) => {
            tracing::warn!(
                "[recorder] {} failed to send PLI for source ssrc {}: {:?}",
                stream_name,
                ssrc,
                e
            );
        }
    }
}

fn should_log_video_parser_counter(count: u64) -> bool {
    count <= 3 || count.is_power_of_two() || count.is_multiple_of(500)
}

fn log_video_parser_error(
    stream_name: &str,
    codec_mime: &str,
    parser_error_count: u64,
    rtp_packet_count: u64,
    packet: &rtc::rtp::packet::Packet,
    error: &anyhow::Error,
) {
    if should_log_video_parser_counter(parser_error_count) {
        tracing::warn!(
            "[recorder] {} {} parser error {}; rtp_packets={} marker={} timestamp={} payload_type={} payload_len={} error={}",
            stream_name,
            codec_mime,
            parser_error_count,
            rtp_packet_count,
            packet.header.marker,
            packet.header.timestamp,
            packet.header.payload_type,
            packet.payload.len(),
            error
        );
    }
}

fn is_supported_video_codec(codec_mime: &str) -> bool {
    codec_mime.eq_ignore_ascii_case(MIME_TYPE_H264)
        || codec_mime.eq_ignore_ascii_case(MIME_TYPE_HEVC)
        || codec_mime.eq_ignore_ascii_case(MIME_TYPE_AV1)
        || codec_mime.eq_ignore_ascii_case(MIME_TYPE_VP9)
}

impl RecordingTask {
    pub(crate) fn has_exceeded(&self, max_duration: Duration) -> bool {
        self.started_at.elapsed() >= max_duration
    }

    pub(crate) fn next_rotation_base_dir(&self) -> Option<String> {
        self.base_dir_override
            .as_ref()
            .map(|current| Self::derive_next_base_dir(current))
    }

    fn derive_next_base_dir(current: &str) -> String {
        let trimmed = current.trim_end_matches('/');
        let next_ts = chrono::Utc::now().timestamp().to_string();
        if trimmed.is_empty() {
            return next_ts;
        }

        let mut segments: Vec<&str> = trimmed.split('/').collect();
        if let Some(last) = segments.last()
            && Self::looks_like_timestamp(last)
        {
            segments.pop();
        }

        if segments.is_empty() {
            next_ts
        } else {
            let mut new_path = segments.join("/");
            new_path.push('/');
            new_path.push_str(&next_ts);
            new_path
        }
    }

    fn looks_like_timestamp(segment: &str) -> bool {
        segment.len() >= 9 && segment.chars().all(|c| c.is_ascii_digit())
    }
}
