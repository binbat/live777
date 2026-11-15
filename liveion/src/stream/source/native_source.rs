//! Unified native source wrapper.
//!
//! Both libcamera and V4L2 sources are thin wrappers around
//! [`NativeEncodedSource`].  The only difference is the
//! `NativeSourceParams` they construct — everything else is identical.
//! This module merges them into a single `NativeSource` struct.
//!
//! Native sources use structured per-stream `[[stream.<name>.sources]]` config
//! fields (`kind`, `capture`, `encoder`, `output`).

use super::native_encoded_source::NativeEncodedSource;
use super::source_config::SourceSpec;
use super::{MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};

#[cfg(feature = "source")]
use rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters;

pub struct NativeSource {
    inner: NativeEncodedSource,
    /// Configured (spec) video pipeline params — the ladder's top rung and
    /// the fallback for tier fields a tier leaves unset.  `None` on an
    /// audio-only source (which has no rebuildable video pipeline).
    base_params: Option<livehal::NativeSourceParams>,
}

impl NativeSource {
    pub fn from_spec(spec: &SourceSpec) -> Result<Self> {
        spec.validate()?;
        let video_params = spec.to_native_params()?;
        #[cfg(feature = "encoder-opus")]
        let audio_params = spec.to_audio_params();
        let tiers: Vec<super::tier::QualityTier> = spec
            .tiers
            .iter()
            .map(|t| super::tier::QualityTier {
                name: t.name.clone(),
                // Unset fields overlay on the configured capture — the
                // derivation sees the tier's effective geometry.  On an
                // audio-only source validation guarantees every tier has an
                // explicit bitrate, so the derivation never runs there.
                bitrate: t.encoder.bitrate.unwrap_or_else(|| {
                    let (width, height, fps) = spec
                        .capture
                        .as_ref()
                        .map(|c| (c.width, c.height, c.fps))
                        .unwrap_or((0, 0, 0));
                    super::source_config::TierSpec::derived_bitrate(width, height, fps)
                }),
                width: t.capture.width,
                height: t.capture.height,
                fps: t.capture.fps,
            })
            .collect();

        // Adaptive bitrate is on unless explicitly switched off.  The
        // floor defaults to the lowest rung when tiers are declared (the
        // ladder bottom is the natural floor).  A video leg drives the
        // rule as today (generic `max(target / 8, 300 kbps)` inside
        // AdaptiveBitrateConfig::new); on an audio-only source the AIMD
        // drives the Opus encoder with a floor of `max(bitrate / 4, 16
        // kbps)`.  With both legs the video rule wins and the audio leg's
        // own `adaptive` key is ignored.
        use super::source_config::AdaptiveConfig;
        let floor = tiers.iter().map(|t| t.bitrate).min();
        let adaptive = if let Some(encoder) = &spec.encoder {
            match encoder.adaptive {
                Some(AdaptiveConfig::Switch(false)) => None,
                Some(AdaptiveConfig::Knobs { min_bitrate }) => {
                    Some(super::adaptive_bitrate::AdaptiveBitrateConfig::new(
                        encoder.bitrate,
                        min_bitrate.or(floor),
                    ))
                }
                Some(AdaptiveConfig::Switch(true)) | None => Some(
                    super::adaptive_bitrate::AdaptiveBitrateConfig::new(encoder.bitrate, floor),
                ),
            }
        } else if let Some(audio_encoder) = &spec.audio_encoder {
            let audio_floor = (audio_encoder.bitrate / 4).max(16_000);
            match audio_encoder.adaptive {
                Some(AdaptiveConfig::Switch(false)) => None,
                Some(AdaptiveConfig::Knobs { min_bitrate }) => {
                    Some(super::adaptive_bitrate::AdaptiveBitrateConfig::new(
                        audio_encoder.bitrate,
                        min_bitrate.or(floor).or(Some(audio_floor)),
                    ))
                }
                Some(AdaptiveConfig::Switch(true)) | None => {
                    Some(super::adaptive_bitrate::AdaptiveBitrateConfig::new(
                        audio_encoder.bitrate,
                        floor.or(Some(audio_floor)),
                    ))
                }
            }
        } else {
            // Unreachable: validate() requires at least one complete leg.
            None
        };
        Ok(Self {
            inner: NativeEncodedSource::new(
                spec.stream_id.clone(),
                video_params.clone(),
                #[cfg(feature = "encoder-opus")]
                audio_params,
                adaptive,
                tiers,
            ),
            base_params: video_params,
        })
    }

    /// Effective pipeline params for a tier: the configured (spec) values
    /// with the tier's geometry/bitrate overlaid.  `None` on an audio-only
    /// source (no video pipeline to rebuild).
    #[cfg(feature = "source")]
    fn tier_params(&self, tier: &super::tier::QualityTier) -> Option<livehal::NativeSourceParams> {
        let mut params = self.base_params.clone()?;
        params.width = tier.width.unwrap_or(params.width);
        params.height = tier.height.unwrap_or(params.height);
        params.fps = tier.fps.unwrap_or(params.fps);
        params.bitrate = tier.bitrate;
        Some(params)
    }
}

#[async_trait]
impl StreamSource for NativeSource {
    fn stream_id(&self) -> &str {
        self.inner.stream_id()
    }

    fn state(&self) -> StreamSourceState {
        self.inner.state()
    }

    #[cfg(feature = "encoder-opus")]
    fn should_retry_start(&self, error: &anyhow::Error) -> bool {
        self.inner.should_retry_start(error)
    }

    async fn start(&mut self) -> Result<()> {
        self.inner.start().await
    }

    async fn stop(&mut self) -> Result<()> {
        self.inner.stop().await;
        Ok(())
    }

    fn subscribe_rtp(&self) -> broadcast::Receiver<MediaPacket> {
        self.inner.subscribe_rtp()
    }

    fn subscribe_state(&self) -> broadcast::Receiver<StateChangeEvent> {
        self.inner.subscribe_state()
    }

    #[cfg(feature = "source")]
    async fn get_video_codec(&self) -> Option<RTCRtpCodecParameters> {
        self.inner.get_video_codec().await
    }

    #[cfg(feature = "source")]
    async fn get_audio_codec(&self) -> Option<RTCRtpCodecParameters> {
        self.inner.get_audio_codec().await
    }

    #[cfg(feature = "source")]
    async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        self.inner.get_rtcp_sender().await
    }

    #[cfg(feature = "source")]
    fn adaptive_bitrate_config(&self) -> Option<super::adaptive_bitrate::AdaptiveBitrateConfig> {
        self.inner.adaptive_bitrate_config()
    }

    #[cfg(feature = "source")]
    async fn set_bitrate(&self, bps: u32) -> bool {
        self.inner.set_bitrate(bps)
    }

    #[cfg(feature = "source")]
    fn configured_bitrate(&self) -> Option<u32> {
        Some(self.inner.configured_bitrate())
    }

    #[cfg(feature = "source")]
    fn tiers(&self) -> Vec<super::tier::QualityTier> {
        self.inner.tiers()
    }

    #[cfg(feature = "source")]
    fn active_tier(&self) -> Option<String> {
        self.inner.active_tier()
    }

    #[cfg(feature = "source")]
    fn active_tier_state(&self) -> Option<super::tier::ActiveTierState> {
        Some(self.inner.active_tier_state())
    }

    #[cfg(feature = "source")]
    async fn apply_tier(&mut self, tier: &super::tier::QualityTier) -> bool {
        // Bitrate-only rung: seamless in-place retune, no rebuild.  On an
        // audio-only source this retunes the Opus encoder (see
        // NativeEncodedSource::set_bitrate).
        if !tier.needs_rebuild() {
            if !self.inner.set_bitrate(tier.bitrate) {
                return false;
            }
            self.inner.set_active_tier(Some(tier.name.clone()));
            return true;
        }
        // Geometry rung: rebuild the video pipeline.  Unreachable on an
        // audio-only source — validation rejects geometry tiers there.
        let Some(params) = self.tier_params(tier) else {
            return false;
        };
        if self.inner.reconfigure(params).await.is_ok() {
            self.inner.set_active_tier(Some(tier.name.clone()));
            true
        } else {
            false
        }
    }
}
