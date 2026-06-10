//! NativeEncodedSource — consumes `livesrc::NativePipeline` and bridges
//! encoded packets into the liveion RTP / WHEP infrastructure.
//!
//! Data flow:
//!   C++ SourcePipeline → livesrc FFI → EncodedPacket channel →
//!   H264 RTP packetize (webrtc crate) → RTP broadcast
//!
//! livesrc handles all C++ FFI — this module only sees `EncodedPacket`
//! through an mpsc channel.

use super::{MediaPacket, StateChangeEvent, StreamSourceState};
use anyhow::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{RwLock, broadcast, mpsc};
use webrtc::rtp::codecs::h264::H264Payloader;
use webrtc::rtp::packetizer::{Packetizer as _, new_packetizer};
use webrtc::rtp::sequence::new_random_sequencer;

// ---------------------------------------------------------------------------
// NativeEncodedSource
// ---------------------------------------------------------------------------

pub struct NativeEncodedSource {
    stream_id: String,
    params: livesrc::NativeSourceParams,
    state: Arc<RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    shutdown_tx: Option<broadcast::Sender<()>>,
    pipeline: Option<livesrc::NativePipeline>,
    keyframe_handle: Option<livesrc::KeyframeHandle>,
    #[cfg(feature = "source")]
    dynamic_profile: Arc<RwLock<Option<String>>>,
}

unsafe impl Send for NativeEncodedSource {}
unsafe impl Sync for NativeEncodedSource {}

impl NativeEncodedSource {
    pub fn new(stream_id: String, params: livesrc::NativeSourceParams) -> Self {
        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Self {
            stream_id,
            params,
            state: Arc::new(RwLock::new(StreamSourceState::Initializing)),
            rtp_tx,
            state_tx,
            shutdown_tx: None,
            pipeline: None,
            keyframe_handle: None,
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
            tracing::info!(
                "[{}] state: {:?} -> {:?}{}",
                self.stream_id,
                old_state,
                new_state,
                error.map(|e| format!(" ({})", e)).unwrap_or_default()
            );
        }
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
        if self.pipeline.is_some() {
            anyhow::bail!("Already started");
        }

        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx);

        let mut pipeline = livesrc::NativePipeline::new(&self.params)?;
        let keyframe_handle = pipeline.keyframe_handle();
        let mut rx = pipeline.start()?;

        let rtp_tx = self.rtp_tx.clone();
        let payload_type = self.params.payload_type as u8;
        let clock_rate = self.params.clock_rate;
        let fallback_delta = clock_rate / self.params.fps.max(1);
        #[cfg(feature = "source")]
        let dynamic_profile = self.dynamic_profile.clone();

        tokio::spawn(async move {
            let payloader = Box::new(H264Payloader::default());
            let sequencer = Box::new(new_random_sequencer());
            let ssrc: u32 = rand::random();
            let mut packetizer =
                new_packetizer(1400, payload_type, ssrc, payloader, sequencer, clock_rate);

            // Track the previous RTP 90kHz timestamp so we can pass
            // *increments* to packetizer.packetize().  The webrtc
            // Packetizer maintains an internal timestamp base and
            // wrapping-adds the `samples` parameter after each call.
            let mut last_rtp_ts: Option<u32> = None;

            static DBG_COUNT: AtomicU64 = AtomicU64::new(0);

            loop {
                tokio::select! {
                    pkt = rx.recv() => {
                        let Some(pkt) = pkt else { break };

                        let n = DBG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                        if n % 60 == 0 {
                            tracing::trace!(
                                "[NativeEncodedSource] packet bytes={} count={}",
                                pkt.data.len(), n
                            );
                        }

                        // Convert PTS (microseconds) to RTP 90 kHz clock
                        let rtp_ts = if pkt.pts_us > 0 {
                            (pkt.pts_us * 9 / 100) as u32
                        } else {
                            0u32
                        };

                        // Delta across calls — monotonic, no backward steps.
                        // `fallback_delta` = clock_rate / fps covers
                        // timestamp regressions (e.g. encoder PTS reset).
                        let delta = match last_rtp_ts {
                            Some(prev) if rtp_ts > prev => rtp_ts - prev,
                            Some(_prev) => fallback_delta,
                            None => 0, // first frame: let Packetizer use
                                       // its internal base timestamp
                        };
                        last_rtp_ts = Some(rtp_ts);

                        #[cfg(feature = "source")]
                        {
                            if let Some(profile) = scan_sps_profile(&pkt.data) {
                                let mut guard = dynamic_profile.write().await;
                                if guard.as_ref() != Some(&profile) {
                                    *guard = Some(profile);
                                }
                            }
                        }

                        match packetizer.packetize(&pkt.data.into(), delta) {
                            Ok(packets) => {
                                for packet in packets {
                                    let _ = rtp_tx.send(MediaPacket::RtpPacket(
                                        std::sync::Arc::new(packet),
                                    ));
                                }
                            }
                            Err(e) => {
                                tracing::warn!("RTP packetize error: {}", e);
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        break;
                    }
                }
            }
        });

        self.pipeline = Some(pipeline);
        self.keyframe_handle = Some(keyframe_handle);

        self.set_state(StreamSourceState::Connected, None).await;
        Ok(())
    }

    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        self.pipeline = None;
        self.keyframe_handle = None;
        self.set_state(StreamSourceState::Disconnected, None).await;
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
                    RTCPFeedback {
                        typ: "goog-remb".into(),
                        parameter: "".into(),
                    },
                    RTCPFeedback {
                        typ: "nack".into(),
                        parameter: "".into(),
                    },
                    RTCPFeedback {
                        typ: "nack".into(),
                        parameter: "pli".into(),
                    },
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
        let kh = self.keyframe_handle.clone()?;
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
                            kh.request_keyframe();
                        }
                    }
                }
            }
        });
        Some(tx)
    }
}

// ---------------------------------------------------------------------------
// Minimal H.264 Annex-B SPS scanner — only what's needed for
// dynamic profile-level-id detection (used by get_video_codec).
// RTP packetization is handled by webrtc's H264Payloader + Packetizer.
// ---------------------------------------------------------------------------

fn scan_sps_profile(data: &[u8]) -> Option<String> {
    let mut pos = 0;
    while pos + 3 < data.len() {
        let start_len = if data[pos] == 0 && data[pos + 1] == 0 && data[pos + 2] == 1 {
            3
        } else if pos + 4 <= data.len()
            && data[pos] == 0
            && data[pos + 1] == 0
            && data[pos + 2] == 0
            && data[pos + 3] == 1
        {
            4
        } else {
            pos += 1;
            continue;
        };
        let nal_start = pos + start_len;
        if nal_start + 4 <= data.len() {
            let nal_type = data[nal_start] & 0x1F;
            if nal_type == 7 {
                return Some(format!(
                    "{:02x}{:02x}{:02x}",
                    data[nal_start + 1],
                    data[nal_start + 2],
                    data[nal_start + 3]
                ));
            }
        }
        pos += start_len;
    }
    None
}
