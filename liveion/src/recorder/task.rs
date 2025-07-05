use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use bytes::Bytes;
use chrono::{Datelike, Utc};
use tokio::task::JoinHandle;
use webrtc::api::media_engine::MIME_TYPE_H264;

use crate::recorder::rtp_h264::H264RtpParser;
use crate::recorder::segmenter::Segmenter;
use crate::recorder::STORAGE;
use crate::stream::manager::Manager;

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
            let guard = STORAGE.read().await;
            guard
                .as_ref()
                .cloned()
                .ok_or(anyhow!("storage operator not initialized"))?
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

        // Initialize Segmenter
        let mut segmenter = Segmenter::new(op, stream_name.clone(), path_prefix).await?;

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

        tracing::info!("[recorder] stream {} use codec {}", stream_name, codec_mime);

        tracing::info!("[recorder] subscribed RTP for stream {}", stream_name);

        let stream_name_cloned = stream_name.clone();
        let handle = tokio::spawn(async move {
            let is_h264 = codec_mime.eq_ignore_ascii_case(MIME_TYPE_H264);
            let mut parser = H264RtpParser::new();
            let mut prev_ts: Option<u32> = None;
            let mut frame_cnt: u64 = 0;
            let mut last_log = Instant::now();

            while let Ok(packet) = rtp_receiver.recv().await {
                if !is_h264 {
                    continue; // Currently only handle H.264
                }

                let pkt_ts = packet.header.timestamp;

                if let Ok(Some((frame, is_idr))) = parser.push_packet((*packet).clone()) {
                    // Calculate frame duration based on RTP timestamp delta
                    let duration_ticks: u32 = if let Some(prev) = prev_ts {
                        pkt_ts.wrapping_sub(prev)
                    } else {
                        // Fallback to 30fps => 3000 ticks @ 90kHz
                        3_000
                    };
                    prev_ts = Some(pkt_ts);

                    let _ = segmenter
                        .push_h264(Bytes::from(frame.clone()), is_idr, duration_ticks)
                        .await;

                    frame_cnt += 1;

                    if last_log.elapsed() >= Duration::from_secs(5) {
                        tracing::info!(
                            "[recorder] stream {} received {} frames in last 5s",
                            stream_name_cloned,
                            frame_cnt
                        );
                        frame_cnt = 0;
                        last_log = Instant::now();
                    }
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
