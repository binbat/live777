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

/// Outcome of a manual bitrate request (admin API).
#[cfg(feature = "source")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetBitrateOutcome {
    /// The encoder accepted the new bitrate.  `adaptive_suspended` is true
    /// when the stream's adaptive controller went into manual mode.
    Applied { adaptive_suspended: bool },
    /// No source is registered for the stream.
    SourceNotFound,
    /// The source's encoder backend cannot retune at runtime (or the
    /// pipeline is not running).
    Unsupported,
}

/// Outcome of resolving a configured tier name (admin API).
#[cfg(feature = "source")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierResolution {
    Resolved(u32),
    SourceNotFound,
    TierNotFound,
}

/// How the stream source's encoder bitrate is currently driven.
#[cfg(feature = "source")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitrateMode {
    /// The AIMD controller drives the encoder from subscriber feedback.
    Adaptive,
    /// A manual override (admin API) holds the encoder at a fixed value;
    /// the adaptive controller, when present, is suspended.
    Manual,
    /// No adaptive controller and no manual override — the encoder runs at
    /// its configured bitrate.
    Fixed,
}

impl BitrateMode {
    #[cfg(feature = "source")]
    pub fn as_str(&self) -> &'static str {
        match self {
            BitrateMode::Adaptive => "adaptive",
            BitrateMode::Manual => "manual",
            BitrateMode::Fixed => "fixed",
        }
    }
}

/// Snapshot of a stream source's bitrate state (admin API).
#[cfg(feature = "source")]
#[derive(Debug, Clone)]
pub struct SourceBitrateInfo {
    pub mode: BitrateMode,
    /// Current encoder bitrate (configured value until something retunes
    /// it).  `None` when the stream has no bitrate state.
    pub current: Option<u32>,
    /// Active manual override bitrate.
    pub manual: Option<u32>,
    /// The tier whose bitrate equals the manual override, if any.
    pub active_tier: Option<String>,
    /// Whether the source opted into adaptive bitrate.
    pub adaptive: bool,
    /// Configured quality tiers (empty when the source defines none).
    pub tiers: Vec<super::adaptive_bitrate::BitrateTier>,
}

#[derive(Clone)]
pub struct SourceManager {
    pub(crate) sources: SourceMap,

    #[cfg(feature = "source")]
    bridges: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<SourceBridge>>>>>,

    /// Per-stream manual/auto bitrate coordination, one entry per adaptive
    /// controller (created in `create_bridge`, dropped with the bridge).
    #[cfg(feature = "source")]
    bitrate_controls: Arc<RwLock<HashMap<String, Arc<super::adaptive_bitrate::BitrateControl>>>>,
}

impl SourceManager {
    pub fn new() -> Self {
        Self {
            sources: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "source")]
            bridges: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "source")]
            bitrate_controls: Arc::new(RwLock::new(HashMap::new())),
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
            self.bitrate_controls.write().await.remove(stream_id);
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
            feature = "source-whep"
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
            forward.clone(),
            has_video,
            has_audio,
            #[cfg(any(
                feature = "source-rtsp",
                feature = "source-sdp",
                feature = "source-whep"
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

        let bridge_arc = Arc::new(tokio::sync::Mutex::new(bridge));
        bridges.insert(stream_id.to_string(), bridge_arc.clone());
        drop(bridges);

        // Per-stream bitrate state (issue #409): created for every source
        // with a known encoder bitrate so the admin API can report and
        // override it.  The adaptive controller additionally spawns when
        // the source opted in via `encoder.adaptive_bitrate` and its
        // backend supports runtime retuning; it exits when the bridge is
        // dropped.
        {
            let source_guard = source.lock().await;
            let adaptive_cfg = source_guard.adaptive_bitrate_config();
            let configured = source_guard.configured_bitrate();
            drop(source_guard);

            let control = if let Some(target) = configured {
                let control = Arc::new(super::adaptive_bitrate::BitrateControl::new(target));
                self.bitrate_controls
                    .write()
                    .await
                    .insert(stream_id.to_string(), control.clone());
                Some(control)
            } else {
                None
            };

            if let (Some(cfg), Some(control)) = (adaptive_cfg, control) {
                super::adaptive_bitrate::spawn(
                    stream_id.to_string(),
                    forward,
                    source.clone(),
                    Arc::downgrade(&bridge_arc),
                    cfg,
                    control,
                );
            }
        }

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

    /// Apply a manual bitrate override to a stream's source encoder (admin
    /// API, issue #409).  A stream with an adaptive controller goes into
    /// manual mode — the controller suspends until
    /// [`SourceManager::resume_adaptive_bitrate`].
    #[cfg(feature = "source")]
    pub async fn set_source_bitrate(&self, stream_id: &str, bps: u32) -> SetBitrateOutcome {
        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        };
        let Some(source) = source else {
            return SetBitrateOutcome::SourceNotFound;
        };

        if !source.lock().await.set_bitrate(bps).await {
            return SetBitrateOutcome::Unsupported;
        }

        let adaptive_suspended =
            if let Some(control) = self.bitrate_controls.read().await.get(stream_id) {
                control.set_manual(bps);
                true
            } else {
                false
            };
        SetBitrateOutcome::Applied { adaptive_suspended }
    }

    /// Clear a manual bitrate override, returning the bitrate afterwards
    /// and whether the source has an adaptive controller (which then
    /// resumes from that value).  `None` when the stream has no bitrate
    /// state at all (no source, or a non-native source).
    #[cfg(feature = "source")]
    pub async fn clear_manual_bitrate(&self, stream_id: &str) -> Option<(u32, bool)> {
        let control = self.bitrate_controls.read().await.get(stream_id).cloned();
        let control = control?;
        control.clear_manual();

        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        };
        let adaptive = match source {
            Some(source) => source.lock().await.adaptive_bitrate_config().is_some(),
            None => false,
        };
        Some((control.current(), adaptive))
    }

    /// Resolve a configured tier name to its bitrate for a stream's source.
    #[cfg(feature = "source")]
    pub async fn resolve_bitrate_tier(&self, stream_id: &str, tier: &str) -> TierResolution {
        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        };
        let Some(source) = source else {
            return TierResolution::SourceNotFound;
        };
        let tiers = source.lock().await.bitrate_tiers();
        match tiers.iter().find(|t| t.name == tier) {
            Some(t) => TierResolution::Resolved(t.bitrate),
            None => TierResolution::TierNotFound,
        }
    }

    /// Snapshot of a stream source's bitrate state for the admin API.
    #[cfg(feature = "source")]
    pub async fn source_bitrate_info(&self, stream_id: &str) -> Option<SourceBitrateInfo> {
        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        }?;
        let (tiers, adaptive) = {
            let source_guard = source.lock().await;
            (
                source_guard.bitrate_tiers(),
                source_guard.adaptive_bitrate_config().is_some(),
            )
        };

        let control = self.bitrate_controls.read().await.get(stream_id).cloned();
        let manual = control.as_ref().and_then(|c| c.manual());
        let current = control.as_ref().map(|c| c.current());
        let active_tier = manual.and_then(|m| {
            tiers
                .iter()
                .find(|t| t.bitrate == m)
                .map(|t| t.name.clone())
        });
        let mode = match (adaptive, manual) {
            (true, None) => BitrateMode::Adaptive,
            (_, Some(_)) => BitrateMode::Manual,
            _ => BitrateMode::Fixed,
        };

        Some(SourceBitrateInfo {
            mode,
            current,
            manual,
            active_tier,
            adaptive,
            tiers,
        })
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
            self.bitrate_controls.write().await.clear();
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
        #[cfg(feature = "source")]
        tiers: Vec<super::super::adaptive_bitrate::BitrateTier>,
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
                #[cfg(feature = "source")]
                tiers: Vec::new(),
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

        #[cfg(feature = "source")]
        fn bitrate_tiers(&self) -> Vec<super::super::adaptive_bitrate::BitrateTier> {
            self.tiers.clone()
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

    #[cfg(feature = "source")]
    fn tiered_mock(id: &str) -> MockSource {
        let mut source = MockSource::new(id);
        source.tiers = vec![
            super::super::adaptive_bitrate::BitrateTier {
                name: "low".into(),
                bitrate: 600_000,
            },
            super::super::adaptive_bitrate::BitrateTier {
                name: "mid".into(),
                bitrate: 2_000_000,
            },
        ];
        source
    }

    #[cfg(feature = "source")]
    #[tokio::test]
    async fn resolve_bitrate_tier_matches_configured_names() {
        let manager = SourceManager::new();
        manager
            .add_source(Box::new(tiered_mock("cam")))
            .await
            .unwrap();

        assert_eq!(
            manager.resolve_bitrate_tier("cam", "mid").await,
            super::TierResolution::Resolved(2_000_000)
        );
        assert_eq!(
            manager.resolve_bitrate_tier("cam", "high").await,
            super::TierResolution::TierNotFound
        );
        assert_eq!(
            manager.resolve_bitrate_tier("other", "mid").await,
            super::TierResolution::SourceNotFound
        );
    }

    #[cfg(feature = "source")]
    #[tokio::test]
    async fn source_bitrate_info_reports_fixed_mode_without_control() {
        let manager = SourceManager::new();
        manager
            .add_source(Box::new(tiered_mock("cam")))
            .await
            .unwrap();

        let info = manager.source_bitrate_info("cam").await.unwrap();
        assert_eq!(info.mode, super::BitrateMode::Fixed);
        assert_eq!(info.tiers.len(), 2);
        assert!(info.manual.is_none());
        assert!(info.active_tier.is_none());
        // MockSource has no configured bitrate: no control, no current.
        assert!(info.current.is_none());
        assert!(manager.source_bitrate_info("other").await.is_none());
    }
}
