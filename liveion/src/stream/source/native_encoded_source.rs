//! NativeEncodedSource — consumes `livehal::NativePipeline` and bridges
//! encoded packets into the liveion RTP / WHEP infrastructure.
//!
//! Data flow:
//!   C++ SourcePipeline → livehal FFI → EncodedPacket channel →
//!   codec-specific RTP packetize (webrtc crate) → RTP broadcast
//!
//! A source runs a video leg (the C++ pipeline above), an audio leg
//! (`livehal::AudioPipeline` — pure-Rust capture → Opus encode), or both;
//! the audio leg packetizes into `MediaPacket::AudioRtpPacket`.
//!
//! livehal handles all C++ FFI — this module only sees `EncodedPacket`
//! through an mpsc channel.

use super::{
    MediaPacket, StateChangeEvent, StreamSourceState, source_config::VideoCodec as SourceVideoCodec,
};
use anyhow::Result;
use rtc_rtp::codec::av1::Av1Payloader;
use rtc_rtp::codec::h264::H264Payloader;
use rtc_rtp::codec::h265::HevcPayloader;
#[cfg(feature = "encoder-opus")]
use rtc_rtp::codec::opus::OpusPayloader;
use rtc_rtp::codec::vp8::Vp8Payloader;
use rtc_rtp::codec::vp9::Vp9Payloader;
use rtc_rtp::packetizer::{Packetizer as _, Payloader, new_packetizer};
use rtc_rtp::sequence::new_random_sequencer;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{RwLock, broadcast, mpsc};

// ---------------------------------------------------------------------------
// Capture-to-wire latency sampler — the Rust end of the P0 pipeline
// instrumentation; mirrors the C++ LatencyStats in
// livehal/native-pipeline/include/latency_stats.h ("wire" stage).
// ---------------------------------------------------------------------------

struct WireLatencyStats {
    window_start_us: u64,
    count: u64,
    sum_us: i64,
    max_us: i64,
}

impl WireLatencyStats {
    fn new() -> Self {
        Self {
            window_start_us: 0,
            count: 0,
            sum_us: 0,
            max_us: 0,
        }
    }

    fn sample(&mut self, pts_us: u64, now_us: u64, stream_id: &str) {
        if pts_us == 0 {
            return; // no usable origin timestamp
        }
        if self.window_start_us == 0 {
            self.window_start_us = now_us;
        }
        let delta = now_us as i64 - pts_us as i64;
        self.sum_us += delta;
        self.max_us = self.max_us.max(delta);
        self.count += 1;
        if now_us - self.window_start_us >= 1_000_000 {
            tracing::info!(
                "[{}] [latency] wire: n={} avg={:.2}ms max={:.2}ms",
                stream_id,
                self.count,
                self.sum_us as f64 / 1000.0 / self.count as f64,
                self.max_us as f64 / 1000.0
            );
            self.window_start_us = now_us;
            self.count = 0;
            self.sum_us = 0;
            self.max_us = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// NativeEncodedSource
// ---------------------------------------------------------------------------

pub struct NativeEncodedSource {
    stream_id: String,
    /// Video leg params; `None` on an audio-only source.
    params: Option<livehal::NativeSourceParams>,
    /// Audio leg params; `None` on a video-only source.
    #[cfg(feature = "encoder-opus")]
    audio_params: Option<livehal::AudioSourceParams>,
    adaptive: Option<super::adaptive_bitrate::AdaptiveBitrateConfig>,
    tiers: Vec<super::tier::QualityTier>,
    /// The tier last applied through the admin API (`None` = the
    /// configured base profile).
    active_tier: Option<String>,
    state: Arc<std::sync::RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    shutdown_tx: Option<broadcast::Sender<()>>,
    pipeline: Option<livehal::NativePipeline>,
    #[cfg(feature = "encoder-opus")]
    audio_pipeline: Option<livehal::AudioPipeline>,
    keyframe_handle: Option<livehal::KeyframeHandle>,
    bitrate_handle: Option<livehal::BitrateHandle>,
    #[cfg(feature = "encoder-opus")]
    audio_bitrate_handle: Option<livehal::AudioBitrateHandle>,
    packetize_handle: Option<tokio::task::JoinHandle<()>>,
    #[cfg(feature = "encoder-opus")]
    audio_packetize_handle: Option<tokio::task::JoinHandle<()>>,
    rtcp_sender: Option<mpsc::UnboundedSender<Vec<u8>>>,
    rtcp_handle: Option<tokio::task::JoinHandle<()>>,
    #[cfg(feature = "source")]
    dynamic_profile: Arc<RwLock<Option<String>>>,
}

// `NativeEncodedSource` is `Send + Sync` because all its fields are
// `Send + Sync` (strings, params, channels, Arc<RwLock>, and the livehal
// pipeline/handle types).

impl NativeEncodedSource {
    pub fn new(
        stream_id: String,
        params: Option<livehal::NativeSourceParams>,
        #[cfg(feature = "encoder-opus")] audio_params: Option<livehal::AudioSourceParams>,
        adaptive: Option<super::adaptive_bitrate::AdaptiveBitrateConfig>,
        tiers: Vec<super::tier::QualityTier>,
    ) -> Self {
        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Self {
            stream_id,
            params,
            #[cfg(feature = "encoder-opus")]
            audio_params,
            adaptive,
            tiers,
            active_tier: None,
            state: Arc::new(std::sync::RwLock::new(StreamSourceState::Initializing)),
            rtp_tx,
            state_tx,
            shutdown_tx: None,
            pipeline: None,
            #[cfg(feature = "encoder-opus")]
            audio_pipeline: None,
            keyframe_handle: None,
            bitrate_handle: None,
            #[cfg(feature = "encoder-opus")]
            audio_bitrate_handle: None,
            packetize_handle: None,
            #[cfg(feature = "encoder-opus")]
            audio_packetize_handle: None,
            rtcp_sender: None,
            rtcp_handle: None,
            #[cfg(feature = "source")]
            dynamic_profile: Arc::new(RwLock::new(None)),
        }
    }

    async fn set_state(&self, new_state: StreamSourceState, error: Option<String>) {
        let changed = {
            let mut state = self.state.write().unwrap();
            let old_state = *state;
            if old_state != new_state {
                *state = new_state;
                Some(old_state)
            } else {
                None
            }
        };

        if let Some(old_state) = changed {
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
        *self.state.read().unwrap()
    }

    pub fn subscribe_rtp(&self) -> broadcast::Receiver<MediaPacket> {
        self.rtp_tx.subscribe()
    }

    pub fn subscribe_state(&self) -> broadcast::Receiver<StateChangeEvent> {
        self.state_tx.subscribe()
    }

    pub async fn start(&mut self) -> Result<()> {
        if self.shutdown_tx.is_some() {
            anyhow::bail!("Already started");
        }

        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);
        self.shutdown_tx = Some(shutdown_tx.clone());

        // Video leg: capture+encode pipeline, RTCP keyframe relay and the
        // RTP packetize task.
        if let Some(params) = self.params.clone() {
            let mut pipeline = livehal::NativePipeline::new(&params)?;
            let keyframe_handle = pipeline.keyframe_handle();
            self.bitrate_handle = Some(pipeline.bitrate_handle());
            let mut rx = pipeline.start()?;

            // Start a single RTCP task for this source.  It will exit when the
            // sender is dropped in stop().
            let (rtcp_tx, mut rtcp_rx) = mpsc::unbounded_channel::<Vec<u8>>();
            let kh = keyframe_handle.clone();
            let rtcp_task = tokio::spawn(async move {
                while let Some(data) = rtcp_rx.recv().await {
                    if let Ok(packets) = rtc_rtcp::packet::unmarshal(&mut &data[..]) {
                        for packet in packets {
                            let any = packet.as_any();
                            if any
                                .downcast_ref::<rtc_rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>()
                                .is_some()
                                || any
                                    .downcast_ref::<rtc_rtcp::payload_feedbacks::full_intra_request::FullIntraRequest>()
                                    .is_some()
                            {
                                kh.request_keyframe();
                            }
                        }
                    }
                }
            });
            self.rtcp_sender = Some(rtcp_tx);
            self.rtcp_handle = Some(rtcp_task);

            let rtp_tx = self.rtp_tx.clone();
            let payload_type = params.payload_type as u8;
            let clock_rate = params.clock_rate;
            let codec = params.codec;
            let fallback_delta = clock_rate / params.fps.max(1);
            let stream_id = self.stream_id.clone();
            #[cfg(feature = "source")]
            let dynamic_profile = self.dynamic_profile.clone();

            let handle = tokio::spawn(async move {
                let codec_typed =
                    SourceVideoCodec::try_from(codec).unwrap_or(SourceVideoCodec::H264);
                let payloader: Box<dyn Payloader> = match codec_typed {
                    SourceVideoCodec::H265 => Box::new(HevcPayloader),
                    SourceVideoCodec::Av1 => Box::new(Av1Payloader::default()),
                    SourceVideoCodec::Vp8 => Box::new(Vp8Payloader::default()),
                    SourceVideoCodec::Vp9 => Box::new(Vp9Payloader::default()),
                    SourceVideoCodec::H264 => Box::new(H264Payloader::default()),
                };
                let sequencer = Box::new(new_random_sequencer());
                let ssrc: u32 = rand::random();
                let mut packetizer = new_packetizer(
                    std::time::Instant::now(),
                    1400,
                    payload_type,
                    ssrc,
                    payloader,
                    sequencer,
                    clock_rate,
                );

                // Track the previous RTP 90kHz timestamp so we can pass
                // *increments* to packetizer.packetize().  The webrtc
                // Packetizer maintains an internal timestamp base and
                // wrapping-adds the `samples` parameter after each call.
                let mut last_rtp_ts: Option<u32> = None;
                let mut wire_stats = WireLatencyStats::new();

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

                            // Convert PTS (microseconds) to the RTP clock.
                            let rtp_ts = if pkt.pts_us > 0 {
                                pts_us_to_rtp_ts(pkt.pts_us, clock_rate)
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

                            match packetizer.packetize(std::time::Instant::now(), &pkt.data.into(), delta) {
                                Ok(packets) => {
                                    for packet in packets {
                                        let _ = rtp_tx.send(MediaPacket::RtpPacket(
                                            std::sync::Arc::new(packet),
                                        ));
                                    }
                                    wire_stats.sample(
                                        pkt.pts_us,
                                        livehal::monotonic_us(),
                                        &stream_id,
                                    );
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
        }

        // Audio leg: livehal's pure-Rust capture → Opus pipeline plus its
        // own packetize task.  No RTCP task — audio has no keyframes.
        #[cfg(feature = "encoder-opus")]
        if let Some(audio_params) = self.audio_params.clone()
            && let Err(e) = self.start_audio_leg(audio_params, &shutdown_tx)
        {
            // Roll back a started video leg so a half-started source never
            // leaks a running pipeline.
            tracing::error!(
                "[{}] audio leg failed to start ({}), stopping source",
                self.stream_id,
                e
            );
            self.stop().await;
            return Err(e);
        }

        self.set_state(StreamSourceState::Connected, None).await;
        Ok(())
    }

    /// Start the audio leg: create and start the livehal audio pipeline
    /// and spawn the Opus RTP packetize task draining it.
    #[cfg(feature = "encoder-opus")]
    fn start_audio_leg(
        &mut self,
        audio_params: livehal::AudioSourceParams,
        shutdown_tx: &broadcast::Sender<()>,
    ) -> Result<()> {
        let mut pipeline = livehal::AudioPipeline::new(&audio_params)?;
        self.audio_bitrate_handle = Some(pipeline.bitrate_handle());
        let mut rx = pipeline.start()?;

        let rtp_tx = self.rtp_tx.clone();
        let stream_id = self.stream_id.clone();
        let mut shutdown_rx = shutdown_tx.subscribe();

        let handle = tokio::spawn(async move {
            let payloader: Box<dyn Payloader> = Box::<OpusPayloader>::default();
            let sequencer = Box::new(new_random_sequencer());
            let ssrc: u32 = rand::random();
            let mut packetizer = new_packetizer(
                std::time::Instant::now(),
                1400,
                audio_params.payload_type as u8,
                ssrc,
                payloader,
                sequencer,
                audio_params.clock_rate,
            );

            // Samples per Opus frame at the 48 kHz RTP clock — the
            // fallback delta when a timestamp regresses.
            let fallback_delta = audio_params.frame_ms * 48;
            let mut last_rtp_ts: Option<u32> = None;
            let mut wire_stats = WireLatencyStats::new();

            loop {
                tokio::select! {
                    pkt = rx.recv() => {
                        let Some(pkt) = pkt else { break };

                        // Convert PTS (microseconds) to the RTP clock.
                        let rtp_ts = if pkt.pts_us > 0 {
                            pts_us_to_rtp_ts(pkt.pts_us, audio_params.clock_rate)
                        } else {
                            0u32
                        };

                        // Delta across calls — monotonic, no backward steps.
                        let delta = match last_rtp_ts {
                            Some(prev) if rtp_ts > prev => rtp_ts - prev,
                            Some(_prev) => fallback_delta,
                            None => 0, // first frame: let Packetizer use
                                       // its internal base timestamp
                        };
                        last_rtp_ts = Some(rtp_ts);

                        match packetizer.packetize(std::time::Instant::now(), &pkt.data.into(), delta) {
                            Ok(packets) => {
                                for packet in packets {
                                    let _ = rtp_tx.send(MediaPacket::AudioRtpPacket(
                                        std::sync::Arc::new(packet),
                                    ));
                                }
                                wire_stats.sample(
                                    pkt.pts_us,
                                    livehal::monotonic_us(),
                                    &stream_id,
                                );
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

        self.audio_pipeline = Some(pipeline);
        self.audio_packetize_handle = Some(handle);
        Ok(())
    }

    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        // Drop the RTCP sender so the RTCP task exits naturally.
        self.rtcp_sender = None;

        // Abort and wait for the worker tasks so they do not touch
        // rtp_tx / rx after we drop the pipeline.
        if let Some(handle) = self.packetize_handle.take() {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }
        #[cfg(feature = "encoder-opus")]
        if let Some(handle) = self.audio_packetize_handle.take() {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }
        if let Some(handle) = self.rtcp_handle.take() {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }

        self.pipeline = None;
        // Dropping the audio pipeline stops and joins its capture/encode
        // threads (AudioPipeline::drop calls stop()).
        #[cfg(feature = "encoder-opus")]
        {
            self.audio_pipeline = None;
        }
        self.keyframe_handle = None;
        self.set_state(StreamSourceState::Disconnected, None).await;
    }

    #[cfg(feature = "source")]
    pub async fn get_video_codec(
        &self,
    ) -> Option<rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters> {
        use rtc::rtp_transceiver::rtp_sender::{RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters};

        let params = self.params.as_ref()?;
        let mime_type = format!("video/{}", params.codec_name.to_uppercase());

        // Build codec-specific SDP fmtp line.
        let codec = SourceVideoCodec::try_from(params.codec).unwrap_or(SourceVideoCodec::H264);
        let sdp_fmtp_line = match codec {
            SourceVideoCodec::H264 => {
                // H.264: use dynamically detected profile-level-id if available.
                let profile = self
                    .dynamic_profile
                    .read()
                    .await
                    .clone()
                    .unwrap_or_else(|| params.default_profile.clone());
                format!(
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={}",
                    profile
                )
            }
            SourceVideoCodec::H265 => {
                // H.265: uses the profile_idc/level_idc/tier_flag resolved by
                // source_config.rs — the same values the C++ encoder receives.
                format!(
                    "profile-id={};tier-flag={};level-id={}",
                    params.profile_idc, params.tier_flag, params.level_idc,
                )
            }
            // AV1, VP8, VP9: no fmtp required for basic negotiation.
            _ => String::new(),
        };

        Some(RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type,
                clock_rate: params.clock_rate,
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
            payload_type: params.payload_type as u8,
        })
    }

    #[cfg(feature = "source")]
    pub async fn get_audio_codec(
        &self,
    ) -> Option<rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters> {
        #[cfg(feature = "encoder-opus")]
        if let Some(params) = &self.audio_params {
            use rtc::peer_connection::configuration::media_engine::MIME_TYPE_OPUS;
            use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters};

            // Fixed wire contract (RFC 7587), mirroring the Opus codec
            // literal in livetwo's whipsynth packetizer: Opus 48 kHz,
            // payload type 111, stereo SDP channel count.
            return Some(RTCRtpCodecParameters {
                rtp_codec: RTCRtpCodec {
                    mime_type: MIME_TYPE_OPUS.to_owned(),
                    clock_rate: params.clock_rate,
                    channels: 2,
                    sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                    rtcp_feedback: vec![],
                },
                payload_type: params.payload_type as u8,
            });
        }
        None
    }

    #[cfg(feature = "source")]
    pub async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        self.rtcp_sender.clone()
    }

    /// Adaptive-bitrate bounds, when enabled in the source config.
    pub fn adaptive_bitrate_config(
        &self,
    ) -> Option<super::adaptive_bitrate::AdaptiveBitrateConfig> {
        self.adaptive
    }

    /// The bitrate the pipeline is currently configured with (the boot
    /// `encoder.bitrate` until a tier moves it).  The video leg's bitrate
    /// when present, else the audio leg's (mirroring the tier rule).
    pub fn configured_bitrate(&self) -> u32 {
        if let Some(params) = &self.params {
            return params.bitrate;
        }
        #[cfg(feature = "encoder-opus")]
        if let Some(audio_params) = &self.audio_params {
            return audio_params.bitrate;
        }
        // Unreachable: validate() guarantees at least one complete leg.
        0
    }

    /// Named quality tiers from the source config.
    pub fn tiers(&self) -> Vec<super::tier::QualityTier> {
        self.tiers.clone()
    }

    /// The tier last applied through the admin API (`None` = base config).
    pub fn active_tier(&self) -> Option<String> {
        self.active_tier.clone()
    }

    pub fn set_active_tier(&mut self, name: Option<String>) {
        self.active_tier = name;
    }

    /// The state the pipeline is actually running with: the last applied
    /// tier's name plus the current params (post-rollback these are the
    /// restored previous params).  On an audio-only source the geometry
    /// fields are zeroed (there is no capture geometry) and the bitrate
    /// is the audio leg's.
    #[cfg(feature = "source")]
    pub fn active_tier_state(&self) -> super::tier::ActiveTierState {
        super::tier::ActiveTierState {
            name: self.active_tier.clone(),
            width: self.params.as_ref().map(|p| p.width).unwrap_or(0),
            height: self.params.as_ref().map(|p| p.height).unwrap_or(0),
            fps: self.params.as_ref().map(|p| p.fps).unwrap_or(0),
            bitrate: self.configured_bitrate(),
        }
    }

    /// Retune the running encoder (adaptive bitrate control, issue #409).
    /// False when the pipeline is not running or the backend cannot retune.
    /// With both legs present the video encoder wins, mirroring the tier
    /// rule; on an audio-only source the Opus encoder is retuned.
    pub fn set_bitrate(&self, bps: u32) -> bool {
        if self.params.is_some() {
            return self
                .bitrate_handle
                .as_ref()
                .is_some_and(|h| h.set_bitrate(bps));
        }
        #[cfg(feature = "encoder-opus")]
        return self
            .audio_bitrate_handle
            .as_ref()
            .is_some_and(|h| h.set_bitrate(bps));
        #[cfg(not(feature = "encoder-opus"))]
        return false;
    }

    /// Rebuild the capture+encoder pipeline with new params (quality-tier
    /// switch carrying resolution/framerate).  The RTP/state broadcast
    /// channels survive, so subscribers stay attached across the rebuild;
    /// they only observe a short frame gap and an in-band SPS/PPS change.
    /// On start failure the previous params are restored (best effort) so
    /// the stream is not left dead.
    ///
    /// Video-leg only: an audio-only source has no pipeline to rebuild
    /// (validated tiers never carry geometry there, so this is
    /// unreachable through the admin API).
    pub async fn reconfigure(&mut self, params: livehal::NativeSourceParams) -> Result<()> {
        if self.pipeline.is_none() {
            anyhow::bail!("source is not running");
        }
        let Some(old_params) = self.params.clone() else {
            anyhow::bail!("source is not running");
        };
        self.stop().await;
        self.params = Some(params);
        // The new stream's SPS may carry a different profile-level-id —
        // let it be re-derived instead of serving the stale value.
        #[cfg(feature = "source")]
        self.dynamic_profile.write().await.take();
        if let Err(e) = self.start().await {
            tracing::error!(
                "[{}] reconfigure failed ({}), rolling back to previous params",
                self.stream_id,
                e
            );
            self.params = Some(old_params);
            let _ = self.start().await;
            return Err(e);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Minimal H.264 Annex-B SPS scanner — only used for H.264 dynamic
// profile-level-id detection in get_video_codec().
// ---------------------------------------------------------------------------

/// Convert a microsecond PTS to an RTP timestamp at `clock_rate`.
/// Use u128 for the intermediate product so that very large PTS values
/// cannot overflow before we wrap to the 32-bit RTP timestamp.
/// (At the 90 kHz video clock this is exactly `pts_us * 9 / 100`.)
fn pts_us_to_rtp_ts(pts_us: u64, clock_rate: u32) -> u32 {
    ((pts_us as u128 * clock_rate as u128) / 1_000_000) as u32
}

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "encoder-opus"))]
mod tests {
    use super::*;

    fn sine_audio_params() -> livehal::AudioSourceParams {
        livehal::AudioSourceParams {
            capture_backend: "sine".into(),
            capture_device: "default".into(),
            sample_rate: 48_000,
            channels: 2,
            bitrate: 64_000,
            application: livehal::AudioApplication::Audio,
            frame_ms: 20,
            vbr: true,
            inband_fec: false,
            dtx: false,
            payload_type: 111,
            clock_rate: 48_000,
            complexity: 9,
        }
    }

    #[tokio::test]
    async fn audio_leg_emits_opus_rtp() {
        let mut source = NativeEncodedSource::new(
            "mic".to_string(),
            None,
            Some(sine_audio_params()),
            None,
            vec![],
        );
        let mut rx = source.subscribe_rtp();
        source.start().await.expect("start audio-only source");

        // The sine pipeline paces 20 ms frames in real time; the first
        // Opus RTP packet must arrive quickly with the fixed wire contract.
        let packet = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(MediaPacket::AudioRtpPacket(packet)) = rx.recv().await {
                    break packet;
                }
            }
        })
        .await
        .expect("audio RTP packet within 2s");
        assert_eq!(packet.header.payload_type, 111);
        assert!(!packet.payload.is_empty());

        source.stop().await;
        assert_eq!(source.state(), StreamSourceState::Disconnected);
    }

    #[tokio::test]
    async fn audio_only_codec_and_bitrate_surface() {
        let source = NativeEncodedSource::new(
            "mic".to_string(),
            None,
            Some(sine_audio_params()),
            None,
            vec![],
        );
        assert!(source.get_video_codec().await.is_none());
        let audio = source.get_audio_codec().await.expect("audio codec");
        assert_eq!(audio.rtp_codec.mime_type, "audio/opus");
        assert_eq!(audio.rtp_codec.clock_rate, 48_000);
        assert_eq!(audio.rtp_codec.channels, 2);
        assert_eq!(audio.rtp_codec.sdp_fmtp_line, "minptime=10;useinbandfec=1");
        assert!(audio.rtp_codec.rtcp_feedback.is_empty());
        assert_eq!(audio.payload_type, 111);
        assert_eq!(source.configured_bitrate(), 64_000);
        // Not running: retuning is rejected (fixed-looking rate).
        assert!(!source.set_bitrate(32_000));
    }

    #[tokio::test]
    async fn inject_audio_rtp_packet_routes_to_audio_track() {
        use crate::forward::PeerForward;
        use rtc::rtp::header::Header;
        use rtc::rtp::packet::Packet;
        use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;

        let forward = PeerForward::new(
            "mic-inject",
            vec![],
            vec![],
            true,
            std::sync::Arc::new(crate::stream::cable::CableHub::bare()),
            api::strategy::Strategy::default(),
            broadcast::channel(4).0,
        );
        let codec = NativeEncodedSource::new(
            "mic".to_string(),
            None,
            Some(sine_audio_params()),
            None,
            vec![],
        )
        .get_audio_codec()
        .await
        .expect("audio codec");
        forward
            .add_virtual_track(RtpCodecKind::Audio, codec)
            .await
            .expect("add audio virtual track");

        let mut track_rx = {
            let tracks = forward.internal.publish_tracks.read().await;
            tracks
                .iter()
                .find(|t| t.kind() == RtpCodecKind::Audio)
                .expect("audio track exists")
                .subscribe()
        };

        let packet = Packet {
            header: Header {
                version: 2,
                marker: true,
                payload_type: 111,
                sequence_number: 1,
                timestamp: 960,
                ssrc: 0x1234,
                ..Default::default()
            },
            payload: bytes::Bytes::from_static(&[0xF8, 0x01]),
        };
        forward
            .inject_audio_rtp_packet(std::sync::Arc::new(packet))
            .await
            .expect("inject audio RTP");

        let received = tokio::time::timeout(std::time::Duration::from_secs(2), track_rx.recv())
            .await
            .expect("packet on audio track")
            .expect("track open");
        assert_eq!(received.header.payload_type, 111);
        assert_eq!(received.payload.to_vec(), vec![0xF8, 0x01]);

        // No video track: video injection must report the missing leg.
        let video_packet = Packet {
            header: Header {
                payload_type: 96,
                ..Default::default()
            },
            payload: bytes::Bytes::new(),
        };
        forward
            .inject_video_rtp_packet(std::sync::Arc::new(video_packet))
            .await
            .expect_err("no video track");
    }
}
