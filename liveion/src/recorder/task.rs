use std::sync::Arc;

use anyhow::{anyhow, Result};
use bytes::Bytes;
use chrono::{Datelike, Utc};
use tokio::task::JoinHandle;
use webrtc::rtp::packet::Packet;

use crate::stream::manager::Manager;
use crate::recorder::segmenter::Segmenter;
use crate::recorder::STORAGE;
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

        // Subscribe to video RTP. If the video track hasn't appeared yet, loop every 500 ms until it does.
        let mut rtp_receiver = loop {
            if let Some(rx) = forward.subscribe_video_rtp().await {
                break rx;
            }
            tracing::debug!("[recorder] waiting for video track of stream {}", stream_name);
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        };

        let handle = tokio::spawn(async move {
            let mut depay = RePayloadCodec::new("video/h264".to_string());
            while let Ok(packet) = rtp_receiver.recv().await {
                // Depacketize RTP into complete frames
                for p in depay.payload((*packet).clone()) {
                    let payload = p.payload.clone();
                    if payload.is_empty() {
                        continue;
                    }
                    let nal_unit_type = payload[0] & 0x1F;
                    let is_idr = nal_unit_type == 5;
                    let mut buf = vec![0, 0, 0, 1];
                    buf.extend_from_slice(&payload);
                    let _ = segmenter
                        .push_h264(Bytes::from(buf), is_idr)
                        .await;
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