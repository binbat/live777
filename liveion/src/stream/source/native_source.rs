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
}

impl NativeSource {
    pub fn from_spec(spec: &SourceSpec) -> Result<Self> {
        spec.validate()?;
        let native_params = spec.to_native_params()?;
        let tiers: Vec<super::adaptive_bitrate::BitrateTier> = spec
            .tiers
            .iter()
            .map(|t| super::adaptive_bitrate::BitrateTier {
                name: t.name.clone(),
                bitrate: t.bitrate,
            })
            .collect();
        let adaptive = spec.encoder.adaptive_bitrate.then(|| {
            // Explicit min_bitrate wins; with tiers configured the AIMD
            // floor defaults to the lowest tier instead of the generic
            // max(target / 8, 300 kbps).
            let min = spec
                .encoder
                .min_bitrate
                .or_else(|| tiers.iter().map(|t| t.bitrate).min());
            super::adaptive_bitrate::AdaptiveBitrateConfig::new(spec.encoder.bitrate, min)
        });
        Ok(Self {
            inner: NativeEncodedSource::new(spec.stream_id.clone(), native_params, adaptive, tiers),
        })
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
    fn bitrate_tiers(&self) -> Vec<super::adaptive_bitrate::BitrateTier> {
        self.inner.bitrate_tiers()
    }
}
