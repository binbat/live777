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

/// Default per-attempt codec re-wait inside bridge creation, for callers
/// without their own wait budget (startup auto-start, source API).
#[cfg(feature = "source")]
pub const DEFAULT_BRIDGE_CODEC_WAIT: Duration = Duration::from_secs(6);

/// Default RTCP-sender wait inside bridge creation (non-fatal when it
/// elapses: keyframe requests just won't work).
#[cfg(feature = "source")]
pub const DEFAULT_BRIDGE_RTCP_WAIT: Duration = Duration::from_secs(2);

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
        if sources.contains_key(&stream_id) {
            source.stop().await?;
            anyhow::bail!("Source already exists: {}", stream_id);
        }
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

    pub async fn has_source(&self, stream_id: &str) -> bool {
        self.sources.read().await.contains_key(stream_id)
    }

    /// Whether a media bridge (virtual tracks) was installed for the stream.
    /// A source can exist without a bridge when bridge creation failed.
    #[cfg(feature = "source")]
    pub async fn has_bridge(&self, stream_id: &str) -> bool {
        self.bridges.read().await.contains_key(stream_id)
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

    /// Install the media bridge (virtual tracks) for a source.
    ///
    /// `codec_wait` bounds how long to poll for the source's codec to become
    /// known before giving up; `rtcp_wait` bounds the (non-fatal) wait for
    /// the source's RTCP sender. Both are caller-controlled so a subscriber
    /// blocked in on-demand source startup is not held longer than its own
    /// start budget, while startup paths can afford a longer grace period.
    #[cfg(feature = "source")]
    pub async fn create_bridge(
        &self,
        stream_id: &str,
        forward: PeerForward,
        codec_wait: Duration,
        rtcp_wait: Duration,
    ) -> Result<()> {
        info!("Creating bridge for {}", stream_id);

        {
            let bridges = self.bridges.read().await;
            if bridges.contains_key(stream_id) {
                anyhow::bail!("Bridge already exists for source: {}", stream_id);
            }
        }

        let sources = self.sources.read().await;
        let source = sources
            .get(stream_id)
            .ok_or_else(|| anyhow::anyhow!("Source not found: {}", stream_id))?
            .clone();
        drop(sources);

        let codec_deadline = std::time::Instant::now() + codec_wait;
        let (video_codec, audio_codec) = loop {
            let source_guard = source.lock().await;

            let video_codec = source_guard.get_video_codec().await;
            let audio_codec = source_guard.get_audio_codec().await;

            if video_codec.is_some() || audio_codec.is_some() {
                info!(
                    "Codec ready for {}: video={}, audio={}",
                    stream_id,
                    video_codec.is_some(),
                    audio_codec.is_some()
                );
                drop(source_guard);
                break (video_codec, audio_codec);
            }

            drop(source_guard);

            let now = std::time::Instant::now();
            if now >= codec_deadline {
                anyhow::bail!("Codec not ready for {} within {:?}", stream_id, codec_wait);
            }

            warn!("Codec not ready for {}, retrying...", stream_id);

            tokio::time::sleep(
                Duration::from_millis(200).min(codec_deadline.saturating_duration_since(now)),
            )
            .await;
        };

        let has_video = video_codec.is_some();
        let has_audio = audio_codec.is_some();
        #[cfg(any(
            feature = "source-rtsp",
            feature = "source-sdp",
            feature = "source-whep",
            feature = "native-source"
        ))]
        let video_codec_name = video_codec.as_ref().and_then(|c| {
            c.rtp_codec
                .mime_type
                .split('/')
                .nth(1)
                .map(|s| s.to_string())
        });

        if let Some(codec) = video_codec {
            info!(
                "Adding video track for {}: {}",
                stream_id, codec.rtp_codec.mime_type
            );

            if let Err(e) = forward
                .add_virtual_track(rtc::rtp_transceiver::rtp_sender::RtpCodecKind::Video, codec)
                .await
            {
                anyhow::bail!("Failed to add video track for {}: {:?}", stream_id, e);
            }
        }

        if let Some(codec) = audio_codec {
            info!(
                "Adding audio track for {}: {}",
                stream_id, codec.rtp_codec.mime_type
            );

            if let Err(e) = forward
                .add_virtual_track(rtc::rtp_transceiver::rtp_sender::RtpCodecKind::Audio, codec)
                .await
            {
                anyhow::bail!("Failed to add audio track for {}: {:?}", stream_id, e);
            }
        }

        let source_guard = source.lock().await;
        let rtp_rx = source_guard.subscribe_rtp();
        let state_rx = source_guard.subscribe_state();

        info!("Waiting for RTCP sender for {}", stream_id);

        let rtcp_deadline = std::time::Instant::now() + rtcp_wait;
        let rtcp_sender = loop {
            let rtcp_sender = source_guard.get_rtcp_sender().await;

            if rtcp_sender.is_some() {
                info!("RTCP sender ready for {}", stream_id);
                break rtcp_sender;
            }

            let now = std::time::Instant::now();
            if now >= rtcp_deadline {
                warn!(
                    "RTCP sender not ready for {} within {:?}, continuing without it",
                    stream_id, rtcp_wait
                );
                break None;
            }

            tokio::time::sleep(
                Duration::from_millis(100).min(rtcp_deadline.saturating_duration_since(now)),
            )
            .await;
        };

        drop(source_guard);

        let mut bridge = SourceBridge::new(
            stream_id.to_string(),
            forward,
            has_video,
            has_audio,
            #[cfg(any(
                feature = "source-rtsp",
                feature = "source-sdp",
                feature = "source-whep",
                feature = "native-source"
            ))]
            video_codec_name,
        );

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

        // Re-check under write lock: another concurrent create_bridge may have
        // inserted a bridge while we were doing async setup above.
        if bridges.contains_key(stream_id) {
            drop(bridges);
            if let Err(e) = bridge.stop().await {
                warn!("Failed to stop duplicate bridge for {}: {}", stream_id, e);
            }
            anyhow::bail!("Bridge already exists for source: {}", stream_id);
        }

        let sources = self.sources.read().await;
        if !sources.contains_key(stream_id) {
            drop(sources);
            drop(bridges);
            if let Err(e) = bridge.stop().await {
                warn!("Failed to stop orphan bridge for {}: {}", stream_id, e);
            }
            anyhow::bail!("Source was removed while creating bridge: {}", stream_id);
        }
        drop(sources);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::source::{MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
    use anyhow::Result;
    use async_trait::async_trait;
    use tokio::sync::broadcast;

    struct MockSource {
        id: String,
        state: StreamSourceState,
        rtp_tx: broadcast::Sender<MediaPacket>,
        state_tx: broadcast::Sender<StateChangeEvent>,
        started: bool,
    }

    impl MockSource {
        fn new(id: &str) -> Self {
            let (rtp_tx, _) = broadcast::channel(16);
            let (state_tx, _) = broadcast::channel(16);
            Self {
                id: id.to_string(),
                state: StreamSourceState::Disconnected,
                rtp_tx,
                state_tx,
                started: false,
            }
        }
    }

    #[async_trait]
    impl StreamSource for MockSource {
        fn stream_id(&self) -> &str {
            &self.id
        }

        fn state(&self) -> StreamSourceState {
            self.state
        }

        async fn start(&mut self) -> Result<()> {
            self.started = true;
            self.state = StreamSourceState::Connected;
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            self.started = false;
            self.state = StreamSourceState::Disconnected;
            Ok(())
        }

        fn subscribe_rtp(&self) -> broadcast::Receiver<MediaPacket> {
            self.rtp_tx.subscribe()
        }

        fn subscribe_state(&self) -> broadcast::Receiver<StateChangeEvent> {
            self.state_tx.subscribe()
        }
    }

    #[tokio::test]
    async fn add_source_rejects_duplicate_stream_id() {
        let manager = SourceManager::new();
        let source1 = Box::new(MockSource::new("test"));
        let source2 = Box::new(MockSource::new("test"));

        manager.add_source(source1).await.unwrap();
        let err = manager.add_source(source2).await.unwrap_err();
        assert!(err.to_string().contains("Source already exists"));

        let sources = manager.list_sources().await;
        assert_eq!(sources.len(), 1);
    }

    #[tokio::test]
    async fn stop_all_stops_all_sources() {
        let manager = SourceManager::new();
        manager
            .add_source(Box::new(MockSource::new("a")))
            .await
            .unwrap();
        manager
            .add_source(Box::new(MockSource::new("b")))
            .await
            .unwrap();

        manager.stop_all().await.unwrap();

        let sources = manager.list_sources().await;
        assert!(sources.is_empty());
    }
}
