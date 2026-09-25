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
    /// Configured (spec) pipeline params — the ladder's top rung and the
    /// fallback for tier fields a tier leaves unset.
    base_params: livehal::NativeSourceParams,
}

impl NativeSource {
    pub fn from_spec(spec: &SourceSpec) -> Result<Self> {
        spec.validate()?;
        let native_params = spec.to_native_params()?;
        let tiers: Vec<super::tier::QualityTier> = spec
            .tiers
            .iter()
            .map(|t| super::tier::QualityTier {
                name: t.name.clone(),
                // Unset fields overlay on the configured capture — the
                // derivation sees the tier's effective geometry.
                bitrate: t.bitrate.unwrap_or_else(|| {
                    super::source_config::TierSpec::derived_bitrate(
                        t.width.unwrap_or(spec.capture.width),
                        t.height.unwrap_or(spec.capture.height),
                        t.fps.unwrap_or(spec.capture.fps),
                    )
                }),
                width: t.width,
                height: t.height,
                fps: t.fps,
            })
            .collect();
        let adaptive = spec.encoder.adaptive_bitrate.then(|| {
            super::adaptive_bitrate::AdaptiveBitrateConfig::new(
                spec.encoder.bitrate,
                spec.encoder.min_bitrate,
            )
        });
        Ok(Self {
            inner: NativeEncodedSource::new(
                spec.stream_id.clone(),
                native_params.clone(),
                adaptive,
                tiers,
            ),
            base_params: native_params,
        })
    }

    /// Effective pipeline params for a tier: the configured (spec) values
    /// with the tier's geometry/bitrate overlaid.
    #[cfg(feature = "source")]
    fn tier_params(&self, tier: &super::tier::QualityTier) -> livehal::NativeSourceParams {
        let mut params = self.base_params.clone();
        params.width = tier.width.unwrap_or(params.width);
        params.height = tier.height.unwrap_or(params.height);
        params.fps = tier.fps.unwrap_or(params.fps);
        params.bitrate = tier.bitrate;
        params
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
    async fn apply_tier(&mut self, tier: &super::tier::QualityTier) -> bool {
        // Bitrate-only rung: seamless in-place retune, no rebuild.
        if !tier.needs_rebuild() {
            if !self.inner.set_bitrate(tier.bitrate) {
                return false;
            }
            self.inner.set_active_tier(Some(tier.name.clone()));
            return true;
        }
        let params = self.tier_params(tier);
        if self.inner.reconfigure(params).await.is_ok() {
            self.inner.set_active_tier(Some(tier.name.clone()));
            true
        } else {
            false
        }
    }
}
