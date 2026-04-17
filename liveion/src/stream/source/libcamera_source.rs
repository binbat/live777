//! Libcamera-Bridge Source (FFI Edition).
//!
//! Direct integration with libcamera via C FFI.
//! Eliminates process overhead and IPC latency by linking libcamera-bridge
//! as a static library.

use super::h264_utils::{H264Packetizer, AnnexBParser, parse_profile_level_id};
use super::stream_config_v2::LibcameraUrlParams;
use super::{InternalSourceConfig, MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::{info, warn};
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};
use std::time::Instant;

// SYNC imports with rtsp_source.rs patterns (V14.8-STABLE)
#[cfg(feature = "source")]
use webrtc::rtp_transceiver::RTCPFeedback;
#[cfg(feature = "source")]
use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters};

const CHANNEL_VIDEO_RTP: u8 = 0;

// --- FFI Bindings ---

#[repr(C)]
struct BridgeContext { _private: [u8; 0] }

/// Safety wrapper for the raw FFI pointer to allow Send/Sync crossing tokio task boundaries.
#[derive(Clone, Copy)]
pub struct LibcameraPtr(pub *mut BridgeContext);
unsafe impl Send for LibcameraPtr {}
unsafe impl Sync for LibcameraPtr {}

// V14.8: SYNC WITH bridge_ffi.cpp (5 arguments)
type NALCallbackFFI = unsafe extern "C" fn(data: *const u8, size: usize, is_keyframe: c_int, timestamp: u64, user_data: *mut c_void);

unsafe extern "C" {
    fn bridge_init(
        width: c_int, 
        height: c_int, 
        fps: c_int, 
        bitrate: c_int, 
        camera_id: c_int,
        rotation: c_int,
        hflip: c_int,
        vflip: c_int
    ) -> *mut BridgeContext;

    fn bridge_set_callback(ctx: *mut BridgeContext, callback: NALCallbackFFI, user_data: *mut c_void);
    fn bridge_start(ctx: *mut BridgeContext) -> bool;
    fn bridge_stop(ctx: *mut BridgeContext);
    fn bridge_request_keyframe(ctx: *mut BridgeContext);
    fn bridge_get_error(ctx: *mut BridgeContext) -> *const c_char;
    fn bridge_free(ctx: *mut BridgeContext);
}

/// Raw message from C++ callback
struct NALMessage {
    data: Vec<u8>,
    is_keyframe: bool,
    timestamp_us: u64,
}

pub struct LibcameraSource {
    config: InternalSourceConfig,
    params: LibcameraUrlParams,
    state: Arc<RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    task_handles: Vec<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<broadcast::Sender<()>>,
    #[cfg(feature = "source")]
    dynamic_profile: Arc<RwLock<Option<String>>>,
    
    // FFI Bridge handle
    bridge_ctx: Option<LibcameraPtr>,
    // Holding the raw pointer to prevent Sender from being dropped prematurely
    _user_data_holder: Option<*mut c_void>,
}

unsafe impl Send for LibcameraSource {}
unsafe impl Sync for LibcameraSource {}

impl LibcameraSource {
    pub fn from_url(url: &str, config: &crate::config::SourceConfig) -> Result<Self> {
        let params = super::stream_config_v2::parse_libcamera_url(url)?;
        let internal_config = InternalSourceConfig::from_config(config);

        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Ok(Self {
            config: internal_config,
            params,
            state: Arc::new(RwLock::new(StreamSourceState::Initializing)),
            rtp_tx,
            state_tx,
            task_handles: Vec::new(),
            shutdown_tx: None,
            #[cfg(feature = "source")]
            dynamic_profile: Arc::new(RwLock::new(None)),
            bridge_ctx: None,
            _user_data_holder: None,
        })
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
                "[{}] Libcamera state changed: {:?} -> {:?}{}",
                self.config.stream_id,
                old_state,
                new_state,
                error.map(|e| format!(" ({})", e)).unwrap_or_default()
            );
        }
    }

    /// The C++ callback trampoline (V14.8-SYNC)
    unsafe extern "C" fn nal_callback(data: *const u8, size: usize, is_keyframe: c_int, timestamp: u64, user_data: *mut c_void) {
        if user_data.is_null() { return; }
        
        // V14.8: Safety - explicitly scoped unsafe operations
        unsafe {
            let tx = &*(user_data as *const mpsc::UnboundedSender<NALMessage>);
            let buf = std::slice::from_raw_parts(data, size);
            
            let msg = NALMessage {
                data: buf.to_vec(),
                is_keyframe: is_keyframe != 0,
                timestamp_us: timestamp,
            };
            
            let _ = tx.send(msg);
        }
    }

    #[cfg(feature = "source")]
    async fn build_video_codec_params(&self) -> RTCRtpCodecParameters {
        let mime_type = format!("video/{}", self.params.codec.to_uppercase());
        let profile = self.dynamic_profile.read().await.clone().unwrap_or_else(|| self.params.profile.clone());

        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type,
                clock_rate: self.params.clock_rate,
                channels: 0,
                sdp_fmtp_line: format!("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={}", profile),
                rtcp_feedback: vec![
                    RTCPFeedback { typ: "goog-remb".into(), parameter: "".into() },
                    RTCPFeedback { typ: "nack".into(), parameter: "".into() },
                    RTCPFeedback { typ: "nack".into(), parameter: "pli".into() },
                ],
            },
            payload_type: self.params.payload_type,
            stats_id: String::new(),
        }
    }
}

#[async_trait]
impl StreamSource for LibcameraSource {
    fn stream_id(&self) -> &str { &self.config.stream_id }
    fn state(&self) -> StreamSourceState { *self.state.blocking_read() }

    async fn start(&mut self) -> Result<()> {
        if self.bridge_ctx.is_some() { anyhow::bail!("Already started"); }

        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx);

        info!("[{}] Starting libcamera-bridge (V14.8-SYNC)", self.config.stream_id);

        let ctx_raw = unsafe {
            bridge_init(self.params.width as c_int, self.params.height as c_int, self.params.fps as c_int, self.params.bitrate as c_int, self.params.camera_id as c_int, self.params.rotation as c_int, if self.params.hflip { 1 } else { 0 }, if self.params.vflip { 1 } else { 0 })
        };
        if ctx_raw.is_null() { anyhow::bail!("Failed to initialize libcamera bridge"); }
        
        let ctx = LibcameraPtr(ctx_raw);
        self.bridge_ctx = Some(ctx);

        let (nal_tx, mut nal_rx) = mpsc::unbounded_channel::<NALMessage>();
        let nal_tx_box = Box::new(nal_tx);
        let user_data_raw = Box::into_raw(nal_tx_box) as *mut c_void;
        self._user_data_holder = Some(user_data_raw);

        unsafe { bridge_set_callback(ctx.0, Self::nal_callback, user_data_raw); }

        let _stream_id = self.config.stream_id.clone();
        let rtp_tx = self.rtp_tx.clone();
        let params = self.params.clone();
        #[cfg(feature = "source")]
        let dynamic_profile = self.dynamic_profile.clone();

        self.task_handles.push(tokio::spawn(async move {
            let mut packetizer = H264Packetizer::new(1400, params.payload_type, params.clock_rate);
            let mut parser = AnnexBParser::new();
            let start_inst = Instant::now();
            let mut last_rtp_ts: u32 = 0;

            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => break,
                    msg = nal_rx.recv() => {
                        if let Some(msg) = msg {
                            // 1. Sync Timestamp (FFI us -> RTP 90kHz)
                            let rtp_ts = if msg.timestamp_us > 0 {
                                (msg.timestamp_us * 9 / 100) as u32
                            } else {
                                (start_inst.elapsed().as_micros() * 9 / 100) as u32
                            };
                            
                            // Prevent time going backwards in JitterBuffer
                            let final_ts = if rtp_ts > last_rtp_ts { rtp_ts } else { last_rtp_ts.wrapping_add(3000) };
                            last_rtp_ts = final_ts;

                            // 2. Parse AnnexB (Handle multiple NALs in one buffer)
                            parser.push(&msg.data);
                            let nals = parser.extract_nals();
                            
                            for nal in nals {
                                // 3. Profile detection (SPS)
                                #[cfg(feature = "source")]
                                if nal.nal_type == super::h264_utils::NalType::Sps {
                                    if let Some(profile) = parse_profile_level_id(&nal.data) {
                                        let mut dp = dynamic_profile.write().await;
                                        if dp.as_ref() != Some(&profile) {
                                            info!("[V14.8] SPS Profile: {}", profile);
                                            *dp = Some(profile);
                                        }
                                    }
                                }

                                // 4. Packetize with real timestamp
                                packetizer.advance_timestamp(final_ts.wrapping_sub(packetizer.get_current_timestamp()));
                                
                                let rtp_packets = packetizer.packetize(&nal);
                                for packet in rtp_packets {
                                    let _ = rtp_tx.send(MediaPacket::Rtp {
                                        channel: CHANNEL_VIDEO_RTP,
                                        data: packet.to_bytes(),
                                    });
                                }
                            }
                        } else { break; }
                    }
                }
            }
        }));

        if unsafe { !bridge_start(ctx.0) } { 
            let error_msg = unsafe {
                let err = bridge_get_error(ctx.0);
                if err.is_null() { "Unknown err".to_string() }
                else { CStr::from_ptr(err).to_string_lossy().into_owned() }
            };
            self.stop().await?; 
            anyhow::bail!("Bridge failed: {}", error_msg); 
        }
        self.set_state(StreamSourceState::Connected, None).await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(()); }
        for h in self.task_handles.drain(..) { let _ = h.await; }
        if let Some(ctx) = self.bridge_ctx.take() { unsafe { bridge_stop(ctx.0); bridge_free(ctx.0); } }
        if let Some(ptr) = self._user_data_holder.take() { unsafe { let _ = Box::from_raw(ptr as *mut mpsc::UnboundedSender<NALMessage>); } }
        self.set_state(StreamSourceState::Disconnected, None).await;
        Ok(())
    }

    fn subscribe_rtp(&self) -> broadcast::Receiver<MediaPacket> { self.rtp_tx.subscribe() }
    fn subscribe_state(&self) -> broadcast::Receiver<StateChangeEvent> { self.state_tx.subscribe() }

    #[cfg(feature = "source")]
    async fn get_video_codec(&self) -> Option<RTCRtpCodecParameters> { Some(self.build_video_codec_params().await) }
    #[cfg(feature = "source")]
    async fn get_audio_codec(&self) -> Option<RTCRtpCodecParameters> { None }

    #[cfg(feature = "source")]
    async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let bridge_ctx = self.bridge_ctx; 
        tokio::spawn(async move {
            while let Some(data) = rx.recv().await {
                if let Ok(packets) = webrtc::rtcp::packet::unmarshal(&mut &data[..]) {
                    for packet in packets {
                        if packet.as_any().downcast_ref::<webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>().is_some() {
                            if let Some(ctx) = bridge_ctx { unsafe { bridge_request_keyframe(ctx.0) }; }
                        }
                    }
                }
            }
        });
        Some(tx)
    }
}

impl Drop for LibcameraSource {
    fn drop(&mut self) {
        if let Some(ctx) = self.bridge_ctx.take() { unsafe { bridge_free(ctx.0) }; }
        if let Some(ptr) = self._user_data_holder.take() { unsafe { let _ = Box::from_raw(ptr as *mut mpsc::UnboundedSender<NALMessage>); } }
    }
}
