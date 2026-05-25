//! Libcamera-Bridge Source (SourcePipeline FFI Edition).
//!
//! Thin wrapper around [`NativeEncodedSource`](super::native_encoded_source::NativeEncodedSource).
//! The only libcamera-specific logic is building `NativeSourceParams` with
//! `capture_backend = "libcamera"` from the legacy URL parameters.

use super::native_encoded_source::{NativeEncodedSource, NativeSourceParams};
use super::stream_config_v2::SourceSpec;
use super::{MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};

#[cfg(feature = "source")]
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters;

pub struct LibcameraSource {
    inner: NativeEncodedSource,
}

impl LibcameraSource {
    pub fn from_url(url: &str, config: &crate::config::SourceConfig) -> Result<Self> {
        let params = super::stream_config_v2::parse_libcamera_url(url)?;

        let native_params = NativeSourceParams {
            capture_backend: "libcamera".into(),
            capture_device: format!("{}", params.camera_id),
            width: params.width,
            height: params.height,
            fps: params.fps,
            capture_pixel_format: 2, // Yuv420p
            encoder_backend: "v4l2_m2m".into(),
            codec: 100, // H264
            bitrate: params.bitrate,
            profile: params.profile.clone(),
            gop: 60,
            payload_type: params.payload_type as u32,
            clock_rate: params.clock_rate,
            #[cfg(feature = "source")]
            codec_name: params.codec.clone(),
            #[cfg(feature = "source")]
            default_profile: params.profile.clone(),
        };

        Ok(Self {
            inner: NativeEncodedSource::new(config.stream_id.clone(), native_params),
        })
    }

    /// Create a `LibcameraSource` directly from a structured `SourceSpec`.
    ///
    /// This bypasses the legacy URL roundtrip — `capture.backend`, `encoder.*`,
    /// and `output.*` fields map directly to `NativeSourceParams`.
    pub fn from_spec(spec: &SourceSpec) -> Result<Self> {
        let native_params = spec.to_native_params()?;
        Ok(Self {
            inner: NativeEncodedSource::new(spec.stream_id.clone(), native_params),
        })
    }
}

#[async_trait]
impl StreamSource for LibcameraSource {
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
