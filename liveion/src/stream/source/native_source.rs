//! Unified native source wrapper.
//!
//! Both libcamera and V4L2 sources are thin wrappers around
//! [`NativeEncodedSource`].  The only difference is the
//! `NativeSourceParams` they construct — everything else is identical.
//! This module merges them into a single `NativeSource` struct.
//!
//! Only the structured config path (`SourceSpec` → `NativeSourceParams`)
//! is supported.  Legacy URL-based config has been removed.

use super::native_encoded_source::NativeEncodedSource;
use super::stream_config_v2::SourceSpec;
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
        let native_params = spec.to_native_params()?;
        Ok(Self {
            inner: NativeEncodedSource::new(spec.stream_id.clone(), native_params),
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
}
