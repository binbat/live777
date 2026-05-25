//! NativeEncodedSource — shared Rust consumer of the C++ SourcePipeline FFI.
//!
//! Both `LibcameraSource` and `V4L2Source` are thin wrappers around this type.
//! The only difference is the `NativeSourceParams` they provide.
//!
//! Data flow:
//!   C++ SourcePipeline → EncodedPacketFFI callback → copy → Annex-B parse →
//!   SPS profile detect → H264 packetize → RTP broadcast
//!
//! The `EncodedPacketFFI.data` pointer is valid only within the FFI callback.
//! Data is immediately copied into a `Vec<u8>` before any processing.

use super::h264_utils::{AnnexBParser, H264Packetizer, NalType, parse_profile_level_id};
use super::native_ffi::*;
use super::{MediaPacket, StateChangeEvent, StreamSourceState};
use anyhow::Result;
use std::ffi::{c_char, CString};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::info;

const CHANNEL_VIDEO_RTP: u8 = 0;
const ERR_BUF_LEN: usize = 256;

// ---------------------------------------------------------------------------
// Shared pipeline handle — safe to clone across tokio tasks
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SharedPipelineHandle {
    inner: Arc<Mutex<Option<*mut SourcePipelineHandle>>>,
}

unsafe impl Send for SharedPipelineHandle {}
unsafe impl Sync for SharedPipelineHandle {}

impl SharedPipelineHandle {
    fn new() -> Self {
        Self { inner: Arc::new(Mutex::new(None)) }
    }

    fn set(&self, h: *mut SourcePipelineHandle) {
        *self.inner.lock().unwrap() = Some(h);
    }

    fn take(&self) -> Option<*mut SourcePipelineHandle> {
        self.inner.lock().unwrap().take()
    }

    fn is_some(&self) -> bool {
        self.inner.lock().unwrap().is_some()
    }

    /// Call source_pipeline_request_keyframe while holding the lock so
    /// that cleanup_pipeline() cannot free the handle concurrently.
    fn request_keyframe(&self) {
        let guard = self.inner.lock().unwrap();
        if let Some(h) = *guard {
            unsafe { source_pipeline_request_keyframe(h); }
        }
    }
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

pub struct NativeSourceParams {
    pub capture_backend: String,
    pub capture_device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub capture_pixel_format: u32,
    pub encoder_backend: String,
    pub codec: u32,
    pub bitrate: u32,
    pub profile: String,
    pub gop: u32,
    pub payload_type: u32,
    pub clock_rate: u32,
    #[cfg(feature = "source")]
    pub codec_name: String,
    #[cfg(feature = "source")]
    pub default_profile: String,
}

// ---------------------------------------------------------------------------
// NativeEncodedSource
// ---------------------------------------------------------------------------

pub struct NativeEncodedSource {
    stream_id: String,
    params: NativeSourceParams,
    state: Arc<RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    shutdown_tx: Option<broadcast::Sender<()>>,
    handle: SharedPipelineHandle,
    _cstrs: Option<Vec<CString>>,
    callback_ctx: Option<*mut CallbackCtx>,
    #[cfg(feature = "source")]
    dynamic_profile: Arc<RwLock<Option<String>>>,
}

unsafe impl Send for NativeEncodedSource {}
unsafe impl Sync for NativeEncodedSource {}

// ---------------------------------------------------------------------------
// FFI callback context — state that persists across callbacks
// ---------------------------------------------------------------------------

struct CallbackCtx {
    rtp_tx: broadcast::Sender<MediaPacket>,
    payload_type: u8,
    clock_rate: u32,
    start_instant: Instant,
    parser: Mutex<AnnexBParser>,
    packetizer: Mutex<H264Packetizer>,
    /// Last RTP timestamp emitted.  Used to compute deltas so each encoded
    /// packet gets a monotonic timestamp that does not regress.
    last_rtp_ts: Mutex<Option<u32>>,
    #[cfg(feature = "source")]
    dynamic_profile: Arc<RwLock<Option<String>>>,
}

// ---------------------------------------------------------------------------
// FFI callback — invoked from C++ encoder thread
// ---------------------------------------------------------------------------

unsafe extern "C" fn on_encoded_packet(
    pkt: *const EncodedPacketFFI,
    user_data: *mut std::ffi::c_void,
) {
    if pkt.is_null() || user_data.is_null() {
        return;
    }

    let pkt = unsafe { &*pkt };

    // Copy immediately — pkt.data is invalid after return
    let data = if pkt.size > 0 && !pkt.data.is_null() {
        unsafe { std::slice::from_raw_parts(pkt.data, pkt.size) }.to_vec()
    } else {
        return;
    };

    let pts_us = pkt.pts_us;
    let ctx = unsafe { &*(user_data as *const CallbackCtx) };

    // FFI us → RTP 90kHz
    let rtp_ts = if pts_us > 0 {
        (pts_us * 9 / 100) as u32
    } else {
        (ctx.start_instant.elapsed().as_micros() * 9 / 100) as u32
    };

    // Delta across callbacks: monotonic, no backward steps.
    // Stores the effective timestamp so the next callback sees a
    // monotonically-increasing baseline.
    let delta = {
        let mut last = ctx.last_rtp_ts.lock().unwrap();
        let (effective_ts, d) = match *last {
            Some(prev) if rtp_ts > prev => (rtp_ts, rtp_ts - prev),
            Some(prev) => {
                let next = prev.wrapping_add(3000);
                (next, 3000)
            }
            None => (rtp_ts, 0),
        };
        *last = Some(effective_ts);
        d
    };

    // Parse Annex-B
    let nals = {
        let mut parser = ctx.parser.lock().unwrap();
        parser.push(&data);
        parser.extract_nals()
    };

    // Packetize: advance once per encoded packet, then packetize all NALs
    let mut packetizer = ctx.packetizer.lock().unwrap();
    packetizer.advance_timestamp(delta);

    for nal in &nals {
        #[cfg(feature = "source")]
        if nal.nal_type == NalType::Sps {
            if let Some(profile) = parse_profile_level_id(&nal.data) {
                let mut guard = ctx.dynamic_profile.blocking_write();
                if guard.as_ref() != Some(&profile) {
                    *guard = Some(profile);
                }
            }
        }

        let rtp_packets = packetizer.packetize(nal);
        for packet in rtp_packets {
            let _ = ctx.rtp_tx.send(MediaPacket::Rtp {
                channel: CHANNEL_VIDEO_RTP,
                data: packet.to_bytes(),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// impl NativeEncodedSource
// ---------------------------------------------------------------------------

impl NativeEncodedSource {
    pub fn new(stream_id: String, params: NativeSourceParams) -> Self {
        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Self {
            stream_id,
            params,
            state: Arc::new(RwLock::new(StreamSourceState::Initializing)),
            rtp_tx,
            state_tx,
            shutdown_tx: None,
            handle: SharedPipelineHandle::new(),
            _cstrs: None,
            callback_ctx: None,
            #[cfg(feature = "source")]
            dynamic_profile: Arc::new(RwLock::new(None)),
        }
    }

    async fn set_state(&self, new_state: StreamSourceState, error: Option<String>) {
        let mut state = self.state.write().await;
        let old_state = *state;
        if old_state != new_state {
            *state = new_state;
            let _ = self.state_tx.send(StateChangeEvent {
                old_state,
                new_state,
                error: error.clone(),
            });
            info!(
                "[{}] state: {:?} -> {:?}{}",
                self.stream_id,
                old_state,
                new_state,
                error.map(|e| format!(" ({})", e)).unwrap_or_default()
            );
        }
    }

    fn build_ffi_config(params: &NativeSourceParams) -> (SourcePipelineConfigFFI, Vec<CString>) {
        let mut cstrs = Vec::new();

        let cap_backend = CString::new(params.capture_backend.as_str()).unwrap();
        let cap_device = CString::new(params.capture_device.as_str()).unwrap();
        let enc_backend = CString::new(params.encoder_backend.as_str()).unwrap();
        let profile = CString::new(params.profile.as_str()).unwrap();

        let cfg = SourcePipelineConfigFFI {
            capture: CaptureConfigFFI {
                backend: cap_backend.as_ptr(),
                device: cap_device.as_ptr(),
                width: params.width,
                height: params.height,
                fps: params.fps,
                pixel_format: params.capture_pixel_format,
                prefer_dmabuf: 0,
            },
            encoder: EncoderConfigFFI {
                backend: enc_backend.as_ptr(),
                codec: params.codec,
                width: params.width,
                height: params.height,
                fps: params.fps,
                bitrate: params.bitrate,
                profile: profile.as_ptr(),
                gop: params.gop,
                prefer_dmabuf: 0,
            },
            payload_type: params.payload_type,
            clock_rate: params.clock_rate,
        };

        cstrs.push(cap_backend);
        cstrs.push(cap_device);
        cstrs.push(enc_backend);
        cstrs.push(profile);

        (cfg, cstrs)
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn state(&self) -> StreamSourceState {
        *self.state.blocking_read()
    }

    pub fn subscribe_rtp(&self) -> broadcast::Receiver<MediaPacket> {
        self.rtp_tx.subscribe()
    }

    pub fn subscribe_state(&self) -> broadcast::Receiver<StateChangeEvent> {
        self.state_tx.subscribe()
    }

    pub async fn start(&mut self) -> Result<()> {
        if self.handle.is_some() {
            anyhow::bail!("Already started");
        }

        let (shutdown_tx, _shutdown_rx) = broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx);

        let (ffi_cfg, cstrs) = Self::build_ffi_config(&self.params);
        self._cstrs = Some(cstrs);

        let ctx = Box::new(CallbackCtx {
            rtp_tx: self.rtp_tx.clone(),
            payload_type: self.params.payload_type as u8,
            clock_rate: self.params.clock_rate,
            start_instant: Instant::now(),
            parser: Mutex::new(AnnexBParser::new()),
            packetizer: Mutex::new(H264Packetizer::new(
                1400,
                self.params.payload_type as u8,
                self.params.clock_rate,
            )),
            last_rtp_ts: Mutex::new(None),
            #[cfg(feature = "source")]
            dynamic_profile: self.dynamic_profile.clone(),
        });
        let user_data = Box::into_raw(ctx) as *mut std::ffi::c_void;

        let hooks = SourcePipelineHooksFFI {
            on_packet: Some(on_encoded_packet),
            user_data,
        };

        let mut errbuf: [c_char; ERR_BUF_LEN] = [0; ERR_BUF_LEN];

        let raw_handle = unsafe {
            source_pipeline_create(
                &ffi_cfg as *const _,
                &hooks as *const _,
                errbuf.as_mut_ptr(),
                ERR_BUF_LEN,
            )
        };

        self._cstrs = None;

        if raw_handle.is_null() {
            unsafe { drop(Box::from_raw(user_data as *mut CallbackCtx)); }
            let err_str = unsafe { std::ffi::CStr::from_ptr(errbuf.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            anyhow::bail!("source_pipeline_create failed: {}", err_str);
        }

        self.handle.set(raw_handle);
        self.callback_ctx = Some(user_data as *mut CallbackCtx);

        if !unsafe { source_pipeline_start(raw_handle) } {
            self.cleanup_pipeline();
            anyhow::bail!("source_pipeline_start failed");
        }

        self.set_state(StreamSourceState::Connected, None).await;
        Ok(())
    }

    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.cleanup_pipeline();
        self.set_state(StreamSourceState::Disconnected, None).await;
    }

    /// 1. Take the raw handle out of the shared Arc → RTCP tasks see None.
    /// 2. Stop + free SourcePipeline (no more callbacks).
    /// 3. Free CallbackCtx.
    fn cleanup_pipeline(&mut self) {
        let raw_handle = self.handle.take();

        if let Some(h) = raw_handle {
            unsafe {
                source_pipeline_stop(h);
                source_pipeline_free(h);
            }
        }

        if let Some(ctx_ptr) = self.callback_ctx.take() {
            unsafe { drop(Box::from_raw(ctx_ptr as *mut CallbackCtx)); }
        }
    }

    #[cfg(feature = "source")]
    pub async fn get_video_codec(
        &self,
    ) -> Option<webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters> {
        use webrtc::rtp_transceiver::RTCPFeedback;
        use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters};

        let mime_type = format!("video/{}", self.params.codec_name.to_uppercase());
        let profile = self
            .dynamic_profile
            .read()
            .await
            .clone()
            .unwrap_or_else(|| self.params.default_profile.clone());

        Some(RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type,
                clock_rate: self.params.clock_rate,
                channels: 0,
                sdp_fmtp_line: format!(
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={}",
                    profile
                ),
                rtcp_feedback: vec![
                    RTCPFeedback { typ: "goog-remb".into(), parameter: "".into() },
                    RTCPFeedback { typ: "nack".into(), parameter: "".into() },
                    RTCPFeedback { typ: "nack".into(), parameter: "pli".into() },
                ],
            },
            payload_type: self.params.payload_type as u8,
            stats_id: String::new(),
        })
    }

    #[cfg(feature = "source")]
    pub async fn get_audio_codec(
        &self,
    ) -> Option<webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters> {
        None
    }

    #[cfg(feature = "source")]
    pub async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        let handle = self.handle.clone();
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(async move {
            while let Some(data) = rx.recv().await {
                if let Ok(packets) = webrtc::rtcp::packet::unmarshal(&mut &data[..]) {
                    for packet in packets {
                        if packet
                            .as_any()
                            .downcast_ref::<webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>()
                            .is_some()
                        {
                            handle.request_keyframe();
                        }
                    }
                }
            }
        });
        Some(tx)
    }
}

impl Drop for NativeEncodedSource {
    fn drop(&mut self) {
        self.cleanup_pipeline();
    }
}
