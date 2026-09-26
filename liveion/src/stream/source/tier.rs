//! Source quality tiers (ladder rungs) — an *external*, operator-driven
//! source-level feature, distinct from the encoder-internal adaptive
//! bitrate control in `adaptive_bitrate.rs`.
//!
//! A tier names a preset group of source parameters: optional capture
//! (`width`/`height`/`fps`) and encoder (`bitrate`) overlays.  Applying
//! a tier re-provisions the source: encoder-only tiers retune the
//! running encoder in place, while a tier touching capture geometry
//! rebuilds the capture+encoder pipeline underneath the stream.  The
//! same mechanism can later grow to switch source *types* (e.g. from a
//! native capture source to an RTSP URL) — tiers deliberately describe
//! source configuration, not encoder internals.

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

/// The state a source is actually running with (native sources only):
/// the last applied tier's name plus the pipeline's current capture
/// geometry and encoder bitrate.  Unlike [`QualityTier`] these are
/// *effective* values — always fully resolved, never "inherit the base
/// config".  Used to rebuild hook state after a failed tier apply rolls
/// the pipeline back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTierState {
    /// The tier last applied through the admin API; `None` = the
    /// configured base profile.
    pub name: Option<String>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
}
