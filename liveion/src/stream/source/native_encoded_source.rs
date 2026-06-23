//! NativeEncodedSource — consumes `livesrc::NativePipeline` and bridges
//! encoded packets into the liveion RTP / WHEP infrastructure.
//!
//! Data flow:
//!   C++ SourcePipeline → livesrc FFI → EncodedPacket channel →
//!   codec-specific RTP packetize (webrtc crate) → RTP broadcast
//!
//! livesrc handles all C++ FFI — this module only sees `EncodedPacket`
//! through an mpsc channel.

use super::{
    MediaPacket, StateChangeEvent, StreamSourceState, source_config::VideoCodec as SourceVideoCodec,
};
use anyhow::Result;
use rtc_rtp::codec::av1::Av1Payloader;
use rtc_rtp::codec::h264::H264Payloader;
use rtc_rtp::codec::h265::HevcPayloader;
use rtc_rtp::codec::vp8::Vp8Payloader;
use rtc_rtp::codec::vp9::Vp9Payloader;
use rtc_rtp::packetizer::{Packetizer as _, Payloader, new_packetizer};
use rtc_rtp::sequence::new_random_sequencer;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{RwLock, broadcast, mpsc};

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
    packetize_handle: Option<tokio::task::JoinHandle<()>>,
    #[cfg(feature = "source")]
    dynamic_profile: Arc<RwLock<Option<String>>>,
}

// SAFETY: `NativeEncodedSource` is composed entirely of `Send + Sync` types:
// `String`, `NativeSourceParams`, `Arc<RwLock<…>>`, broadcast/mpsc channels,
// and `livesrc::NativePipeline` / `livesrc::KeyframeHandle` (which already
// implement `Send`/`Sync`). No raw pointers or thread-local state are held.
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
            packetize_handle: None,
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
        let codec = self.params.codec;
        let fallback_delta = clock_rate / self.params.fps.max(1);
        #[cfg(feature = "source")]
        let dynamic_profile = self.dynamic_profile.clone();

        let handle = tokio::spawn(async move {
            let codec_typed = SourceVideoCodec::try_from(codec).unwrap_or(SourceVideoCodec::H264);
            let payloader: Box<dyn Payloader> = match codec_typed {
                SourceVideoCodec::H265 => Box::new(HevcPayloader::default()),
                SourceVideoCodec::Av1 => Box::new(Av1Payloader::default()),
                SourceVideoCodec::Vp8 => Box::new(Vp8Payloader::default()),
                SourceVideoCodec::Vp9 => Box::new(Vp9Payloader::default()),
                SourceVideoCodec::H264 => Box::new(H264Payloader::default()),
            };
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
                        if n.is_multiple_of(60) {
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
                            // Dynamic profile-level-id detection is H.264-specific.
                            // Use a read-lock first: the profile rarely changes, so
                            // the fast path avoids contending with get_video_codec().
                            if codec_typed == SourceVideoCodec::H264 {
                                let needs_update = {
                                    let guard = dynamic_profile.read().await;
                                    guard.as_ref().is_none()
                                };
                                if needs_update
                                    && let Some(profile) = scan_sps_profile(&pkt.data)
                                {
                                    let mut guard = dynamic_profile.write().await;
                                    if guard.as_ref() != Some(&profile) {
                                        *guard = Some(profile);
                                    }
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
        self.packetize_handle = Some(handle);

        self.set_state(StreamSourceState::Connected, None).await;
        Ok(())
    }

    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        // Wait for the packetize task to finish so it does not touch
        // rtp_tx / rx after we drop the pipeline.
        if let Some(handle) = self.packetize_handle.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }
        self.pipeline = None;
        self.keyframe_handle = None;
        self.set_state(StreamSourceState::Disconnected, None).await;
    }

    #[cfg(feature = "source")]
    pub async fn get_video_codec(
        &self,
    ) -> Option<rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters> {
        use rtc::rtp_transceiver::rtp_sender::{RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters};

        let mime_type = format!("video/{}", self.params.codec_name.to_uppercase());

        // Build codec-specific SDP fmtp line.
        let codec = SourceVideoCodec::try_from(self.params.codec).unwrap_or(SourceVideoCodec::H264);
        let sdp_fmtp_line = match codec {
            SourceVideoCodec::H264 => {
                // H.264: use dynamically detected profile-level-id if available.
                let profile = self
                    .dynamic_profile
                    .read()
                    .await
                    .clone()
                    .unwrap_or_else(|| self.params.default_profile.clone());
                format!(
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={}",
                    profile
                )
            }
            SourceVideoCodec::H265 => {
                // H.265: default to Main profile / level 3.1.
                // A future improvement can parse VPS/SPS for dynamic profile/level.
                "profile-id=1;tier-flag=0;level-id=93".to_string()
            }
            // AV1, VP8, VP9: no fmtp required for basic negotiation.
            _ => String::new(),
        };

        Some(RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type,
                clock_rate: self.params.clock_rate,
                channels: 0,
                sdp_fmtp_line,
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
        })
    }

    #[cfg(feature = "source")]
    pub async fn get_audio_codec(
        &self,
    ) -> Option<rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters> {
        None
    }

    #[cfg(feature = "source")]
    pub async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        let kh = self.keyframe_handle.clone()?;
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(async move {
            while let Some(data) = rx.recv().await {
                if let Ok(packets) = rtc_rtcp::packet::unmarshal(&mut &data[..]) {
                    for packet in packets {
                        if packet
                            .as_any()
                            .downcast_ref::<rtc_rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>()
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
// Minimal H.264 Annex-B SPS scanner — only used for H.264 dynamic
// profile-level-id detection in get_video_codec().
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
        if nal_start < data.len() {
            let nal_type = data[nal_start] & 0x1F;
            if nal_type == 7 {
                // Copy the SPS NAL payload into a small buffer while removing
                // H.264 emulation prevention bytes (0x00 0x00 0x03). The
                // profile-level-id follows the NAL header byte, so we need the
                // next three de-escaped bytes.
                let mut buf = [0u8; 4];
                let mut src = nal_start;
                let mut dst = 0;
                while src < data.len() && dst < 4 {
                    if src + 2 < data.len()
                        && data[src] == 0
                        && data[src + 1] == 0
                        && data[src + 2] == 3
                    {
                        buf[dst] = 0;
                        dst += 1;
                        if dst < 4 {
                            buf[dst] = 0;
                            dst += 1;
                        }
                        src += 3;
                    } else {
                        buf[dst] = data[src];
                        dst += 1;
                        src += 1;
                    }
                }
                if dst == 4 {
                    return Some(format!("{:02x}{:02x}{:02x}", buf[1], buf[2], buf[3]));
                }
            }
        }
        pos += start_len;
    }
    None
}
