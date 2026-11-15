//! Wayland compositor source (`wayland://`).
//!
//! live777 acts as a headless Wayland display server: GUI apps render into
//! the compositor ([`compositor`], smithay-based) and the composited image
//! is H.264-encoded and published as a WebRTC stream. Until the first real
//! client content arrives, a synthetic pattern generator ([`pattern`]) feeds
//! the pipeline so the stream is alive immediately. Desktop audio is
//! captured from a PipeWire sink monitor and published as Opus ([`audio`]).
//!
//! Data flow:
//!   frame producer → mpsc<ArgbFrame> → XRGB8888→I420 ([`convert`]) →
//!   OpenH264 ([`encoder`]) → H264 RTP packetize → broadcast
//!   PipeWire monitor → Opus → RTP packetize → broadcast (audio track)

mod audio;
mod compositor;
mod convert;
mod encoder;
mod gl;
pub mod input;
mod pattern;

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use async_trait::async_trait;
use rtc_rtp::codec::h264::H264Payloader;
use rtc_rtp::packetizer::{Packetizer as _, new_packetizer};
use rtc_rtp::sequence::new_random_sequencer;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use super::{MediaPacket, StateChangeEvent, StreamSource, StreamSourceState};

const VIDEO_PAYLOAD_TYPE: u8 = 96;
const VIDEO_CLOCK_RATE: u32 = 90_000;

/// One composited frame in Wayland `XRGB8888` shm layout (little-endian byte
/// order B,G,R,X per pixel). This is the hand-off seam between the frame
/// producer and the encode pipeline; the Stage-2 compositor replaces the
/// pattern generator without touching anything downstream of it.
pub struct ArgbFrame {
    pub width: u32,
    pub height: u32,
    /// Bytes per row in `data` (>= width * 4).
    pub stride: u32,
    pub data: Vec<u8>,
    /// Presentation time, CLOCK_MONOTONIC-relative microseconds.
    pub pts_us: u64,
    /// The pipeline must encode this frame as an IDR (set by the compositor
    /// for PLI-triggered repaints and idle keepalives, so a static desktop
    /// still hands every new subscriber a keyframe to sync on).
    pub force_keyframe: bool,
}

/// Parsed `wayland://<socket-name>?width=&height=&fps=&bitrate=` URL.
/// `socket_name` is the Wayland display name the compositor will create in
/// Stage 2; Stage 1 only parses and stores it.
#[derive(Debug, Clone)]
pub struct WaylandUrlConfig {
    pub socket_name: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    /// Spawn an XWayland server attached to the compositor so X11 clients can
    /// connect (needs an `Xwayland` binary on PATH; gracefully off without
    /// one). `xwayland=0` disables.
    pub xwayland: bool,
    /// Pinned X11 display number for XWayland (`xdisplay=3` → `DISPLAY=:3`).
    /// Unset picks the lowest free display.
    pub xdisplay: Option<u32>,
    /// Capture desktop audio from a PipeWire sink monitor and publish it as
    /// an Opus track (`audio=0` disables). Needs a PipeWire daemon reachable
    /// at `$XDG_RUNTIME_DIR/pipewire-0`; without one the stream stays
    /// video-only with a warning.
    pub audio: bool,
    /// Opus bitrate in bps (`audio_bitrate=`), default 64k.
    pub audio_bitrate: u32,
    /// Only capture from the sink whose PipeWire `node.name` contains this
    /// substring (`audio_sink=`); unset captures the first `Audio/Sink`.
    pub audio_sink: Option<String>,
    /// Accept remote keyboard/pointer input from subscribers (`input=1`):
    /// datachannel messages on the stream's `input` cable (or the anonymous
    /// default bus) are injected into the compositor's seat. Off by default:
    /// any subscriber can then drive the desktop.
    pub input: bool,
}

impl WaylandUrlConfig {
    pub const DEFAULT_WIDTH: u32 = 1280;
    pub const DEFAULT_HEIGHT: u32 = 720;
    pub const DEFAULT_FPS: u32 = 30;
    pub const DEFAULT_BITRATE: u32 = 2_000_000;
    pub const DEFAULT_AUDIO_BITRATE: u32 = 64_000;

    /// OpenH264 encodes up to 3840x2160 (or 2160x3840) and takes the target
    /// bitrate as a signed 32-bit value.
    const MAX_DIMENSION: u32 = 3840;
    const MAX_CROSS_DIMENSION: u32 = 2160;

    pub fn parse(stream_id: &str, url: &str) -> Result<Self> {
        let scheme_end = url.find("://").unwrap_or(0);
        if scheme_end == 0 || !url[..scheme_end].eq_ignore_ascii_case("wayland") {
            anyhow::bail!("not a wayland:// URL: {}", super::redact_url(url));
        }
        let rest = &url[scheme_end + 3..];
        let (authority, query) = rest.split_once('?').unwrap_or((rest, ""));

        let socket_name = if authority.is_empty() {
            format!("live777-{stream_id}")
        } else {
            authority.to_string()
        };

        let mut config = Self {
            socket_name,
            width: Self::DEFAULT_WIDTH,
            height: Self::DEFAULT_HEIGHT,
            fps: Self::DEFAULT_FPS,
            bitrate: Self::DEFAULT_BITRATE,
            xwayland: true,
            xdisplay: None,
            audio: true,
            audio_bitrate: Self::DEFAULT_AUDIO_BITRATE,
            audio_sink: None,
            input: false,
        };
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            let parsed = || {
                value
                    .parse::<u32>()
                    .map_err(|e| anyhow::anyhow!("wayland URL: invalid {key} '{value}': {e}"))
            };
            match key.as_ref() {
                "width" => config.width = parsed()?,
                "height" => config.height = parsed()?,
                "fps" => config.fps = parsed()?,
                "bitrate" => config.bitrate = parsed()?,
                "xwayland" => {
                    config.xwayland = parse_bool(&key, &value)?;
                }
                "xdisplay" => config.xdisplay = Some(parsed()?),
                "audio" => {
                    config.audio = parse_bool(&key, &value)?;
                }
                "audio_bitrate" => config.audio_bitrate = parsed()?,
                "audio_sink" => config.audio_sink = Some(value.into_owned()),
                "input" => {
                    config.input = parse_bool(&key, &value)?;
                }
                _ => {}
            }
        }
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            anyhow::bail!("wayland URL: width/height must be non-zero");
        }
        // I420 4:2:0 needs even dimensions; the pattern generator tiles in
        // fixed-size cells.
        if !self.width.is_multiple_of(2) || !self.height.is_multiple_of(2) {
            anyhow::bail!("wayland URL: width/height must be even");
        }
        if self.width < 64 || self.height < 64 {
            anyhow::bail!("wayland URL: width/height must be at least 64");
        }
        let greater = self.width.max(self.height);
        let smaller = self.width.min(self.height);
        if greater > Self::MAX_DIMENSION || smaller > Self::MAX_CROSS_DIMENSION {
            anyhow::bail!(
                "wayland URL: resolution {}x{} exceeds the encoder limit ({}x{} or {}x{})",
                self.width,
                self.height,
                Self::MAX_DIMENSION,
                Self::MAX_CROSS_DIMENSION,
                Self::MAX_CROSS_DIMENSION,
                Self::MAX_DIMENSION
            );
        }
        if self.fps == 0 || self.fps > 240 {
            anyhow::bail!("wayland URL: fps must be in 1..=240");
        }
        if self.bitrate == 0 || self.bitrate > i32::MAX as u32 {
            anyhow::bail!("wayland URL: bitrate must be in 1..={}", i32::MAX);
        }
        if self.audio_bitrate < 500 || self.audio_bitrate > 512_000 {
            anyhow::bail!("wayland URL: audio_bitrate must be in 500..=512000 (Opus limits)");
        }
        Ok(())
    }
}

/// `1/true/on` vs `0/false/off` for boolean URL parameters.
fn parse_bool(key: &str, value: &str) -> Result<bool> {
    match value {
        "1" | "true" | "on" => Ok(true),
        "0" | "false" | "off" => Ok(false),
        _ => anyhow::bail!("wayland URL: invalid {key} '{value}' (expected 0/1/true/false)"),
    }
}

pub struct WaylandSource {
    stream_id: String,
    config: WaylandUrlConfig,
    state: Arc<RwLock<StreamSourceState>>,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    /// Cancellation for the whole producer side (pattern, compositor,
    /// pipeline); recreated on each `start()` so a restarted source works.
    cancel: Option<CancellationToken>,
    compositor: Option<compositor::CompositorHandle>,
    /// PipeWire desktop-audio capture; `Some` only when `audio=1` and the
    /// daemon handshake succeeded (otherwise the stream is video-only).
    audio: Option<audio::AudioHandle>,
    generator_handle: Option<tokio::task::JoinHandle<()>>,
    mux_handle: Option<tokio::task::JoinHandle<()>>,
    pipeline_handle: Option<tokio::task::JoinHandle<()>>,
    rtcp_handle: Option<tokio::task::JoinHandle<()>>,
    rtcp_sender: Option<mpsc::UnboundedSender<Vec<u8>>>,
    /// Set by the RTCP task on PLI/FIR; consumed by the pipeline, which
    /// forces the encoder's next frame to an IDR.
    keyframe_request: Arc<AtomicBool>,
    /// Set by the RTCP task on PLI/FIR; consumed by the compositor's frame
    /// timer, which repaints and re-emits the current canvas even when no
    /// client commit happened — otherwise a static desktop would never
    /// deliver the requested keyframe to a newly joined subscriber.
    repaint_request: Arc<AtomicBool>,
    /// Live target bitrate; the pipeline recreates the encoder when it
    /// diverges (OpenH264's safe API has no runtime retune, and recreation
    /// is sub-millisecond, starting with a fresh IDR).
    bitrate: Arc<AtomicU32>,
    dynamic_profile: Arc<tokio::sync::RwLock<Option<String>>>,
    running: bool,
}

impl WaylandSource {
    pub fn new(stream_id: &str, url: &str) -> Result<Self> {
        let config = WaylandUrlConfig::parse(stream_id, url)?;
        let (rtp_tx, _) = broadcast::channel(1024);
        let (state_tx, _) = broadcast::channel(16);

        Ok(Self {
            stream_id: stream_id.to_string(),
            bitrate: Arc::new(AtomicU32::new(config.bitrate)),
            config,
            state: Arc::new(RwLock::new(StreamSourceState::Initializing)),
            rtp_tx,
            state_tx,
            cancel: None,
            compositor: None,
            audio: None,
            generator_handle: None,
            mux_handle: None,
            pipeline_handle: None,
            rtcp_handle: None,
            rtcp_sender: None,
            keyframe_request: Arc::new(AtomicBool::new(false)),
            repaint_request: Arc::new(AtomicBool::new(false)),
            dynamic_profile: Arc::new(tokio::sync::RwLock::new(None)),
            running: false,
        })
    }

    fn set_state(&self, new_state: StreamSourceState, error: Option<String>) {
        publish_state(
            &self.stream_id,
            &self.state,
            &self.state_tx,
            new_state,
            error,
        );
    }
}

fn publish_state(
    stream_id: &str,
    state: &RwLock<StreamSourceState>,
    state_tx: &broadcast::Sender<StateChangeEvent>,
    new_state: StreamSourceState,
    error: Option<String>,
) {
    let changed = {
        let mut guard = state.write().unwrap();
        let old_state = *guard;
        if old_state != new_state {
            *guard = new_state;
            Some(old_state)
        } else {
            None
        }
    };

    if let Some(old_state) = changed {
        let _ = state_tx.send(StateChangeEvent {
            old_state,
            new_state,
            error: error.clone(),
        });
        tracing::info!(
            "[{}] state: {:?} -> {:?}{}",
            stream_id,
            old_state,
            new_state,
            error.map(|e| format!(" ({})", e)).unwrap_or_default()
        );
    }
}

/// Frames → convert → encode → packetize → broadcast. Exits on shutdown or
/// when the frame producer hangs up.
#[allow(clippy::too_many_arguments)]
async fn pipeline_loop(
    stream_id: String,
    config: WaylandUrlConfig,
    mut frame_rx: mpsc::Receiver<ArgbFrame>,
    cancel: CancellationToken,
    rtp_tx: broadcast::Sender<MediaPacket>,
    state: Arc<RwLock<StreamSourceState>>,
    state_tx: broadcast::Sender<StateChangeEvent>,
    keyframe_request: Arc<AtomicBool>,
    bitrate: Arc<AtomicU32>,
    dynamic_profile: Arc<tokio::sync::RwLock<Option<String>>>,
) {
    let mut encoder = match encoder::H264Encoder::new(config.fps, bitrate.load(Ordering::Relaxed)) {
        Ok(encoder) => encoder,
        Err(e) => {
            publish_state(
                &stream_id,
                &state,
                &state_tx,
                StreamSourceState::Error,
                Some(format!("encoder init failed: {e}")),
            );
            return;
        }
    };
    let mut converter = convert::I420Converter::new(config.width, config.height);

    let payloader = Box::new(H264Payloader::default());
    let sequencer = Box::new(new_random_sequencer());
    let ssrc: u32 = rand::random();
    let mut packetizer = new_packetizer(
        std::time::Instant::now(),
        1400,
        VIDEO_PAYLOAD_TYPE,
        ssrc,
        payloader,
        sequencer,
        VIDEO_CLOCK_RATE,
    );

    // Track the previous RTP 90 kHz timestamp to pass *increments* to
    // packetizer.packetize() (it maintains an internal base and
    // wrapping-adds the `samples` parameter after each call).
    let mut last_rtp_ts: Option<u32> = None;
    let fallback_delta = VIDEO_CLOCK_RATE / config.fps.max(1);
    let mut ever_encoded = false;

    loop {
        let frame = tokio::select! {
            _ = cancel.cancelled() => break,
            frame = frame_rx.recv() => {
                let Some(frame) = frame else { break };
                frame
            }
        };

        if frame.width != config.width || frame.height != config.height {
            tracing::warn!(
                "[{}] dropping {}x{} frame, pipeline is {}x{}",
                stream_id,
                frame.width,
                frame.height,
                config.width,
                config.height
            );
            continue;
        }

        let want_bitrate = bitrate.load(Ordering::Relaxed);
        if let Err(e) = encoder.set_bitrate(want_bitrate) {
            tracing::warn!("[{}] encoder bitrate change failed: {}", stream_id, e);
        }
        if keyframe_request.swap(false, Ordering::Relaxed) || frame.force_keyframe {
            encoder.request_keyframe();
        }

        if let Err(e) = converter.convert(&frame) {
            tracing::warn!("[{}] frame conversion failed: {}", stream_id, e);
            continue;
        }

        let au = match encoder.encode(converter.planes(), frame.pts_us) {
            Ok(Some(au)) => au,
            Ok(None) => continue,
            Err(e) => {
                // A first-frame failure is deterministic (bad encoder
                // params, which URL validation should have caught) — report
                // and give up instead of spinning. Later failures may be
                // transient; drop the frame.
                if !ever_encoded {
                    publish_state(
                        &stream_id,
                        &state,
                        &state_tx,
                        StreamSourceState::Error,
                        Some(format!("first encode failed: {e}")),
                    );
                    return;
                }
                tracing::warn!("[{}] encode failed: {}", stream_id, e);
                continue;
            }
        };
        ever_encoded = true;

        // Convert PTS (microseconds) to the RTP 90 kHz clock; u128 for the
        // intermediate product so large PTS values cannot overflow before
        // the wrap to 32 bits.
        let rtp_ts = if frame.pts_us > 0 {
            ((frame.pts_us as u128 * 9) / 100) as u32
        } else {
            0
        };
        let delta = match last_rtp_ts {
            Some(prev) if rtp_ts > prev => rtp_ts - prev,
            Some(_) => fallback_delta,
            None => 0,
        };
        last_rtp_ts = Some(rtp_ts);

        // The fmtp profile-level-id is taken from the actual SPS once the
        // first frame is through the encoder.
        let profile_known = dynamic_profile.read().await.is_some();
        if !profile_known && let Some(profile) = super::scan_sps_profile(&au.data) {
            *dynamic_profile.write().await = Some(profile);
        }
        if au.is_keyframe {
            tracing::debug!("[{}] encoded IDR ({} bytes)", stream_id, au.data.len());
        }

        match packetizer.packetize(std::time::Instant::now(), &au.data.into(), delta) {
            Ok(packets) => {
                for packet in packets {
                    let _ = rtp_tx.send(MediaPacket::RtpPacket(Arc::new(packet)));
                }
            }
            Err(e) => {
                tracing::warn!("[{}] RTP packetize error: {}", stream_id, e);
            }
        }
    }
}

/// Frame producer selection: the test pattern feeds the pipeline until the
/// compositor emits its first frame, then the compositor owns the stream
/// (permanently — a later all-clients-disconnected desktop shows the
/// composited background, never the pattern again). Single-task select loop,
/// so the switch is race-free.
async fn frame_mux(
    stream_id: String,
    frame_tx: mpsc::Sender<ArgbFrame>,
    mut pattern_rx: mpsc::Receiver<ArgbFrame>,
    mut comp_rx: mpsc::Receiver<ArgbFrame>,
    pattern_cancel: CancellationToken,
) {
    let mut pattern_open = true;
    let mut comp_open = true;
    let mut compositor_live = false;
    while pattern_open || comp_open {
        tokio::select! {
            frame = comp_rx.recv(), if comp_open => {
                match frame {
                    Some(frame) => {
                        if !compositor_live {
                            compositor_live = true;
                            pattern_cancel.cancel();
                            // Never select the pattern arm again; the
                            // producer is cancelled, its channel may linger.
                            pattern_open = false;
                            tracing::info!(
                                "[{stream_id}] wayland compositor produced its first frame; test pattern stopped"
                            );
                        }
                        let _ = frame_tx.try_send(frame);
                    }
                    None => comp_open = false,
                }
            }
            frame = pattern_rx.recv(), if pattern_open && !compositor_live => {
                match frame {
                    Some(frame) => {
                        let _ = frame_tx.try_send(frame);
                    }
                    None => pattern_open = false,
                }
            }
        }
    }
}

#[async_trait]
impl StreamSource for WaylandSource {
    fn stream_id(&self) -> &str {
        &self.stream_id
    }

    fn state(&self) -> StreamSourceState {
        *self.state.read().unwrap()
    }

    async fn start(&mut self) -> Result<()> {
        if self.running {
            anyhow::bail!("Already started");
        }

        let cancel = CancellationToken::new();
        self.cancel = Some(cancel.clone());

        let (rtcp_tx, mut rtcp_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let keyframe_request = self.keyframe_request.clone();
        let repaint_request = self.repaint_request.clone();
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
                            keyframe_request.store(true, Ordering::Relaxed);
                            repaint_request.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
        });
        self.rtcp_sender = Some(rtcp_tx);
        self.rtcp_handle = Some(rtcp_task);

        // Frames flow: producers → mux → `frame_tx` → encode pipeline.
        let (frame_tx, frame_rx) = mpsc::channel(4);
        let (pattern_tx, pattern_rx) = mpsc::channel(4);
        let (comp_tx, comp_rx) = mpsc::channel(4);

        let pattern_cancel = cancel.child_token();
        self.generator_handle = Some(pattern::spawn(
            &self.config,
            pattern_tx,
            pattern_cancel.clone(),
        ));

        // Binds the Wayland socket synchronously; a failure fails start().
        let compositor = compositor::spawn(
            &self.config,
            comp_tx,
            cancel.clone(),
            self.repaint_request.clone(),
        )
        .map_err(|e| anyhow::anyhow!("[{}] {e}", self.stream_id))?;
        tracing::info!(
            "[{}] wayland source: WAYLAND_DISPLAY={} {}x{}@{} {} bps",
            self.stream_id,
            compositor.socket_name(),
            self.config.width,
            self.config.height,
            self.config.fps,
            self.config.bitrate
        );
        self.compositor = Some(compositor);

        // Desktop audio: PipeWire sink monitor → Opus. Optional — a missing
        // daemon or any handshake failure keeps the stream video-only.
        if self.config.audio {
            match audio::spawn(
                &self.stream_id,
                &self.config,
                self.rtp_tx.clone(),
                cancel.child_token(),
            ) {
                Ok(handle) => self.audio = Some(handle),
                Err(e) => {
                    tracing::warn!(
                        "[{}] wayland audio unavailable ({e:#}); continuing video-only",
                        self.stream_id
                    );
                }
            }
        }

        self.mux_handle = Some(tokio::spawn(frame_mux(
            self.stream_id.clone(),
            frame_tx,
            pattern_rx,
            comp_rx,
            pattern_cancel,
        )));

        self.pipeline_handle = Some(tokio::spawn(pipeline_loop(
            self.stream_id.clone(),
            self.config.clone(),
            frame_rx,
            cancel.clone(),
            self.rtp_tx.clone(),
            self.state.clone(),
            self.state_tx.clone(),
            self.keyframe_request.clone(),
            self.bitrate.clone(),
            self.dynamic_profile.clone(),
        )));

        self.running = true;
        self.set_state(StreamSourceState::Connected, None);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        // Cancelling stops the pattern, the compositor's event loop (and
        // with it the socket file), and the encode pipeline.
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        // The audio capture thread runs on a child token; drop joins it.
        self.audio = None;
        if let Some(compositor) = self.compositor.take() {
            compositor.join().await;
        }

        // Drop the RTCP sender so the RTCP task exits naturally.
        self.rtcp_sender = None;

        for handle in [
            self.generator_handle.take(),
            self.mux_handle.take(),
            self.pipeline_handle.take(),
            self.rtcp_handle.take(),
        ]
        .into_iter()
        .flatten()
        {
            handle.abort();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        }

        self.running = false;
        self.set_state(StreamSourceState::Disconnected, None);
        Ok(())
    }

    fn subscribe_rtp(&self) -> broadcast::Receiver<MediaPacket> {
        self.rtp_tx.subscribe()
    }

    fn subscribe_state(&self) -> broadcast::Receiver<StateChangeEvent> {
        self.state_tx.subscribe()
    }

    fn input_sink(&self) -> Option<Box<dyn Fn(Vec<u8>) + Send + Sync>> {
        if !self.config.input {
            return None;
        }
        let tx = self.compositor.as_ref()?.input_sender()?;
        Some(Box::new(move |msg: Vec<u8>| {
            if let Some(event) = input::parse(&msg) {
                let _ = tx.send(event);
            }
        }))
    }

    async fn get_video_codec(
        &self,
    ) -> Option<rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters> {
        use rtc::rtp_transceiver::rtp_sender::{RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters};

        // Report the profile-level-id of the encoder's actual SPS; until the
        // first frame is scanned the codec is not known yet.
        let profile = self.dynamic_profile.read().await.clone()?;

        Some(RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "video/H264".into(),
                clock_rate: VIDEO_CLOCK_RATE,
                channels: 0,
                sdp_fmtp_line: format!(
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id={profile}"
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
            payload_type: VIDEO_PAYLOAD_TYPE,
        })
    }

    async fn get_audio_codec(
        &self,
    ) -> Option<rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters> {
        use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters};

        // Gate on the video codec being known: the bridge adds tracks in a
        // single pass once any codec is ready, so audio must not become
        // available *before* video or the video track would be missed.
        self.dynamic_profile.read().await.as_ref()?;
        if !self.audio.as_ref().is_some_and(|a| a.running()) {
            return None;
        }

        Some(RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: "audio/opus".into(),
                clock_rate: audio::AUDIO_CLOCK_RATE,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".into(),
                rtcp_feedback: vec![],
            },
            payload_type: audio::AUDIO_PAYLOAD_TYPE,
        })
    }

    async fn get_rtcp_sender(&self) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
        self.rtcp_sender.clone()
    }

    async fn set_bitrate(&self, bps: u32) -> bool {
        if bps == 0 || bps > i32::MAX as u32 || !self.running {
            return false;
        }
        self.bitrate.store(bps, Ordering::Relaxed);
        true
    }

    fn configured_bitrate(&self) -> Option<u32> {
        Some(self.config.bitrate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_defaults() {
        let cfg = WaylandUrlConfig::parse("cam", "wayland://").unwrap();
        assert_eq!(cfg.socket_name, "live777-cam");
        assert_eq!(cfg.width, 1280);
        assert_eq!(cfg.height, 720);
        assert_eq!(cfg.fps, 30);
        assert_eq!(cfg.bitrate, 2_000_000);
        assert!(cfg.xwayland);
        assert_eq!(cfg.xdisplay, None);
    }

    #[test]
    fn url_full_params() {
        let cfg = WaylandUrlConfig::parse(
            "cam",
            "wayland://my-display?width=1920&height=1080&fps=60&bitrate=4000000&xwayland=0&xdisplay=3",
        )
        .unwrap();
        assert_eq!(cfg.socket_name, "my-display");
        assert_eq!(
            (cfg.width, cfg.height, cfg.fps, cfg.bitrate),
            (1920, 1080, 60, 4_000_000)
        );
        assert!(!cfg.xwayland);
        assert_eq!(cfg.xdisplay, Some(3));
    }

    #[test]
    fn url_audio_and_input_params() {
        let cfg = WaylandUrlConfig::parse("cam", "wayland://d").unwrap();
        assert!(cfg.audio);
        assert_eq!(cfg.audio_bitrate, 64_000);
        assert_eq!(cfg.audio_sink, None);
        assert!(!cfg.input);

        let cfg = WaylandUrlConfig::parse(
            "cam",
            "wayland://d?audio=0&input=1&audio_bitrate=96000&audio_sink=my-sink",
        )
        .unwrap();
        assert!(!cfg.audio);
        assert!(cfg.input);
        assert_eq!(cfg.audio_bitrate, 96_000);
        assert_eq!(cfg.audio_sink.as_deref(), Some("my-sink"));

        assert!(WaylandUrlConfig::parse("cam", "wayland://d?audio_bitrate=100").is_err());
        assert!(WaylandUrlConfig::parse("cam", "wayland://d?input=maybe").is_err());
    }

    #[test]
    fn url_rejects_bad_params() {
        for url in [
            "wayland://x?width=0",
            "wayland://x?height=0",
            "wayland://x?width=1281&height=720", // odd: I420 needs even
            "wayland://x?width=32&height=32",    // below the pattern minimum
            "wayland://x?fps=0",
            "wayland://x?fps=1000",
            "wayland://x?bitrate=0",
            "wayland://x?bitrate=3000000000", // exceeds the encoder's i32 control
            "wayland://x?width=4000&height=3000",
            "wayland://x?width=abc",
            "wayland://x?xwayland=maybe",
            "wayland://x?xdisplay=abc",
        ] {
            assert!(WaylandUrlConfig::parse("cam", url).is_err(), "{url}");
        }
    }

    #[tokio::test]
    async fn frame_mux_switches_to_compositor_on_first_frame() {
        let (frame_tx, mut frame_rx) = mpsc::channel(8);
        let (pattern_tx, pattern_rx) = mpsc::channel(8);
        let (comp_tx, comp_rx) = mpsc::channel(8);
        let pattern_cancel = CancellationToken::new();
        let mux = tokio::spawn(frame_mux(
            "t".into(),
            frame_tx,
            pattern_rx,
            comp_rx,
            pattern_cancel.clone(),
        ));

        let frame = |mark: u8| ArgbFrame {
            width: 64,
            height: 64,
            stride: 256,
            data: vec![mark; 64 * 64 * 4],
            pts_us: 0,
            force_keyframe: false,
        };

        // Pattern frames flow until the compositor produces.
        pattern_tx.try_send(frame(1)).unwrap();
        assert_eq!(frame_rx.recv().await.unwrap().data[0], 1);

        // The first compositor frame switches over and stops the pattern.
        comp_tx.try_send(frame(2)).unwrap();
        assert_eq!(frame_rx.recv().await.unwrap().data[0], 2);
        assert!(pattern_cancel.is_cancelled());

        // Pattern frames after the switch are ignored, even with one pending.
        pattern_tx.try_send(frame(1)).unwrap();
        comp_tx.try_send(frame(3)).unwrap();
        assert_eq!(frame_rx.recv().await.unwrap().data[0], 3);

        // Both producers gone: the mux task ends.
        drop(comp_tx);
        drop(pattern_tx);
        tokio::time::timeout(std::time::Duration::from_secs(5), mux)
            .await
            .unwrap()
            .unwrap();
    }
}
