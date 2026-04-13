//! Libcamera-Bridge Source.
//!
//! Launches the `libcamera-bridge` binary, captures its H.264 stdout,
//! and supports sending keyframe requests via stdin (`k\n`).

use super::h264_utils::{AnnexBParser, H264Packetizer, NalType, parse_profile_level_id};
use super::stream_config_v2::ExecUrlParams;
use super::{InternalSourceConfig, MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};
use anyhow::Result;
use async_trait::async_trait;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::{debug, error, info, trace, warn};

#[cfg(feature = "source")]
use webrtc::rtp_transceiver::RTCPFeedback;
#[cfg(feature = "source")]
use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters};

const CHANNEL_VIDEO_RTP: u8 = 0;

pub struct LibcameraSource {
    config: InternalSourceConfig,
    params: ExecUrlParams,
    state: Arc<RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    task_handles: Vec<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<broadcast::Sender<()>>,
    #[cfg(feature = "source")]
    dynamic_profile: Arc<RwLock<Option<String>>>,
    stdin_tx: mpsc::UnboundedSender<String>,
    stdin_rx: Option<mpsc::UnboundedReceiver<String>>,
}

impl LibcameraSource {
    /// Create a new LibcameraSource from a URL.
    pub fn from_url(url: &str, config: &crate::config::SourceConfig) -> Result<Self> {
        // We use the specialized libcamera parser
        let mut params = super::stream_config_v2::parse_libcamera_url(url)?;
        
        // If the path is empty or just "libcamera-bridge", try to find it in the expected path
        if params.executable.is_empty() || params.executable == "libcamera-bridge" {
            params.executable = "/home/hao/livesrc/libcamera-bridge/build/libcamera-bridge".to_string();
        }

        let internal_config = InternalSourceConfig::from_config(config);

        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);
        let (stdin_tx, stdin_rx) = mpsc::unbounded_channel();

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
            stdin_tx,
            stdin_rx: Some(stdin_rx),
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

    /// Process stdout loop
    async fn stdout_task(
        stream_id: String,
        mut stdout: tokio::process::ChildStdout,
        params: ExecUrlParams,
        rtp_tx: broadcast::Sender<MediaPacket>,
        state: Arc<RwLock<StreamSourceState>>,
        state_tx: broadcast::Sender<StateChangeEvent>,
        mut shutdown_rx: broadcast::Receiver<()>,
        #[cfg(feature = "source")] dynamic_profile: Arc<RwLock<Option<String>>>,
    ) {
        info!("[{}] libcamera stdout task started", stream_id);

        let mut annexb = AnnexBParser::new();
        let mut packetizer = H264Packetizer::new(1400, params.payload_type, params.clock_rate);
        
        let mut buf = vec![0u8; 32 * 1024];
        let mut _frame_count: u64 = 0;

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => break,
                res = stdout.read(&mut buf) => {
                    match res {
                        Ok(0) => {
                            warn!("[{}] libcamera stdout EOF", stream_id);
                            let mut s = state.write().await;
                            *s = StreamSourceState::Disconnected;
                            let _ = state_tx.send(StateChangeEvent {
                                old_state: StreamSourceState::Connected,
                                new_state: StreamSourceState::Disconnected,
                                error: Some("Process exited".to_string()),
                            });
                            break;
                        }
                        Ok(n) => {
                            annexb.push(&buf[..n]);
                            let nals = annexb.extract_nals();
                            
                            for nal in nals {
                                #[cfg(feature = "source")]
                                if nal.nal_type == NalType::Sps {
                                    if let Some(profile) = parse_profile_level_id(&nal.data) {
                                        let mut dp = dynamic_profile.write().await;
                                        if dp.as_ref() != Some(&profile) {
                                            info!("[{}] H.264 profile: {}", stream_id, profile);
                                            *dp = Some(profile);
                                        }
                                    }
                                }

                                 let rtp_packets = packetizer.packetize(&nal);
                                for packet in rtp_packets {
                                    let _ = rtp_tx.send(MediaPacket::Rtp {
                                        channel: CHANNEL_VIDEO_RTP,
                                        data: packet.to_bytes(),
                                    });
                                }

                                if nal.nal_type.is_vcl() {
                                    _frame_count += 1;
                                    packetizer.advance_timestamp(params.clock_rate / 30); // Assume 30 FPS

                                    // Check if we should generate SDP file
                                    if let (Some(sps), Some(_pps), Some(sprop)) = (packetizer.cached_sps(), packetizer.cached_pps(), packetizer.get_sprop_parameter_sets()) {
                                        let profile = parse_profile_level_id(&sps).unwrap_or_else(|| "42001f".into());
                                        let sdp_content = format!(
                                            "v=0\no=- 0 0 IN IP4 127.0.0.1\ns=Liveion-Libcamera\nc=IN IP4 127.0.0.1\nt=0 0\nm=video 5002 RTP/AVP {}\na=rtpmap:{} H264/90000\na=fmtp:{} level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={};sprop-parameter-sets={}\n",
                                            params.payload_type, params.payload_type, params.payload_type, profile, sprop
                                        );
                                        
                                        let sdp_path = format!("conf/{}.sdp", stream_id);
                                        if !std::path::Path::new(&sdp_path).exists() {
                                            if let Err(e) = std::fs::write(&sdp_path, sdp_content) {
                                                error!("[{}] Failed to write SDP file: {}", stream_id, e);
                                            } else {
                                                info!("[{}] Automatically generated SDP file: {}", stream_id, sdp_path);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!("[{}] libcamera stdout error: {}", stream_id, e);
                            break;
                        }
                    }
                }
            }
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
        if !self.task_handles.is_empty() { anyhow::bail!("Already started"); }

        let (shutdown_tx, _) = broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx.clone());

        info!("[{}] Starting libcamera-bridge: {} args: {:?}", self.config.stream_id, self.params.executable, self.params.args);

        let mut child = Command::new(&self.params.executable)
            .args(&self.params.args)
            .stdout(Stdio::piped())
            .stdin(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let stdout = child.stdout.take().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdin_rx = self.stdin_rx.take().ok_or_else(|| anyhow::anyhow!("Stdin receiver already taken"))?;

        // Process management task
        let stream_id = self.config.stream_id.clone();
        let state = self.state.clone();
        let state_tx = self.state_tx.clone();
        let mut shutdown_rx = shutdown_tx.subscribe();
        
        self.task_handles.push(tokio::spawn(async move {
            tokio::select! {
                status = child.wait() => {
                    info!("[{}] Process exited: {:?}", stream_id, status);
                    let mut s = state.write().await;
                    *s = StreamSourceState::Disconnected;
                    let _ = state_tx.send(StateChangeEvent {
                        old_state: StreamSourceState::Connected,
                        new_state: StreamSourceState::Disconnected,
                        error: Some("Terminated".into()),
                    });
                }
                _ = shutdown_rx.recv() => {
                    let _ = child.kill().await;
                }
            }
        }));

        // Stdin relay & Keyframe timer task
        let _stream_id = self.config.stream_id.clone();
        let mut shutdown_rx = shutdown_tx.subscribe();
        self.task_handles.push(tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => break,
                    _ = interval.tick() => {
                        let _ = stdin.write_all(b"k\n").await;
                        let _ = stdin.flush().await;
                    }
                    cmd = stdin_rx.recv() => {
                        if let Some(c) = cmd {
                            let _ = stdin.write_all(c.as_bytes()).await;
                            let _ = stdin.flush().await;
                        } else { break; }
                    }
                }
            }
        }));

        // Stdout task
        let stream_id = self.config.stream_id.clone();
        let params = self.params.clone();
        let rtp_tx = self.rtp_tx.clone();
        let state = self.state.clone();
        let state_tx = self.state_tx.clone();
        let shutdown_rx_stdout = shutdown_tx.subscribe();
        #[cfg(feature = "source")]
        let dynamic_profile = self.dynamic_profile.clone();

        self.task_handles.push(tokio::spawn(async move {
            {
                let mut s = state.write().await;
                *s = StreamSourceState::Connected;
                let _ = state_tx.send(StateChangeEvent {
                    old_state: StreamSourceState::Initializing,
                    new_state: StreamSourceState::Connected,
                    error: None,
                });
            }
            Self::stdout_task(stream_id, stdout, params, rtp_tx, state, state_tx, shutdown_rx_stdout, #[cfg(feature = "source")] dynamic_profile).await;
        }));

        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(()); }
        for h in self.task_handles.drain(..) { let _ = h.await; }
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
        let stdin_tx = self.stdin_tx.clone();
        let stream_id = self.config.stream_id.clone();
        tokio::spawn(async move {
            while let Some(data) = rx.recv().await {
                // Try to parse RTCP to identify PLI/FIR
                if let Ok(packets) = webrtc::rtcp::packet::unmarshal(&mut &data[..]) {
                    for packet in packets {
                        let is_pli = packet.as_any().downcast_ref::<webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>().is_some();
                        
                        if is_pli {
                            info!("[{}] Instant Keyframe Request (PLI received)", stream_id);
                            let _ = stdin_tx.send("k\n".into());
                        }
                    }
                } else {
                    // Fallback: any RTCP on this channel might be a hint
                    let _ = stdin_tx.send("k\n".into());
                }
            }
        });
        Some(tx)
    }
}
