use std::sync::Arc;

use anyhow::{anyhow, Result};
use bytes::Bytes;
use chrono::{Datelike, Utc};
use tokio::task::JoinHandle;
use webrtc::api::media_engine::MIME_TYPE_H264;

use crate::recorder::segmenter::Segmenter;
use crate::recorder::STORAGE;
use crate::stream::manager::Manager;
use livetwo::payload::{RePayload, RePayloadCodec};

#[derive(Debug)]
pub struct RecordingTask {
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

        let handle = tokio::spawn(async move {
            let mut depay = RePayloadCodec::new(codec_mime.clone());
            let is_h264 = codec_mime == MIME_TYPE_H264;
            while let Ok(packet) = rtp_receiver.recv().await {
                // Depacketize RTP into complete frames
                for p in depay.payload((*packet).clone()) {
                    let payload = p.payload.clone();
                    if payload.is_empty() {
                        continue;
                    }
                    if is_h264 {
                        // For H.264, wrap with Annex-B start code and push to segmenter
                        let nal_unit_type = payload[0] & 0x1F;
                        let is_idr = nal_unit_type == 5;
                        let mut buf = vec![0, 0, 0, 1];
                        buf.extend_from_slice(&payload);
                        let _ = segmenter.push_h264(Bytes::from(buf), is_idr).await;
                    } else {
                        // TODO: 支持 VP8/VP9 等编码封装到 fMP4
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
