use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use bytes::Bytes;
use chrono::{Datelike, Utc};
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
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

        // Wait for video track and obtain codec mime type
        let codec_mime = loop {
            if let Some(c) = forward.first_video_codec().await {
                break c;
            }
            tracing::debug!(
                "[recorder] waiting for video codec of stream {}",
                stream_name
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
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
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        };

        tracing::info!("[recorder] stream {} use codec {}", stream_name, codec_mime);

        tracing::info!("[recorder] subscribed RTP for stream {}", stream_name);

        let stream_name_cloned = stream_name.clone();
        let handle = tokio::spawn(async move {
            // Open raw .h264 stream file for debugging
            let dump_path = format!("{}.h264", stream_name_cloned);
            let mut dump_file = match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&dump_path)
                .await
            {
                Ok(f) => Some(f),
                Err(e) => {
                    tracing::warn!("[recorder] open dump file failed: {}", e);
                    None
                }
            };
            let is_h264 = codec_mime.eq_ignore_ascii_case(MIME_TYPE_H264);
            let mut parser = H264RtpParser::new();
            let mut frame_cnt: u64 = 0;
            let mut last_log = Instant::now();

            while let Ok(packet) = rtp_receiver.recv().await {
                if !is_h264 {
                    continue; // Currently only handle H.264
                }

                if let Ok(Some((frame, is_idr))) = parser.push_packet((*packet).clone()) {
                    let _ = segmenter
                        .push_h264(Bytes::from(frame.clone()), is_idr)
                        .await;

                    if let Some(f) = dump_file.as_mut() {
                        let _ = f.write_all(&frame).await;
                    }

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
