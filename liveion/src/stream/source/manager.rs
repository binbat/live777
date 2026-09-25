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
    /// The requested bitrate exceeds the ceiling: the configured
    /// `encoder.bitrate` (carried here), or `i32::MAX` when the source has
    /// no configured bitrate — the encoder control channel is a signed
    /// 32-bit value, so larger requests cannot be represented.
    AboveCeiling(u32),
    /// The source's encoder backend cannot retune at runtime (or the
    /// pipeline is not running).
    Unsupported,
}

/// Outcome of clearing a manual bitrate override (admin API).
#[cfg(feature = "source")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearBitrateOutcome {
    /// The override was cleared.  `bitrate` is what the encoder runs at
    /// now: the held value an adaptive controller resumes from, or the
    /// restored configured bitrate of a fixed source.  `adaptive` tells
    /// whether an adaptive controller exists for the stream.
    Cleared { bitrate: u32, adaptive: bool },
    /// A fixed source's configured bitrate could not be restored (encoder
    /// rejected the retune).  The manual override is left in effect so the
    /// reported state keeps matching the encoder.
    RestoreFailed,
}

/// Outcome of resolving a configured tier name (admin API).
#[cfg(feature = "source")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TierResolution {
    Resolved(super::adaptive_bitrate::QualityTier),
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
    pub tiers: Vec<super::adaptive_bitrate::QualityTier>,
}

#[derive(Clone)]
pub struct SourceManager {
    pub(crate) sources: SourceMap,

    #[cfg(feature = "source")]
    bridges: Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<SourceBridge>>>>>,

    /// Per-stream manual/auto bitrate coordination: one entry per source
    /// bridge with a known encoder bitrate (created in `create_bridge`,
    /// removed in `remove_source`/`stop_all`).  Present for fixed-bitrate
    /// sources too, not only adaptive ones.
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
    /// [`SourceManager::clear_manual_bitrate`].
    #[cfg(feature = "source")]
    pub async fn set_source_bitrate(&self, stream_id: &str, bps: u32) -> SetBitrateOutcome {
        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        };
        let Some(source) = source else {
            return SetBitrateOutcome::SourceNotFound;
        };

        // Hold the source lock across the encoder retune and the manual-flag
        // update: the adaptive controller applies its decisions under the
        // same lock and re-checks the flag there, so a manual set can
        // neither interleave with nor be overwritten by an in-flight AIMD
        // decision.
        let source_guard = source.lock().await;

        // Config tiers are validated against the same ceiling; raw manual
        // values must be too.  Without a configured bitrate the encoder
        // control channel (a signed 32-bit value) is the only bound.
        let ceiling = source_guard
            .configured_bitrate()
            .unwrap_or(i32::MAX as u32)
            .min(i32::MAX as u32);
        if bps > ceiling {
            return SetBitrateOutcome::AboveCeiling(ceiling);
        }

        if !source_guard.set_bitrate(bps).await {
            return SetBitrateOutcome::Unsupported;
        }
        let adaptive = source_guard.adaptive_bitrate_config().is_some();

        // A control exists for every native source, adaptive or not —
        // `adaptive_suspended` is only true when a controller is actually
        // running for this stream.
        let adaptive_suspended =
            if let Some(control) = self.bitrate_controls.read().await.get(stream_id) {
                control.set_manual(bps);
                adaptive
            } else {
                false
            };
        drop(source_guard);
        SetBitrateOutcome::Applied { adaptive_suspended }
    }

    /// Clear a manual bitrate override.  An adaptive controller resumes
    /// from the held value; a fixed source is retuned back to its
    /// configured bitrate, so clearing restores configured behavior rather
    /// than leaving the manual value running.  `None` when the stream has
    /// no bitrate state at all (no source, or a non-native source).
    #[cfg(feature = "source")]
    pub async fn clear_manual_bitrate(&self, stream_id: &str) -> Option<ClearBitrateOutcome> {
        let control = self.bitrate_controls.read().await.get(stream_id).cloned();
        let control = control?;

        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        };
        let (adaptive, configured) = match &source {
            Some(source) => {
                let guard = source.lock().await;
                (
                    guard.adaptive_bitrate_config().is_some(),
                    guard.configured_bitrate(),
                )
            }
            None => (false, None),
        };

        if !adaptive
            && let (Some(manual), Some(source), Some(configured)) =
                (control.manual(), &source, configured)
            && manual != configured
        {
            if !source.lock().await.set_bitrate(configured).await {
                // Keep the override: the encoder is still at the
                // manual value, and the reported state must match.
                return Some(ClearBitrateOutcome::RestoreFailed);
            }
            control.note_applied(configured);
        }

        control.clear_manual();
        Some(ClearBitrateOutcome::Cleared {
            bitrate: control.current(),
            adaptive,
        })
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
        let tiers = source.lock().await.quality_tiers();
        match tiers.into_iter().find(|t| t.name == tier) {
            Some(t) => TierResolution::Resolved(t),
            None => TierResolution::TierNotFound,
        }
    }

    /// Apply a resolution/framerate tier (admin API): rebuild the stream
    /// source's capture+encoder pipeline with the tier's geometry and
    /// bitrate.  Subscribers stay attached; they observe a short frame
    /// gap.  Like [`SourceManager::set_source_bitrate`], an adaptive
    /// stream goes into manual mode at the tier's bitrate.
    #[cfg(feature = "source")]
    pub async fn apply_source_tier(
        &self,
        stream_id: &str,
        tier: &super::adaptive_bitrate::QualityTier,
    ) -> SetBitrateOutcome {
        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        };
        let Some(source) = source else {
            return SetBitrateOutcome::SourceNotFound;
        };

        // The rebuild swaps the whole pipeline; the adaptive controller
        // never touches the encoder mid-rebuild because it only applies
        // decisions under this same source lock.
        let mut source_guard = source.lock().await;
        if !source_guard.apply_quality_tier(tier).await {
            return SetBitrateOutcome::Unsupported;
        }
        let adaptive = source_guard.adaptive_bitrate_config().is_some();

        let adaptive_suspended =
            if let Some(control) = self.bitrate_controls.read().await.get(stream_id) {
                control.set_manual(tier.bitrate);
                adaptive
            } else {
                false
            };
        drop(source_guard);
        SetBitrateOutcome::Applied { adaptive_suspended }
    }

    /// Snapshot of a stream source's bitrate state for the admin API.
    #[cfg(feature = "source")]
    pub async fn source_bitrate_info(&self, stream_id: &str) -> Option<SourceBitrateInfo> {
        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        }?;
        let (tiers, adaptive, configured) = {
            let source_guard = source.lock().await;
            (
                source_guard.quality_tiers(),
                source_guard.adaptive_bitrate_config().is_some(),
                source_guard.configured_bitrate(),
            )
        };

        let control = self.bitrate_controls.read().await.get(stream_id).cloned();
        let manual = control.as_ref().and_then(|c| c.manual());
        // No control yet (source in standby, no bridge): report the
        // configured bitrate, which is where the encoder will start.
        let current = control.as_ref().map(|c| c.current()).or(configured);
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
        tiers: Vec<super::super::adaptive_bitrate::QualityTier>,
        #[cfg(feature = "source")]
        adaptive: bool,
        #[cfg(feature = "source")]
        configured: Option<u32>,
        #[cfg(feature = "source")]
        retune_ok: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
                #[cfg(feature = "source")]
                adaptive: false,
                #[cfg(feature = "source")]
                configured: None,
                #[cfg(feature = "source")]
                retune_ok: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
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
        fn quality_tiers(&self) -> Vec<super::super::adaptive_bitrate::QualityTier> {
            self.tiers.clone()
        }

        #[cfg(feature = "source")]
        fn adaptive_bitrate_config(
            &self,
        ) -> Option<super::super::adaptive_bitrate::AdaptiveBitrateConfig> {
            self.adaptive
                .then_some(super::super::adaptive_bitrate::AdaptiveBitrateConfig {
                    target: 4_000_000,
                    min: 300_000,
                })
        }

        #[cfg(feature = "source")]
        async fn set_bitrate(&self, _bps: u32) -> bool {
            self.retune_ok.load(std::sync::atomic::Ordering::Relaxed)
        }

        #[cfg(feature = "source")]
        fn configured_bitrate(&self) -> Option<u32> {
            self.configured
        }

        #[cfg(feature = "source")]
        async fn apply_quality_tier(
            &mut self,
            tier: &super::super::adaptive_bitrate::QualityTier,
        ) -> bool {
            if !self.retune_ok.load(std::sync::atomic::Ordering::Relaxed) {
                return false;
            }
            self.configured = Some(tier.bitrate);
            true
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
    fn tier(name: &str, bitrate: u32) -> super::super::adaptive_bitrate::QualityTier {
        super::super::adaptive_bitrate::QualityTier {
            name: name.into(),
            bitrate,
            width: None,
            height: None,
            fps: None,
        }
    }

    #[cfg(feature = "source")]
    fn tiered_mock(id: &str) -> MockSource {
        let mut source = MockSource::new(id);
        source.tiers = vec![tier("low", 600_000), tier("mid", 2_000_000)];
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
            super::TierResolution::Resolved(tier("mid", 2_000_000))
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

    #[cfg(feature = "source")]
    #[tokio::test]
    async fn set_source_bitrate_reports_adaptive_suspension() {
        let manager = SourceManager::new();

        // Fixed source with a control (the state every native source gets
        // in `create_bridge`): no controller runs, so nothing suspends.
        manager
            .add_source(Box::new(tiered_mock("fixed")))
            .await
            .unwrap();
        manager.bitrate_controls.write().await.insert(
            "fixed".to_string(),
            std::sync::Arc::new(super::super::adaptive_bitrate::BitrateControl::new(
                4_000_000,
            )),
        );
        assert_eq!(
            manager.set_source_bitrate("fixed", 1_000_000).await,
            super::SetBitrateOutcome::Applied {
                adaptive_suspended: false
            }
        );

        // Adaptive source: the manual set suspends its controller.
        let mut adaptive_mock = tiered_mock("adaptive");
        adaptive_mock.adaptive = true;
        manager.add_source(Box::new(adaptive_mock)).await.unwrap();
        manager.bitrate_controls.write().await.insert(
            "adaptive".to_string(),
            std::sync::Arc::new(super::super::adaptive_bitrate::BitrateControl::new(
                4_000_000,
            )),
        );
        assert_eq!(
            manager.set_source_bitrate("adaptive", 1_000_000).await,
            super::SetBitrateOutcome::Applied {
                adaptive_suspended: true
            }
        );

        // The manual override is recorded either way.
        for id in ["fixed", "adaptive"] {
            let control = manager.bitrate_controls.read().await;
            assert_eq!(control[id].manual(), Some(1_000_000));
        }

        // Missing source and missing control both keep their outcomes.
        assert_eq!(
            manager.set_source_bitrate("other", 1_000_000).await,
            super::SetBitrateOutcome::SourceNotFound
        );
        manager
            .add_source(Box::new(tiered_mock("no-control")))
            .await
            .unwrap();
        assert_eq!(
            manager.set_source_bitrate("no-control", 1_000_000).await,
            super::SetBitrateOutcome::Applied {
                adaptive_suspended: false
            }
        );
    }

    #[cfg(feature = "source")]
    #[tokio::test]
    async fn set_source_bitrate_rejects_above_ceiling() {
        let manager = SourceManager::new();
        let mut mock = tiered_mock("cam");
        mock.configured = Some(4_000_000);
        manager.add_source(Box::new(mock)).await.unwrap();

        assert_eq!(
            manager.set_source_bitrate("cam", 5_000_000).await,
            super::SetBitrateOutcome::AboveCeiling(4_000_000)
        );
        // The ceiling itself is accepted.
        assert_eq!(
            manager.set_source_bitrate("cam", 4_000_000).await,
            super::SetBitrateOutcome::Applied {
                adaptive_suspended: false
            }
        );

        // No configured bitrate: the only bound is the signed 32-bit
        // encoder control channel.
        manager
            .add_source(Box::new(tiered_mock("noceiling")))
            .await
            .unwrap();
        assert_eq!(
            manager
                .set_source_bitrate("noceiling", i32::MAX as u32 + 1)
                .await,
            super::SetBitrateOutcome::AboveCeiling(i32::MAX as u32)
        );
    }

    #[cfg(feature = "source")]
    #[tokio::test]
    async fn source_bitrate_info_falls_back_to_configured_bitrate() {
        let manager = SourceManager::new();
        let mut mock = tiered_mock("cam");
        mock.configured = Some(4_000_000);
        manager.add_source(Box::new(mock)).await.unwrap();

        // Standby (no bridge/control yet): report the configured value,
        // which is where the encoder will start.
        let info = manager.source_bitrate_info("cam").await.unwrap();
        assert_eq!(info.current, Some(4_000_000));
    }

    #[cfg(feature = "source")]
    #[tokio::test]
    async fn clear_manual_bitrate_restores_configured_on_fixed_source() {
        let manager = SourceManager::new();
        let mut mock = tiered_mock("cam");
        mock.configured = Some(4_000_000);
        manager.add_source(Box::new(mock)).await.unwrap();
        manager.bitrate_controls.write().await.insert(
            "cam".to_string(),
            std::sync::Arc::new(super::super::adaptive_bitrate::BitrateControl::new(
                4_000_000,
            )),
        );

        manager.set_source_bitrate("cam", 1_000_000).await;

        assert_eq!(
            manager.clear_manual_bitrate("cam").await,
            Some(super::ClearBitrateOutcome::Cleared {
                bitrate: 4_000_000,
                adaptive: false,
            })
        );
        let control = manager.bitrate_controls.read().await;
        assert_eq!(control["cam"].manual(), None);
        assert_eq!(control["cam"].current(), 4_000_000);
    }

    #[cfg(feature = "source")]
    #[tokio::test]
    async fn clear_manual_bitrate_keeps_override_when_restore_fails() {
        let manager = SourceManager::new();
        let mut mock = tiered_mock("cam");
        mock.configured = Some(4_000_000);
        let retune_ok = mock.retune_ok.clone();
        manager.add_source(Box::new(mock)).await.unwrap();
        manager.bitrate_controls.write().await.insert(
            "cam".to_string(),
            std::sync::Arc::new(super::super::adaptive_bitrate::BitrateControl::new(
                4_000_000,
            )),
        );

        manager.set_source_bitrate("cam", 1_000_000).await;

        // The backend starts rejecting retunes: the clear must not pretend
        // the encoder went back to the configured value.
        retune_ok.store(false, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            manager.clear_manual_bitrate("cam").await,
            Some(super::ClearBitrateOutcome::RestoreFailed)
        );
        let control = manager.bitrate_controls.read().await;
        assert_eq!(control["cam"].manual(), Some(1_000_000));
    }

    #[cfg(feature = "source")]
    #[tokio::test]
    async fn apply_source_tier_rebuilds_and_records_manual() {
        let manager = SourceManager::new();
        let mut mock = tiered_mock("cam");
        mock.configured = Some(4_000_000);
        mock.adaptive = true;
        manager.add_source(Box::new(mock)).await.unwrap();
        manager.bitrate_controls.write().await.insert(
            "cam".to_string(),
            std::sync::Arc::new(super::super::adaptive_bitrate::BitrateControl::new(
                4_000_000,
            )),
        );

        let mut res_tier = tier("low", 600_000);
        res_tier.width = Some(640);
        res_tier.height = Some(480);
        res_tier.fps = Some(15);

        // Adaptive source: the rebuild suspends the controller and records
        // the tier as the manual override.
        assert_eq!(
            manager.apply_source_tier("cam", &res_tier).await,
            super::SetBitrateOutcome::Applied {
                adaptive_suspended: true
            }
        );
        {
            let control = manager.bitrate_controls.read().await;
            assert_eq!(control["cam"].manual(), Some(600_000));
        }
        let info = manager.source_bitrate_info("cam").await.unwrap();
        assert_eq!(info.mode, super::BitrateMode::Manual);
        assert_eq!(info.active_tier.as_deref(), Some("low"));

        // Missing source keeps its outcome.
        assert_eq!(
            manager.apply_source_tier("ghost", &res_tier).await,
            super::SetBitrateOutcome::SourceNotFound
        );

        // A rejected rebuild reports Unsupported.
        let mut mock = tiered_mock("broken");
        mock.configured = Some(4_000_000);
        let retune_ok = mock.retune_ok.clone();
        manager.add_source(Box::new(mock)).await.unwrap();
        retune_ok.store(false, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            manager.apply_source_tier("broken", &res_tier).await,
            super::SetBitrateOutcome::Unsupported
        );
    }
}
