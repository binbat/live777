//! Source quality tiers (ladder rungs) — an *external*, operator-driven
//! source-level feature, distinct from the encoder-internal adaptive
//! bitrate control in `adaptive_bitrate.rs`.
//!
//! A tier names a source configuration preset: an encoder bitrate plus
//! optional capture geometry (`width`/`height`/`fps`).  Applying a tier
//! re-provisions the source: bitrate-only tiers retune the running
//! encoder in place, while a tier carrying geometry rebuilds the
//! capture+encoder pipeline underneath the stream.  The same mechanism
//! can later grow to switch source *types* (e.g. from a native capture
//! source to an RTSP URL) — tiers deliberately describe source
//! configuration, not encoder internals.

/// A named quality tier from the source config, switchable through the
/// admin API (`POST /api/sources/{stream}/tier`).
///
/// Unset geometry fields overlay on the configured capture params, so a
/// mixed ladder (bitrate-only and geometry tiers side by side) is
/// well-defined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityTier {
    pub name: String,
    pub bitrate: u32,
    /// Tier capture width; `None` keeps the configured capture size.
    pub width: Option<u32>,
    /// Tier capture height; `None` keeps the configured capture size.
    pub height: Option<u32>,
    /// Tier capture framerate; `None` keeps the configured rate.
    pub fps: Option<u32>,
}

impl QualityTier {
    /// Whether applying this tier needs a pipeline rebuild (it carries
    /// geometry) rather than an in-place encoder retune.
    pub fn needs_rebuild(&self) -> bool {
        self.width.is_some() || self.height.is_some() || self.fps.is_some()
    }
}
