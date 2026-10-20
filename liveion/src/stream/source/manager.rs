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

/// Outcome of resolving a configured tier name (admin API).
#[cfg(feature = "source")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TierResolution {
    Resolved(super::tier::QualityTier),
    SourceNotFound,
    TierNotFound,
}

/// Outcome of applying a quality tier (admin API).
#[cfg(feature = "source")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyTierOutcome {
    /// The tier was applied (seamless retune or pipeline rebuild).
    Applied,
    /// No source is registered for the stream.
    SourceNotFound,
    /// The source could not apply the tier (not running, or the rebuild
    /// failed and was rolled back).
    Unsupported,
    /// A synchronous `on_source_changed` hook failed and the apply
    /// was aborted before touching the pipeline.
    HookFailed,
}

/// `on_source_changed` hook configuration for source tier applies (see
/// [`crate::config::HookConfig::on_source_changed`]).
#[cfg(feature = "source")]
#[derive(Clone, Default)]
pub struct TierHooks {
    /// Global `[hooks]` scripts, run before per-stream ones.
    pub global: Vec<String>,
    /// Per-stream `[stream.<name>.hooks]` scripts.
    pub per_stream: HashMap<String, Vec<String>>,
    /// Per-script timeout (`None` = no timeout).
    pub timeout: Option<Duration>,
    /// Whether a failing script aborts the apply.
    pub on_error: crate::config::OnError,
}

#[cfg(feature = "source")]
impl TierHooks {
    /// Build from the liveion config: global `[hooks]` plus every
    /// `[stream.<name>.hooks]` entry that declares tier hooks.
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self {
            global: config.hooks.hooks.on_source_changed.clone(),
            per_stream: config
                .stream
                .streams
                .iter()
                .map(|(name, e)| (name.clone(), e.hooks.on_source_changed.clone()))
                .filter(|(_, scripts)| !scripts.is_empty())
                .collect(),
            timeout: (config.hooks.timeout_ms > 0)
                .then(|| Duration::from_millis(config.hooks.timeout_ms)),
            on_error: config.hooks.on_error,
        }
    }

    /// Whether any `on_source_changed` hook is configured at all.
    pub fn is_empty(&self) -> bool {
        self.global.is_empty() && self.per_stream.is_empty()
    }
}

/// How the stream source's encoder bitrate is currently driven.
#[cfg(feature = "source")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitrateMode {
    /// The AIMD controller drives the encoder from subscriber feedback.
    Adaptive,
    /// No adaptive controller — the encoder runs at its configured
    /// bitrate (or the bitrate of the last applied tier).
    Fixed,
}

impl BitrateMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            BitrateMode::Adaptive => "adaptive",
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
    /// Whether the source opted into adaptive bitrate.
    pub adaptive: bool,
}

/// Snapshot of a stream source's quality-tier state (admin API).
#[cfg(feature = "source")]
#[derive(Debug, Clone)]
pub struct SourceTierInfo {
    /// The last tier applied through the admin API (`None` = the
    /// configured base profile).
    pub active_tier: Option<String>,
    /// Configured quality tiers (empty when the source defines none).
    pub tiers: Vec<super::tier::QualityTier>,
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

    /// Synchronous `on_source_changed` hooks run before tier applies.
    #[cfg(feature = "source")]
    tier_hooks: TierHooks,

    /// Serializes prepare+apply so rapid admin tier calls cannot interleave
    /// a hardware prepare with another tier's rebuild.
    #[cfg(feature = "source")]
    tier_apply_lock: Arc<tokio::sync::Mutex<()>>,
}

impl SourceManager {
    pub fn new() -> Self {
        Self {
            sources: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "source")]
            bridges: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "source")]
            bitrate_controls: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "source")]
            tier_hooks: TierHooks::default(),
            #[cfg(feature = "source")]
            tier_apply_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Construct with `on_source_changed` hooks from the liveion config.
    #[cfg(feature = "source")]
    pub fn with_tier_hooks(config: &crate::config::Config) -> Self {
        let mut manager = Self::new();
        manager.tier_hooks = TierHooks::from_config(config);
        manager
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

    /// Resolve a configured tier name to its bitrate for a stream's source.
    #[cfg(feature = "source")]
    pub async fn resolve_tier(&self, stream_id: &str, tier: &str) -> TierResolution {
        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        };
        let Some(source) = source else {
            return TierResolution::SourceNotFound;
        };
        let tiers = source.lock().await.tiers();
        match tiers.into_iter().find(|t| t.name == tier) {
            Some(t) => TierResolution::Resolved(t),
            None => TierResolution::TierNotFound,
        }
    }

    /// Apply a quality tier (admin API): re-provision the stream source
    /// with the tier's geometry and bitrate.  Bitrate-only tiers retune
    /// the running encoder in place; geometry tiers rebuild the pipeline
    /// underneath the stream (subscribers stay attached across the gap).
    /// The AIMD is not suspended — the tier's bitrate becomes its new
    /// rung ceiling and resume seed.  Configured `on_source_changed`
    /// hooks run synchronously before the re-provisioning; a hook failure
    /// aborts the apply (`on_error = "stop"`).  When the apply does not
    /// reach the target state — prepare aborted, or the rebuild failed
    /// and rolled back — the hooks run again with the source's *current*
    /// state so hardware they switched (e.g. a camera sensor's mode gear)
    /// is switched back.
    #[cfg(feature = "source")]
    pub async fn apply_source_tier(
        &self,
        stream_id: &str,
        tier: &super::tier::QualityTier,
    ) -> ApplyTierOutcome {
        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        };
        let Some(source) = source else {
            return ApplyTierOutcome::SourceNotFound;
        };

        // Prepare+apply is one critical section: a hardware gear switched by
        // a tier hook must not interleave with another tier's rebuild.
        let _tier_guard = self.tier_apply_lock.lock().await;
        if !self.tier_hooks.is_empty() {
            let stream_scripts = self.stream_tier_scripts(stream_id);
            if crate::hook::run_tier_hooks(
                &self.tier_hooks.global,
                stream_scripts,
                stream_id,
                &crate::hook::TierEnv {
                    name: tier.name.clone(),
                    width: tier.width,
                    height: tier.height,
                    fps: tier.fps,
                    bitrate: tier.bitrate,
                },
                self.tier_hooks.timeout,
                self.tier_hooks.on_error,
            )
            .await
            .is_err()
            {
                // The pipeline was never touched, but earlier scripts in
                // the batch may have switched hardware — re-align it with
                // the still-current state.
                let state = source.lock().await.active_tier_state();
                if let Some(state) = state {
                    self.run_tier_hooks_with_state(stream_id, state).await;
                }
                return ApplyTierOutcome::HookFailed;
            }
        }

        // The rebuild swaps the whole pipeline; the adaptive controller
        // never touches the encoder mid-rebuild because it only applies
        // decisions under this same source lock.
        let mut source_guard = source.lock().await;
        if !source_guard.apply_tier(tier).await {
            let restored = source_guard.active_tier_state();
            drop(source_guard);
            // The pipeline rolled back to its previous params; re-align
            // hook-driven hardware with the restored state.
            if let Some(state) = restored {
                self.run_tier_hooks_with_state(stream_id, state).await;
            }
            return ApplyTierOutcome::Unsupported;
        }

        if let Some(control) = self.bitrate_controls.read().await.get(stream_id) {
            control.apply_tier(tier.bitrate);
        }
        drop(source_guard);
        ApplyTierOutcome::Applied
    }

    /// The stream's per-stream tier hook scripts (`[]` when unset).
    #[cfg(feature = "source")]
    fn stream_tier_scripts(&self, stream_id: &str) -> &[String] {
        self.tier_hooks
            .per_stream
            .get(stream_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Re-run the tier hooks against the state the source is *currently*
    /// running with, after a failed tier apply: the pre-apply run switched
    /// hardware for the target tier (e.g. a camera sensor's mode gear, on
    /// platforms where framerate is a sensor-mode property), and the
    /// pipeline rolled back — without this the hardware and the pipeline
    /// diverge.  Best effort: runs with [`crate::config::OnError::Continue`]
    /// and ignores the result, the apply is already failing.
    #[cfg(feature = "source")]
    async fn run_tier_hooks_with_state(
        &self,
        stream_id: &str,
        state: super::tier::ActiveTierState,
    ) {
        let _ = crate::hook::run_tier_hooks(
            &self.tier_hooks.global,
            self.stream_tier_scripts(stream_id),
            stream_id,
            &crate::hook::TierEnv {
                name: state.name.unwrap_or_default(),
                width: Some(state.width),
                height: Some(state.height),
                fps: Some(state.fps),
                bitrate: state.bitrate,
            },
            self.tier_hooks.timeout,
            crate::config::OnError::Continue,
        )
        .await;
    }

    /// Snapshot of a stream source's bitrate state for the admin API.
    #[cfg(feature = "source")]
    pub async fn source_bitrate_info(&self, stream_id: &str) -> Option<SourceBitrateInfo> {
        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        }?;
        let (adaptive, configured) = {
            let source_guard = source.lock().await;
            (
                source_guard.adaptive_bitrate_config().is_some(),
                source_guard.configured_bitrate(),
            )
        };

        let control = self.bitrate_controls.read().await.get(stream_id).cloned();
        // No control yet (source in standby, no bridge): report the
        // configured bitrate, which is where the encoder will start.
        let current = control.as_ref().map(|c| c.current()).or(configured);
        let mode = if adaptive {
            BitrateMode::Adaptive
        } else {
            BitrateMode::Fixed
        };

        Some(SourceBitrateInfo {
            mode,
            current,
            adaptive,
        })
    }

    /// Snapshot of a stream source's quality-tier state for the admin API.
    #[cfg(feature = "source")]
    pub async fn source_tier_info(&self, stream_id: &str) -> Option<SourceTierInfo> {
        let source = {
            let sources = self.sources.read().await;
            sources.get(stream_id).cloned()
        }?;
        let source_guard = source.lock().await;
        Some(SourceTierInfo {
            active_tier: source_guard.active_tier(),
            tiers: source_guard.tiers(),
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
        tiers: Vec<super::super::tier::QualityTier>,
        #[cfg(feature = "source")]
        active_tier: Option<String>,
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
                active_tier: None,
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
        fn tiers(&self) -> Vec<super::super::tier::QualityTier> {
            self.tiers.clone()
        }

        #[cfg(feature = "source")]
        fn active_tier(&self) -> Option<String> {
            self.active_tier.clone()
        }

        #[cfg(feature = "source")]
        fn active_tier_state(&self) -> Option<super::super::tier::ActiveTierState> {
            Some(super::super::tier::ActiveTierState {
                name: self.active_tier.clone(),
                width: 1920,
                height: 1080,
                fps: 30,
                bitrate: self.configured.unwrap_or(4_000_000),
            })
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
        async fn apply_tier(&mut self, tier: &super::super::tier::QualityTier) -> bool {
            if !self.retune_ok.load(std::sync::atomic::Ordering::Relaxed) {
                return false;
            }
            self.configured = Some(tier.bitrate);
            self.active_tier = Some(tier.name.clone());
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
    fn tier(name: &str, bitrate: u32) -> super::super::tier::QualityTier {
        super::super::tier::QualityTier {
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
    async fn resolve_tier_matches_configured_names() {
        let manager = SourceManager::new();
        manager
            .add_source(Box::new(tiered_mock("cam")))
            .await
            .unwrap();

        assert_eq!(
            manager.resolve_tier("cam", "mid").await,
            super::TierResolution::Resolved(tier("mid", 2_000_000))
        );
        assert_eq!(
            manager.resolve_tier("cam", "high").await,
            super::TierResolution::TierNotFound
        );
        assert_eq!(
            manager.resolve_tier("other", "mid").await,
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
        // MockSource has no configured bitrate: no control, no current.
        assert!(info.current.is_none());
        assert!(manager.source_bitrate_info("other").await.is_none());
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
    async fn apply_source_tier_moves_rung_ceiling() {
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

        // The tier apply moves the rung ceiling and adopts the bitrate;
        // the AIMD keeps running within the rung.
        assert_eq!(
            manager.apply_source_tier("cam", &res_tier).await,
            super::ApplyTierOutcome::Applied
        );
        {
            let control = manager.bitrate_controls.read().await;
            assert_eq!(control["cam"].rung_target(), 600_000);
            assert_eq!(control["cam"].current(), 600_000);
        }
        let info = manager.source_bitrate_info("cam").await.unwrap();
        assert_eq!(info.mode, super::BitrateMode::Adaptive);
        let tier_info = manager.source_tier_info("cam").await.unwrap();
        assert_eq!(tier_info.active_tier.as_deref(), Some("low"));
        assert_eq!(tier_info.tiers.len(), 2);

        // Missing source keeps its outcome.
        assert_eq!(
            manager.apply_source_tier("ghost", &res_tier).await,
            super::ApplyTierOutcome::SourceNotFound
        );

        // A rejected rebuild reports Unsupported.
        let mut mock = tiered_mock("broken");
        mock.configured = Some(4_000_000);
        let retune_ok = mock.retune_ok.clone();
        manager.add_source(Box::new(mock)).await.unwrap();
        retune_ok.store(false, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            manager.apply_source_tier("broken", &res_tier).await,
            super::ApplyTierOutcome::Unsupported
        );
    }

    /// Append-to-log shell script; each run appends one line with the
    /// hook contract values.
    #[cfg(all(unix, feature = "source"))]
    fn tier_log_script(dir: &std::path::Path, name: &str, log: &std::path::Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$1 [$2] $LIVE777_SOURCE_WIDTH $LIVE777_SOURCE_HEIGHT $LIVE777_SOURCE_FPS $LIVE777_SOURCE_BITRATE\" >> {}\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[cfg(all(unix, feature = "source"))]
    #[tokio::test]
    async fn apply_source_tier_runs_tier_hooks_with_declared_env() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("tier.log");
        let script = tier_log_script(dir.path(), "tier.sh", &log);

        let mut manager = SourceManager::new();
        manager.tier_hooks = super::TierHooks {
            global: vec![script],
            ..Default::default()
        };
        let mut mock = tiered_mock("cam");
        mock.configured = Some(4_000_000);
        manager.add_source(Box::new(mock)).await.unwrap();

        let mut rung = tier("hd60", 1_000_000);
        rung.width = Some(1280);
        rung.height = Some(720);
        assert_eq!(
            manager.apply_source_tier("cam", &rung).await,
            super::ApplyTierOutcome::Applied
        );
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "cam [hd60] 1280 720  1000000\n"
        );
    }

    #[cfg(all(unix, feature = "source"))]
    #[tokio::test]
    async fn apply_source_tier_failure_reruns_hooks_with_current_state() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("tier.log");
        let script = tier_log_script(dir.path(), "tier.sh", &log);

        let mut manager = SourceManager::new();
        manager.tier_hooks = super::TierHooks {
            global: vec![script],
            ..Default::default()
        };
        let mut mock = tiered_mock("cam");
        mock.configured = Some(4_000_000);
        let retune_ok = mock.retune_ok.clone();
        manager.add_source(Box::new(mock)).await.unwrap();
        retune_ok.store(false, std::sync::atomic::Ordering::Relaxed);

        let mut rung = tier("hd60", 1_000_000);
        rung.width = Some(1280);
        rung.height = Some(720);
        assert_eq!(
            manager.apply_source_tier("cam", &rung).await,
            super::ApplyTierOutcome::Unsupported
        );
        // The pre-apply run exported the declared tier overlay; the
        // compensation run exported the pipeline's actual current values
        // (base profile: empty tier name, mock geometry/bitrate).
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "cam [hd60] 1280 720  1000000\n\
             cam [] 1920 1080 30 4000000\n"
        );
    }

    #[cfg(all(unix, feature = "source"))]
    #[tokio::test]
    async fn apply_source_tier_hook_failure_aborts_and_compensates() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("tier.log");
        // The hook itself fails: it logs its run, then exits 1.
        use std::os::unix::fs::PermissionsExt;
        let fail = dir.path().join("fail.sh");
        std::fs::write(
            &fail,
            format!(
                "#!/bin/sh\necho \"ran [$2]\" >> {}\nexit 1\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fail, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut manager = SourceManager::new();
        manager.tier_hooks = super::TierHooks {
            global: vec![fail.to_str().unwrap().to_string()],
            on_error: crate::config::OnError::Stop,
            ..Default::default()
        };
        let mut mock = tiered_mock("cam");
        mock.configured = Some(4_000_000);
        manager.add_source(Box::new(mock)).await.unwrap();

        assert_eq!(
            manager
                .apply_source_tier("cam", &tier("hd60", 1_000_000))
                .await,
            super::ApplyTierOutcome::HookFailed
        );
        // The pipeline was never touched, but the hooks still re-ran with
        // the current state (compensation for partial prepare side
        // effects) — the failing script logged both runs.
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "ran [hd60]\nran []\n"
        );
    }
}
