//! V4L2 Direct Capture Source (FFI Edition) - Robust Version.
//!
//! Direct integration with USB cameras via V4L2 + V4L2 M2M hardware encoder.
//! Features auto-reconnect and device discovery logic.

use super::h264_utils::{H264Packetizer, AnnexBParser, parse_profile_level_id};
use super::stream_config_v2::parse_v4l2_url;
use super::{InternalSourceConfig, MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::{info, warn, error};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::time::Instant;

#[cfg(feature = "source")]
use webrtc::rtp_transceiver::RTCPFeedback;
#[cfg(feature = "source")]
use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters};

const CHANNEL_VIDEO_RTP: u8 = 0;

// --- FFI Bindings ---
#[repr(C)]
struct V4L2BridgeContext { _private: [u8; 0] }

#[derive(Clone, Copy)]
pub struct V4L2BridgePtr(pub *mut V4L2BridgeContext);
unsafe impl Send for V4L2BridgePtr {}
unsafe impl Sync for V4L2BridgePtr {}

#[derive(Clone, Copy)]
struct SendPtr(pub *mut c_void);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

type V4L2NALCallbackFFI = unsafe extern "C" fn(data: *const u8, size: usize, is_keyframe: c_int, timestamp: u64, user_data: *mut c_void);

unsafe extern "C" {
    fn v4l2_bridge_init(device: *const c_char, width: c_int, height: c_int, fps: c_int, bitrate: c_int) -> *mut V4L2BridgeContext;
    fn v4l2_bridge_set_callback(ctx: *mut V4L2BridgeContext, callback: V4L2NALCallbackFFI, user_data: *mut c_void);
    fn v4l2_bridge_start(ctx: *mut V4L2BridgeContext) -> bool;
    fn v4l2_bridge_stop(ctx: *mut V4L2BridgeContext);
    fn v4l2_bridge_is_running(ctx: *mut V4L2BridgeContext) -> bool;
    fn v4l2_bridge_request_keyframe(ctx: *mut V4L2BridgeContext);
    fn v4l2_bridge_get_error(ctx: *mut V4L2BridgeContext) -> *const c_char;
    fn v4l2_bridge_free(ctx: *mut V4L2BridgeContext);
}

struct NALMessage {
    data: Vec<u8>,
    timestamp_us: u64,
}

enum BridgeCommand {
    RequestKeyframe,
}

pub struct V4L2Source {
    config: InternalSourceConfig,
    device: String,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u32,
    payload_type: u8,
    clock_rate: u32,
    profile: String,
    state: Arc<RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    task_handles: Vec<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<broadcast::Sender<()>>,
    cmd_tx: Option<mpsc::UnboundedSender<BridgeCommand>>,
    #[cfg(feature = "source")]
    dynamic_profile: Arc<RwLock<Option<String>>>,
}

unsafe impl Send for V4L2Source {}
unsafe impl Sync for V4L2Source {}

impl V4L2Source {
    pub fn from_url(url: &str, config: &crate::config::SourceConfig) -> Result<Self> {
        let params = parse_v4l2_url(url)?;
        let internal_config = InternalSourceConfig::from_config(config);
        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Ok(Self {
            config: internal_config,
            device: params.device,
            width: params.width,
            height: params.height,
            fps: params.fps,
            bitrate: params.bitrate,
            payload_type: params.payload_type,
            clock_rate: params.clock_rate,
            profile: params.profile,
            state: Arc::new(RwLock::new(StreamSourceState::Initializing)),
            rtp_tx,
            state_tx,
            task_handles: Vec::new(),
            shutdown_tx: None,
            cmd_tx: None,
            #[cfg(feature = "source")]
            dynamic_profile: Arc::new(RwLock::new(None)),
        })
    }

    async fn set_state(state_arc: &Arc<RwLock<StreamSourceState>>, state_tx: &broadcast::Sender<StateChangeEvent>, stream_id: &str, new_state: StreamSourceState, error: Option<String>) {
        let mut state = state_arc.write().await;
        let old_state = *state;
        if old_state != new_state {
            *state = new_state;
            let _ = state_tx.send(StateChangeEvent { old_state, new_state, error: error.clone() });
            info!("[{}] V4L2 state: {:?} -> {:?}{}", stream_id, old_state, new_state, error.map(|e| format!(" ({})", e)).unwrap_or_default());
        }
    }

    unsafe extern "C" fn nal_callback(data: *const u8, size: usize, _is_kf: c_int, ts: u64, user_data: *mut c_void) {
        if user_data.is_null() { return; }
        let tx = &*(user_data as *const mpsc::UnboundedSender<NALMessage>);
        let buf = std::slice::from_raw_parts(data, size);
        let _ = tx.send(NALMessage { data: buf.to_vec(), timestamp_us: ts });
    }

    #[cfg(feature = "source")]
    async fn build_video_codec_params(&self) -> RTCRtpCodecParameters {
        let profile = self.dynamic_profile.read().await.clone().unwrap_or_else(|| self.profile.clone());
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: "video/H264".to_string(),
                clock_rate: self.clock_rate,
                channels: 0,
                sdp_fmtp_line: format!("level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={}", profile),
                rtcp_feedback: vec![
                    RTCPFeedback { typ: "goog-remb".into(), parameter: "".into() },
                    RTCPFeedback { typ: "nack".into(), parameter: "".into() },
                    RTCPFeedback { typ: "nack".into(), parameter: "pli".into() },
                ],
            },
            payload_type: self.payload_type,
            stats_id: String::new(),
        }
    }
}

#[async_trait]
impl StreamSource for V4L2Source {
    fn stream_id(&self) -> &str { &self.config.stream_id }
    fn state(&self) -> StreamSourceState { *self.state.blocking_read() }

    async fn start(&mut self) -> Result<()> {
        if self.shutdown_tx.is_some() { anyhow::bail!("Already started"); }

        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<BridgeCommand>();
        
        self.shutdown_tx = Some(shutdown_tx);
        self.cmd_tx = Some(cmd_tx);

        let stream_id = self.config.stream_id.clone();
        let device_path = self.device.clone();
        let width = self.width;
        let height = self.height;
        let fps = self.fps;
        let bitrate = self.bitrate;
        let payload_type = self.payload_type;
        let clock_rate = self.clock_rate;
        let rtp_tx = self.rtp_tx.clone();
        let state_tx = self.state_tx.clone();
        let state_arc = self.state.clone();
        #[cfg(feature = "source")]
        let dynamic_profile = self.dynamic_profile.clone();

        self.task_handles.push(tokio::spawn(async move {
            let mut packetizer = H264Packetizer::new(1400, payload_type, clock_rate);
            let mut parser = AnnexBParser::new();
            let start_inst = Instant::now();
            let mut last_rtp_ts: u32 = 0;
            
            loop {
                info!("[{}] Attempting to open V4L2 device: {}", stream_id, device_path);
                Self::set_state(&state_arc, &state_tx, &stream_id, StreamSourceState::Initializing, None).await;

                let device_cstr = match CString::new(device_path.clone()) {
                    Ok(s) => s,
                    Err(_) => { error!("[{}] Invalid device path", stream_id); break; }
                };

                let ctx = unsafe {
                    let ptr = v4l2_bridge_init(device_cstr.as_ptr(), width as c_int, height as c_int, fps as c_int, bitrate as c_int);
                    if ptr.is_null() {
                        V4L2BridgePtr(std::ptr::null_mut())
                    } else {
                        V4L2BridgePtr(ptr)
                    }
                };

                if ctx.0.is_null() {
                    warn!("[{}] Failed to init bridge. Retrying in 5s...", stream_id);
                    tokio::select! {
                        _ = shutdown_rx.recv() => return,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                    }
                }

                let (nal_tx, mut nal_rx) = mpsc::unbounded_channel::<NALMessage>();
                let wrapper = SendPtr(Box::into_raw(Box::new(nal_tx)) as *mut c_void);

                unsafe {
                    v4l2_bridge_set_callback(ctx.0, Self::nal_callback, wrapper.0);
                    if !v4l2_bridge_start(ctx.0) {
                        warn!("[{}] Bridge failed to start. Cleaning up...", stream_id);
                        v4l2_bridge_free(ctx.0);
                        let _ = Box::from_raw(wrapper.0 as *mut mpsc::UnboundedSender<NALMessage>);
                        tokio::select! {
                             _ = shutdown_rx.recv() => return,
                             _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => continue,
                        }
                    }
                }

                Self::set_state(&state_arc, &state_tx, &stream_id, StreamSourceState::Connected, None).await;
                
                let mut health_check = tokio::time::interval(std::time::Duration::from_secs(1));
                let mut session_active = true;
                
                while session_active {
                    tokio::select! {
                        _ = shutdown_rx.recv() => {
                            session_active = false;
                        }
                        _ = health_check.tick() => {
                            if unsafe { !v4l2_bridge_is_running(ctx.0) } {
                                warn!("[{}] Bridge health check failed. Restarting session...", stream_id);
                                session_active = false;
                            }
                        }
                        cmd = cmd_rx.recv() => {
                            if let Some(BridgeCommand::RequestKeyframe) = cmd {
                                unsafe { v4l2_bridge_request_keyframe(ctx.0); }
                            }
                        }
                        msg = nal_rx.recv() => {
                            if let Some(msg) = msg {
                                let rtp_ts = if msg.timestamp_us > 0 {
                                    (msg.timestamp_us * 9 / 100) as u32
                                } else {
                                    (start_inst.elapsed().as_micros() * 9 / 100) as u32
                                };
                                let final_ts = if rtp_ts > last_rtp_ts { rtp_ts } else { last_rtp_ts.wrapping_add(3000) };
                                last_rtp_ts = final_ts;

                                parser.push(&msg.data);
                                let nals = parser.extract_nals();

                                for nal in nals {
                                    #[cfg(feature = "source")]
                                    if nal.nal_type == super::h264_utils::NalType::Sps {
                                        if let Some(profile) = parse_profile_level_id(&nal.data) {
                                            let mut dp = dynamic_profile.write().await;
                                            if dp.as_ref() != Some(&profile) {
                                                info!("[V4L2] SPS Profile refreshed: {}", profile);
                                                *dp = Some(profile);
                                            }
                                        }
                                    }
                                    packetizer.advance_timestamp(final_ts.wrapping_sub(packetizer.get_current_timestamp()));
                                    let rtp_packets = packetizer.packetize(&nal);
                                    for packet in rtp_packets {
                                        let _ = rtp_tx.send(MediaPacket::Rtp {
                                            channel: CHANNEL_VIDEO_RTP,
                                            data: packet.to_bytes(),
                                        });
                                    }
                                }
                            } else {
                                session_active = false;
                            }
                        }
                    }
                }

                unsafe {
                    v4l2_bridge_stop(ctx.0);
                    v4l2_bridge_free(ctx.0);
                    let _ = Box::from_raw(wrapper.0 as *mut mpsc::UnboundedSender<NALMessage>);
                }
                
                if shutdown_rx.try_recv().is_ok() { break; }
                Self::set_state(&state_arc, &state_tx, &stream_id, StreamSourceState::Error, Some("Session lost, retrying...".into())).await;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }));

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(()); }
        for h in self.task_handles.drain(..) { let _ = h.await; }
        self.cmd_tx = None;
        let old_state = *self.state.read().await;
        if old_state != StreamSourceState::Disconnected {
            let mut state = self.state.write().await;
            *state = StreamSourceState::Disconnected;
            let _ = self.state_tx.send(StateChangeEvent { old_state, new_state: StreamSourceState::Disconnected, error: None });
        }
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
        let cmd_tx = self.cmd_tx.as_ref()?.clone();
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(async move {
            while let Some(data) = rx.recv().await {
                if let Ok(packets) = webrtc::rtcp::packet::unmarshal(&mut &data[..]) {
                    for packet in packets {
                        if packet.as_any().downcast_ref::<webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>().is_some() {
                            let _ = cmd_tx.send(BridgeCommand::RequestKeyframe);
                        }
                    }
                }
            }
        });
        Some(tx)
    }
}
