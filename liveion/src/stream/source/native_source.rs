//! Unified native source wrapper.
//!
//! Both libcamera and V4L2 sources are thin wrappers around
//! [`NativeEncodedSource`].  The only difference is the
//! `NativeSourceParams` they construct — everything else is identical.
//! This module merges them into a single `NativeSource` struct.

use super::native_encoded_source::NativeEncodedSource;
use super::stream_config_v2::SourceSpec;
use livesrc::NativeSourceParams;
use super::{MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};

#[cfg(feature = "source")]
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;

pub struct NativeSource {
    inner: NativeEncodedSource,
}

impl NativeSource {
    /// Create from a legacy URL (`libcamera://...` or `v4l2://...`).
    pub fn from_url(
        url: &str,
        config: &crate::config::SourceConfig,
    ) -> Result<Self> {
        let native_params = if url.starts_with("libcamera://") {
            let params = super::stream_config_v2::parse_libcamera_url(url)?;
            NativeSourceParams {
                capture_backend: "libcamera".into(),
                capture_device: format!("{}", params.camera_id),
                width: params.width,
                height: params.height,
                fps: params.fps,
                capture_pixel_format: 2, // Yuv420p
                encoder_backend: "v4l2-m2m".into(),
                codec: 100, // H264
                bitrate: params.bitrate,
                profile: params.profile.clone(),
                gop: 60,
                payload_type: params.payload_type as u32,
                clock_rate: params.clock_rate,
                capture_prefer_dmabuf: 0,
                encoder_prefer_dmabuf: 0,
                codec_name: params.codec.clone(),
                default_profile: params.profile.clone(),
            }
        } else if url.starts_with("v4l2://") {
            let params = super::stream_config_v2::parse_v4l2_url(url)?;
            NativeSourceParams {
                capture_backend: "v4l2".into(),
                capture_device: params.device.clone(),
                width: params.width,
                height: params.height,
                fps: params.fps,
                capture_pixel_format: 0, // Yuyv422
                encoder_backend: "v4l2-m2m".into(),
                codec: 100, // H264
                bitrate: params.bitrate,
                profile: params.profile.clone(),
                gop: 60,
                payload_type: params.payload_type as u32,
                clock_rate: params.clock_rate,
                capture_prefer_dmabuf: 0,
                encoder_prefer_dmabuf: 0,
                codec_name: "H264".into(),
                default_profile: params.profile.clone(),
            }
        } else {
            anyhow::bail!("unsupported native source URL scheme: {}", url);
        };
        Ok(Self {
            inner: NativeEncodedSource::new(config.stream_id.clone(), native_params),
        })
    }

    /// Create directly from a structured `SourceSpec` — no URL roundtrip.
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
