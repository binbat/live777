use super::{StreamSource, StreamSourceState};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

#[cfg(feature = "source")]
use crate::forward::{PeerForward, SourceBridge};

type SourceMap = Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<Box<dyn StreamSource>>>>>>;

#[derive(Clone)]
pub struct SourceManager {
    pub(crate) sources: SourceMap,

    #[cfg(feature = "source")]
    bridges: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<SourceBridge>>>>>,
}

impl SourceManager {
    pub fn new() -> Self {
        Self {
            sources: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "source")]
            bridges: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_source(&self, mut source: Box<dyn StreamSource>) -> Result<String> {
        let stream_id = source.stream_id().to_string();

        source.start().await?;

        let mut sources = self.sources.write().await;
        sources.insert(stream_id.clone(), Arc::new(tokio::sync::Mutex::new(source)));

        info!("Added source: {}", stream_id);

        Ok(stream_id)
    }

    pub async fn remove_source(&self, stream_id: &str) -> Result<()> {
        #[cfg(feature = "source")]
        {
            let mut bridges = self.bridges.write().await;
            if let Some(bridge) = bridges.remove(stream_id) {
                let mut bridge = bridge.lock().await;
                if let Err(e) = bridge.stop().await {
                    warn!("Failed to stop bridge for {}: {}", stream_id, e);
                }
            }
        }

        let mut sources = self.sources.write().await;
        if let Some(source) = sources.remove(stream_id) {
            let mut source = source.lock().await;
            source.stop().await?;
            info!("Removed source: {}", stream_id);
            Ok(())
        } else {
            anyhow::bail!("Source not found: {}", stream_id)
        }
    }

    pub async fn list_sources(&self) -> Vec<(String, String, StreamSourceState)> {
        let sources = self.sources.read().await;
        let mut result = Vec::new();

        for (id, source) in sources.iter() {
            let source = source.lock().await;
            result.push((id.clone(), source.stream_id().to_string(), source.state()));
        }

        result
    }

    #[cfg(feature = "source")]
    pub async fn create_bridge(&self, stream_id: &str, forward: Arc<PeerForward>) -> Result<()> {
        info!("Creating bridge for {}", stream_id);

        let sources = self.sources.read().await;
        let source = sources
            .get(stream_id)
            .ok_or_else(|| anyhow::anyhow!("Source not found: {}", stream_id))?
            .clone();
        drop(sources);

        let max_retries = 30;
        let mut video_codec = None;
        let mut audio_codec = None;

        for attempt in 1..=max_retries {
            let source_guard = source.lock().await;

            video_codec = source_guard.get_video_codec().await;
            audio_codec = source_guard.get_audio_codec().await;

            if video_codec.is_some() || audio_codec.is_some() {
                info!(
                    "Codec ready for {} (attempt {}/{}): video={}, audio={}",
                    stream_id,
                    attempt,
                    max_retries,
                    video_codec.is_some(),
                    audio_codec.is_some()
                );
                drop(source_guard);
                break;
            }

            drop(source_guard);

            if attempt == max_retries {
                anyhow::bail!(
                    "Codec not ready for {} after {} attempts",
                    stream_id,
                    max_retries
                );
            }

            warn!(
                "Codec not ready for {} (attempt {}/{}), retrying...",
                stream_id, attempt, max_retries
            );

            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        if let Some(codec) = video_codec {
            info!(
                "Adding video track for {}: {}",
                stream_id, codec.capability.mime_type
            );

            if let Err(e) = forward
                .add_virtual_track(
                    webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Video,
                    codec,
                )
                .await
            {
                warn!("Failed to add video track: {:?}", e);
            }
        }

        if let Some(codec) = audio_codec {
            info!(
                "Adding audio track for {}: {}",
                stream_id, codec.capability.mime_type
            );

            if let Err(e) = forward
                .add_virtual_track(
                    webrtc::rtp_transceiver::rtp_codec::RTPCodecType::Audio,
                    codec,
                )
                .await
            {
                warn!("Failed to add audio track: {:?}", e);
            }
        }

        let source_guard = source.lock().await;
        let rtp_rx = source_guard.subscribe_rtp();
        let state_rx = source_guard.subscribe_state();

        info!("Waiting for RTCP sender for {}", stream_id);

        let mut rtcp_sender = None;
        for attempt in 1..=20 {
            rtcp_sender = source_guard.get_rtcp_sender().await;

            if rtcp_sender.is_some() {
                info!("RTCP sender ready for {} (attempt {})", stream_id, attempt);
                break;
            }

            if attempt == 20 {
                warn!(
                    "RTCP sender not ready for {} after 20 attempts, continuing without it",
                    stream_id
                );
                break;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        drop(source_guard);

        let mut bridge = SourceBridge::new(stream_id.to_string(), forward);

        if let Some(rtcp_tx) = rtcp_sender {
            bridge.set_rtcp_sender(rtcp_tx);
            info!("RTCP sender connected for {}", stream_id);
        } else {
            warn!(
                "No RTCP sender for {}, keyframe requests will not work",
                stream_id
            );
        }

        bridge.start_bridging(rtp_rx, state_rx).await?;

        let mut bridges = self.bridges.write().await;
        bridges.insert(
            stream_id.to_string(),
            Arc::new(tokio::sync::Mutex::new(bridge)),
        );

        info!("Bridge created for {}", stream_id);

        Ok(())
    }

    #[cfg(feature = "source")]
    pub async fn is_codec_ready(&self, stream_id: &str) -> bool {
        let sources = self.sources.read().await;
        if let Some(source_mutex) = sources.get(stream_id) {
            let source = source_mutex.lock().await;
            return source.get_video_codec().await.is_some()
                || source.get_audio_codec().await.is_some();
        }

        false
    }

    pub async fn stop_all(&self) -> Result<()> {
        info!("Stopping all sources");

        #[cfg(feature = "source")]
        {
            let mut bridges = self.bridges.write().await;
            for (stream_id, bridge) in bridges.drain() {
                let mut bridge = bridge.lock().await;
                if let Err(e) = bridge.stop().await {
                    error!("Failed to stop bridge {}: {}", stream_id, e);
                }
            }
        }

        let mut sources = self.sources.write().await;
        for (stream_id, source) in sources.drain() {
            let mut source = source.lock().await;
            if let Err(e) = source.stop().await {
                error!("Failed to stop source {}: {}", stream_id, e);
            }
        }

        info!("All sources stopped");
        Ok(())
    }
}

impl Default for SourceManager {
    fn default() -> Self {
        Self::new()
    }
}
