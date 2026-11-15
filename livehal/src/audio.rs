//! Pure-Rust audio capture + Opus encode pipeline (no C++ pipeline involved).
//!
//! Data flow:
//!   capture backend thread → PCM chunk stream → frame slicer → Opus encoder
//!     → `EncodedPacket` mpsc
//!
//! RTP contract: Opus 48 kHz, payload type 111, clock rate 48000,
//! `codec == AUDIO_CODEC_OPUS`, `flags == 0`, `pts_us == dts_us` on the
//! [`crate::monotonic_us`] clock.  v1 captures at 48000 Hz only (no
//! resampling), 1 or 2 channels.

use crate::types::{AUDIO_CODEC_OPUS, EncodedPacket};
use anyhow::{Context, Result, bail};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use tokio::sync::{mpsc, watch};
use tracing::warn;

/// Generous cap for one encoded Opus packet (a single frame never reaches
/// half of this, even at 510 kbps with 60 ms frames).
const MAX_OPUS_PACKET: usize = 4000;

/// Capture backends emit PCM in chunks of this duration.
const CHUNK_MS: u64 = 20;

/// Opus application mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioApplication {
    Audio,
    Voip,
    LowDelay,
}

impl AudioApplication {
    fn to_opus(self) -> opus_rs::Application {
        match self {
            Self::Audio => opus_rs::Application::Audio,
            Self::Voip => opus_rs::Application::Voip,
            Self::LowDelay => opus_rs::Application::RestrictedLowDelay,
        }
    }
}

/// Parameters for creating an audio capture+encode pipeline.
///
/// The codec is implied Opus ([`AUDIO_CODEC_OPUS`]); there is no codec field.
#[derive(Debug, Clone)]
pub struct AudioSourceParams {
    /// Capture backend: "alsa" | "sine".
    pub capture_backend: String,
    /// ALSA PCM device, e.g. "default" (unused by the sine backend).
    pub capture_device: String,
    /// Capture sample rate; v1: must be 48000 (no resampling).
    pub sample_rate: u32,
    /// Channel count: 1 | 2.
    pub channels: u32,
    /// Opus bitrate in bps; 6_000..=510_000.
    pub bitrate: u32,
    pub application: AudioApplication,
    /// Opus frame size in ms: 5 | 10 | 20 | 40 | 60 (default 20).
    pub frame_ms: u32,
    /// Variable bitrate (default true; false = CBR).
    pub vbr: bool,
    /// Opus in-band FEC (default false).
    pub inband_fec: bool,
    /// Opus discontinuous transmission (default false).
    pub dtx: bool,
    /// RTP payload type (111).
    pub payload_type: u32,
    /// RTP clock rate (48000).
    pub clock_rate: u32,
    /// Opus encoder complexity, 0..=10 (default 9).
    pub complexity: u32,
}

fn validate(params: &AudioSourceParams) -> Result<()> {
    if params.sample_rate != 48_000 {
        bail!(
            "audio v1 captures at 48000 Hz only (got {})",
            params.sample_rate
        );
    }
    if params.channels != 1 && params.channels != 2 {
        bail!("audio channels must be 1 or 2 (got {})", params.channels);
    }
    if !(6_000..=510_000).contains(&params.bitrate) {
        bail!(
            "opus bitrate out of range 6000..=510000 (got {})",
            params.bitrate
        );
    }
    if ![5, 10, 20, 40, 60].contains(&params.frame_ms) {
        bail!(
            "opus frame_ms must be one of 5/10/20/40/60 (got {})",
            params.frame_ms
        );
    }
    if params.complexity > 10 {
        bail!(
            "opus complexity out of range 0..=10 (got {})",
            params.complexity
        );
    }
    match params.capture_backend.as_str() {
        "sine" => {}
        #[cfg(all(feature = "capture-alsa", target_os = "linux"))]
        "alsa" => {}
        other => bail!("unknown or unavailable audio capture backend {other:?}"),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Capture backends
// ---------------------------------------------------------------------------

struct PcmChunk {
    pts_us: u64,
    samples: Vec<i16>, // interleaved
}

trait Capture: Send {
    /// Blocking producer loop: push chunks until `stop` is set or the receiver
    /// is gone. Backends may recover and reopen after capture errors.
    fn run(self: Box<Self>, tx: mpsc::UnboundedSender<PcmChunk>, stop: Arc<AtomicBool>);
}

fn build_capture(params: &AudioSourceParams) -> Result<Box<dyn Capture>> {
    match params.capture_backend.as_str() {
        "sine" => Ok(Box::new(SineCapture {
            sample_rate: params.sample_rate,
            channels: params.channels as usize,
        })),
        #[cfg(all(feature = "capture-alsa", target_os = "linux"))]
        "alsa" => Ok(Box::new(alsa_capture::AlsaCapture::new(params)?)),
        other => bail!("unknown or unavailable audio capture backend {other:?}"),
    }
}

/// Synthetic 440 Hz sine generator, paced in real time — the CI/dev test path
/// (no sound card required).
struct SineCapture {
    sample_rate: u32,
    channels: usize,
}

const SINE_FREQ_HZ: f64 = 440.0;
const SINE_AMPLITUDE: f64 = 0.3;

impl Capture for SineCapture {
    fn run(self: Box<Self>, tx: mpsc::UnboundedSender<PcmChunk>, stop: Arc<AtomicBool>) {
        use std::time::{Duration, Instant};

        let per_channel = self.sample_rate as usize * CHUNK_MS as usize / 1000;
        let mut chunk: u64 = 0;
        let start = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            let pts_us = crate::monotonic_us();
            let mut samples = vec![0i16; per_channel * self.channels];
            for i in 0..per_channel {
                let t = (chunk * per_channel as u64 + i as u64) as f64 / self.sample_rate as f64;
                let v = (SINE_AMPLITUDE
                    * (2.0 * std::f64::consts::PI * SINE_FREQ_HZ * t).sin()
                    * i16::MAX as f64) as i16;
                for c in 0..self.channels {
                    samples[i * self.channels + c] = v;
                }
            }
            if tx.send(PcmChunk { pts_us, samples }).is_err() {
                break;
            }
            chunk += 1;
            // Pace against an absolute deadline so chunks do not drift.
            let deadline = start + Duration::from_millis(chunk * CHUNK_MS);
            let now = Instant::now();
            if deadline > now {
                std::thread::sleep(deadline - now);
            }
        }
    }
}

#[cfg(all(feature = "capture-alsa", target_os = "linux"))]
mod alsa_capture {
    use super::*;
    use alsa::pcm::{Access, Format, HwParams, PCM};
    use alsa::{Direction, ValueOr};
    use tracing::error;

    pub struct AlsaCapture {
        device: String,
        sample_rate: u32,
        channels: usize,
        pcm: Option<PCM>,
        #[cfg(test)]
        fail_after_first_chunk: std::cell::Cell<bool>,
        #[cfg(test)]
        reopen_failures_remaining: std::cell::Cell<u8>,
    }

    impl AlsaCapture {
        pub fn new(params: &AudioSourceParams) -> Result<Self> {
            let mut capture = Self {
                device: params.capture_device.clone(),
                sample_rate: params.sample_rate,
                channels: params.channels as usize,
                pcm: None,
                #[cfg(test)]
                fail_after_first_chunk: std::cell::Cell::new(false),
                #[cfg(test)]
                reopen_failures_remaining: std::cell::Cell::new(0),
            };
            let pcm = capture.open()?;
            capture.pcm = Some(pcm);
            Ok(capture)
        }

        fn open(&self) -> Result<PCM> {
            #[cfg(test)]
            if self.reopen_failures_remaining.get() > 0 {
                self.reopen_failures_remaining
                    .set(self.reopen_failures_remaining.get() - 1);
                bail!("injected ALSA reopen failure");
            }
            let pcm = PCM::new(&self.device, Direction::Capture, true)
                .with_context(|| format!("snd_pcm_open {:?}", self.device))?;
            {
                let hwp = HwParams::any(&pcm)?;
                hwp.set_access(Access::RWInterleaved)?;
                hwp.set_format(Format::S16LE)?;
                hwp.set_channels(self.channels as u32)?;
                // v1: no resampling — the device must accept the rate natively.
                hwp.set_rate_resample(false)?;
                hwp.set_rate(self.sample_rate, ValueOr::Nearest)?;
                let rate = hwp.get_rate()?;
                if rate != self.sample_rate {
                    bail!("alsa device gave {rate} Hz, need {}", self.sample_rate);
                }
                hwp.set_period_time_near(CHUNK_MS as u32 * 1000, ValueOr::Nearest)?;
                hwp.set_buffer_time_near(100_000, ValueOr::Nearest)?;
                pcm.hw_params(&hwp)?;
            }
            pcm.prepare()?;
            pcm.io_i16()?;
            Ok(pcm)
        }

        fn capture(
            &self,
            pcm: &PCM,
            tx: &mpsc::UnboundedSender<PcmChunk>,
            stop: &Arc<AtomicBool>,
        ) -> Result<()> {
            let io = pcm.io_i16()?;
            let period = self.sample_rate as usize * CHUNK_MS as usize / 1000;
            let mut buf = vec![0i16; period * self.channels];
            while !stop.load(Ordering::Relaxed) {
                match io.readi(&mut buf) {
                    Ok(frames) => {
                        if frames == 0 {
                            continue;
                        }
                        let pts_us = crate::monotonic_us();
                        let samples = buf[..frames * self.channels].to_vec();
                        if tx.send(PcmChunk { pts_us, samples }).is_err() {
                            return Ok(());
                        }
                        #[cfg(test)]
                        if self.fail_after_first_chunk.replace(false) {
                            bail!("injected ALSA read failure");
                        }
                    }
                    Err(e) if e.errno() == libc::EAGAIN || e.errno() == libc::EINTR => {
                        if !pcm.wait(Some(100))? {
                            continue;
                        }
                    }
                    Err(e) => {
                        pcm.recover(e.errno(), true)
                            .with_context(|| format!("snd_pcm_recover after {e}"))?;
                    }
                }
            }
            Ok(())
        }

        fn reopen(&self, stop: &Arc<AtomicBool>) -> Option<PCM> {
            while !stop.load(Ordering::Relaxed) {
                match self.open() {
                    Ok(pcm) => {
                        tracing::info!(device = %self.device, "ALSA capture reopened");
                        return Some(pcm);
                    }
                    Err(e) => {
                        warn!(device = %self.device, error = %e, "ALSA reopen failed; retrying in 1s");
                    }
                }
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
                while !stop.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            None
        }
    }

    impl Capture for AlsaCapture {
        fn run(mut self: Box<Self>, tx: mpsc::UnboundedSender<PcmChunk>, stop: Arc<AtomicBool>) {
            let Some(mut pcm) = self.pcm.take() else {
                error!(device = %self.device, "ALSA PCM was not initialized");
                return;
            };
            while !stop.load(Ordering::Relaxed) {
                match self.capture(&pcm, &tx, &stop) {
                    Ok(()) => return,
                    Err(e) => {
                        warn!(device = %self.device, error = %e, "ALSA capture interrupted; reopening");
                    }
                }
                drop(pcm);
                let Some(reopened) = self.reopen(&stop) else {
                    break;
                };
                pcm = reopened;
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn invalid_device_fails_open() {
            let mut params = AudioSourceParams {
                capture_backend: "alsa".into(),
                capture_device: "live777-test-no-such-device".into(),
                sample_rate: 48_000,
                channels: 2,
                bitrate: 64_000,
                application: AudioApplication::Audio,
                frame_ms: 20,
                vbr: true,
                inband_fec: false,
                dtx: false,
                payload_type: 111,
                clock_rate: 48_000,
                complexity: 9,
            };
            assert!(AlsaCapture::new(&params).is_err());

            params.capture_device = "live777-test-no-such-device-2".into();
            let capture = AlsaCapture {
                device: params.capture_device,
                sample_rate: params.sample_rate,
                channels: params.channels as usize,
                pcm: None,
                fail_after_first_chunk: std::cell::Cell::new(false),
                reopen_failures_remaining: std::cell::Cell::new(0),
            };
            let stop = Arc::new(AtomicBool::new(false));
            let stop_from_thread = Arc::clone(&stop);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                stop_from_thread.store(true, Ordering::Relaxed);
            });
            let start = std::time::Instant::now();
            assert!(capture.reopen(&stop).is_none());
            assert!(start.elapsed() < std::time::Duration::from_secs(1));
        }

        #[test]
        fn capture_reopens_and_stops_after_read_failure() {
            let params = AudioSourceParams {
                capture_backend: "alsa".into(),
                capture_device: std::env::var("LIVE777_TEST_ALSA_DEVICE")
                    .unwrap_or_else(|_| "null".into()),
                sample_rate: 48_000,
                channels: 2,
                bitrate: 64_000,
                application: AudioApplication::Audio,
                frame_ms: 20,
                vbr: true,
                inband_fec: false,
                dtx: false,
                payload_type: 111,
                clock_rate: 48_000,
                complexity: 9,
            };
            let capture = AlsaCapture::new(&params).unwrap();
            capture.fail_after_first_chunk.set(true);
            capture.reopen_failures_remaining.set(1);
            let (tx, mut rx) = mpsc::unbounded_channel();
            let stop = Arc::new(AtomicBool::new(false));
            let capture_stop = Arc::clone(&stop);
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let thread = std::thread::spawn(move || {
                Box::new(capture).run(tx, capture_stop);
                let _ = done_tx.send(());
            });
            let (chunk_tx, chunk_rx) = std::sync::mpsc::channel();
            let forward = std::thread::spawn(move || {
                while let Some(chunk) = rx.blocking_recv() {
                    if chunk_tx.send(chunk).is_err() {
                        break;
                    }
                }
            });

            let first = chunk_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("initial capture chunk");
            assert!(!first.samples.is_empty());
            assert!(
                chunk_rx
                    .recv_timeout(std::time::Duration::from_secs(3))
                    .is_ok(),
                "capture did not resume after reopen"
            );
            stop.store(true, Ordering::Relaxed);
            assert!(
                done_rx
                    .recv_timeout(std::time::Duration::from_secs(1))
                    .is_ok(),
                "capture did not stop within its bounded wait"
            );
            thread.join().unwrap();
            forward.join().unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// Frame slicer
// ---------------------------------------------------------------------------

/// Accumulates interleaved PCM chunks into whole Opus frames; a frame's pts
/// is the chunk pts plus the in-chunk sample offset converted to µs.
struct FrameSlicer {
    pending: Vec<i16>,
    pending_pts_us: u64,
    samples_per_channel: usize,
    channels: usize,
    sample_rate: u32,
}

impl FrameSlicer {
    fn new(sample_rate: u32, channels: usize, frame_ms: u32) -> Self {
        Self {
            pending: Vec::new(),
            pending_pts_us: 0,
            samples_per_channel: sample_rate as usize * frame_ms as usize / 1000,
            channels,
            sample_rate,
        }
    }

    fn frame_len(&self) -> usize {
        self.samples_per_channel * self.channels
    }

    fn push(&mut self, pts_us: u64, samples: &[i16]) {
        if self.pending.is_empty() {
            self.pending_pts_us = pts_us;
        }
        self.pending.extend_from_slice(samples);
    }

    fn next_frame(&mut self) -> Option<(u64, Vec<i16>)> {
        if self.pending.len() < self.frame_len() {
            return None;
        }
        let pts_us = self.pending_pts_us;
        let frame: Vec<i16> = self.pending.drain(..self.frame_len()).collect();
        self.pending_pts_us +=
            self.samples_per_channel as u64 * 1_000_000 / self.sample_rate as u64;
        Some((pts_us, frame))
    }
}

// ---------------------------------------------------------------------------
// Encode task
// ---------------------------------------------------------------------------

struct EncodeTask {
    enc: opus_rs::OpusEncoder,
    slicer: FrameSlicer,
    rx: mpsc::UnboundedReceiver<PcmChunk>,
    tx: mpsc::UnboundedSender<EncodedPacket>,
    bitrate_rx: watch::Receiver<u32>,
    bitrate: u32,
}

impl EncodeTask {
    fn run(&mut self) {
        let mut out = vec![0u8; MAX_OPUS_PACKET];
        while let Some(chunk) = self.rx.blocking_recv() {
            self.slicer.push(chunk.pts_us, &chunk.samples);
            while let Some((pts_us, frame)) = self.slicer.next_frame() {
                self.poll_bitrate();
                match self
                    .enc
                    .encode_i16(&frame, self.slicer.samples_per_channel, &mut out)
                {
                    Ok(n) => {
                        let pkt = EncodedPacket {
                            codec: AUDIO_CODEC_OPUS,
                            data: out[..n].to_vec(),
                            pts_us,
                            dts_us: pts_us,
                            flags: 0,
                        };
                        if self.tx.send(pkt).is_err() {
                            return;
                        }
                    }
                    Err(e) => warn!("opus encode failed: {e}"),
                }
            }
        }
    }

    fn poll_bitrate(&mut self) {
        if self.bitrate_rx.has_changed().unwrap_or(false) {
            let bps = *self.bitrate_rx.borrow_and_update();
            if bps != self.bitrate {
                self.enc.bitrate_bps = bps as i32;
                self.bitrate = bps;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AudioPipeline
// ---------------------------------------------------------------------------

/// Owns an audio capture+encode pipeline and delivers [`EncodedPacket`]
/// frames through a channel.  Mirrors the lifecycle style of
/// [`crate::source_pipeline::NativePipeline`]: `start` returns the receiver,
/// `stop` tears down, `Drop` stops.
pub struct AudioPipeline {
    params: AudioSourceParams,
    stop: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    bitrate_tx: watch::Sender<u32>,
    threads: Vec<JoinHandle<()>>,
    pkt_tx: Option<mpsc::UnboundedSender<EncodedPacket>>,
    rx: Option<mpsc::UnboundedReceiver<EncodedPacket>>,
}

impl AudioPipeline {
    /// Create a new pipeline from the given parameters (validated here).
    ///
    /// The pipeline is configured but **not started** — call [`AudioPipeline::start`]
    /// to begin streaming.
    pub fn new(params: &AudioSourceParams) -> Result<Self> {
        validate(params)?;
        let (bitrate_tx, _) = watch::channel(params.bitrate);
        let (pkt_tx, pkt_rx) = mpsc::unbounded_channel::<EncodedPacket>();
        Ok(Self {
            params: params.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            running: Arc::new(AtomicBool::new(false)),
            bitrate_tx,
            threads: Vec::new(),
            pkt_tx: Some(pkt_tx),
            rx: Some(pkt_rx),
        })
    }

    /// Start streaming; returns a receiver that yields [`EncodedPacket`]
    /// Opus frames. Capture open failures are returned synchronously; a
    /// successfully opened capture can recover and reopen after read failures.
    pub fn start(&mut self) -> Result<mpsc::UnboundedReceiver<EncodedPacket>> {
        if self.rx.is_none() || self.pkt_tx.is_none() {
            bail!("start called twice");
        }
        let capture = build_capture(&self.params)?;
        let rx = self.rx.take().expect("audio receiver was checked");
        let pkt_tx = self.pkt_tx.take().expect("audio sender was checked");
        let params = &self.params;

        let (pcm_tx, pcm_rx) = mpsc::unbounded_channel::<PcmChunk>();

        let mut enc = opus_rs::OpusEncoder::new(
            params.sample_rate as i32,
            params.channels as usize,
            params.application.to_opus(),
        )
        .map_err(|e| anyhow::anyhow!("opus encoder init failed: {e}"))?;
        enc.bitrate_bps = params.bitrate as i32;
        enc.complexity = params.complexity as i32;
        enc.use_cbr = !params.vbr;
        enc.use_inband_fec = params.inband_fec;
        enc.set_dtx(params.dtx);

        let mut task = EncodeTask {
            enc,
            slicer: FrameSlicer::new(
                params.sample_rate,
                params.channels as usize,
                params.frame_ms,
            ),
            rx: pcm_rx,
            tx: pkt_tx,
            bitrate_rx: self.bitrate_tx.subscribe(),
            bitrate: params.bitrate,
        };
        let enc_thread = std::thread::Builder::new()
            .name("livehal-audio-encode".into())
            .spawn(move || task.run())
            .context("spawn audio encode thread")?;

        let stop = Arc::clone(&self.stop);
        let cap_thread = match std::thread::Builder::new()
            .name("livehal-audio-capture".into())
            .spawn(move || capture.run(pcm_tx, stop))
        {
            Ok(handle) => handle,
            Err(e) => {
                self.stop.store(true, Ordering::Relaxed);
                let _ = enc_thread.join();
                self.stop.store(false, Ordering::Relaxed);
                return Err(e).context("spawn audio capture thread");
            }
        };

        self.stop.store(false, Ordering::Relaxed);
        self.threads = vec![cap_thread, enc_thread];
        self.running.store(true, Ordering::Relaxed);
        Ok(rx)
    }

    /// Stop streaming and join the pipeline threads.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.running.store(false, Ordering::Relaxed);
        for h in self.threads.drain(..) {
            let _ = h.join();
        }
    }

    /// Return a cloneable handle for retuning the encoder bitrate from other
    /// tasks (adaptive bitrate control).
    pub fn bitrate_handle(&self) -> AudioBitrateHandle {
        AudioBitrateHandle {
            tx: self.bitrate_tx.clone(),
            running: Arc::clone(&self.running),
        }
    }
}

impl Drop for AudioPipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Cloneable handle for changing the encoder bitrate at runtime.
///
/// Returns false from [`AudioBitrateHandle::set_bitrate`] when the pipeline
/// is not running; callers should treat that as "fixed bitrate" and stop
/// adjusting.
#[derive(Clone)]
pub struct AudioBitrateHandle {
    tx: watch::Sender<u32>,
    running: Arc<AtomicBool>,
}

impl AudioBitrateHandle {
    /// False when the pipeline is not running.
    pub fn set_bitrate(&self, bps: u32) -> bool {
        if !self.running.load(Ordering::Relaxed) {
            return false;
        }
        self.tx.send(bps).is_ok()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn sine_params() -> AudioSourceParams {
        AudioSourceParams {
            capture_backend: "sine".into(),
            capture_device: String::new(),
            sample_rate: 48_000,
            channels: 2,
            bitrate: 64_000,
            application: AudioApplication::Audio,
            frame_ms: 20,
            vbr: true,
            inband_fec: false,
            dtx: false,
            payload_type: 111,
            clock_rate: 48_000,
            complexity: 9,
        }
    }

    /// Receive up to `n` packets, stopping early when the stream ends.
    fn recv_n(rx: &mut mpsc::UnboundedReceiver<EncodedPacket>, n: usize) -> Vec<EncodedPacket> {
        let mut out = Vec::new();
        while out.len() < n {
            match rx.blocking_recv() {
                Some(p) => out.push(p),
                None => break,
            }
        }
        out
    }

    fn rms_f32(samples: &[f32]) -> f64 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f64 = samples
            .iter()
            .map(|&s| (s as f64) * (s as f64))
            .sum::<f64>();
        (sum / samples.len() as f64).sqrt()
    }

    #[test]
    fn params_validation() {
        assert!(AudioPipeline::new(&sine_params()).is_ok());

        let mut p = sine_params();
        p.sample_rate = 44_100;
        assert!(AudioPipeline::new(&p).is_err());

        let mut p = sine_params();
        p.channels = 3;
        assert!(AudioPipeline::new(&p).is_err());

        let mut p = sine_params();
        p.frame_ms = 15;
        assert!(AudioPipeline::new(&p).is_err());

        let mut p = sine_params();
        p.bitrate = 5_999;
        assert!(AudioPipeline::new(&p).is_err());

        let mut p = sine_params();
        p.bitrate = 510_001;
        assert!(AudioPipeline::new(&p).is_err());

        let mut p = sine_params();
        p.capture_backend = "nope".into();
        assert!(AudioPipeline::new(&p).is_err());
    }

    #[test]
    fn slicer_boundary() {
        let mut slicer = FrameSlicer::new(48_000, 2, 20);
        let frame_len = slicer.frame_len();
        assert_eq!(frame_len, 960 * 2);

        // Feed 1.5 frames → exactly one frame out.
        let chunk = vec![1i16; frame_len + frame_len / 2];
        slicer.push(1_000_000, &chunk);
        let (pts_a, frame_a) = slicer.next_frame().expect("first frame");
        assert_eq!(frame_a.len(), frame_len);
        assert!(slicer.next_frame().is_none());

        // Feed 0.5 more frame → exactly one more frame out.
        let chunk = vec![2i16; frame_len / 2];
        slicer.push(1_030_000, &chunk);
        let (pts_b, frame_b) = slicer.next_frame().expect("second frame");
        assert_eq!(frame_b.len(), frame_len);
        assert!(slicer.next_frame().is_none());

        // Frame pts advances by exactly the frame duration.
        assert_eq!(pts_b - pts_a, 20_000);
        assert_eq!(pts_a, 1_000_000);
    }

    #[test]
    fn roundtrip_sine_opus_decode() {
        let params = sine_params();
        let channels = params.channels as usize;
        let mut pipe = AudioPipeline::new(&params).unwrap();
        let mut rx = pipe.start().unwrap();
        // ~1 s of audio at 20 ms frames.
        let packets = recv_n(&mut rx, 50);
        pipe.stop();
        assert_eq!(packets.len(), 50, "stream ended early");
        for p in &packets {
            assert_eq!(p.codec, AUDIO_CODEC_OPUS);
            assert_eq!(p.flags, 0);
            assert_eq!(p.pts_us, p.dts_us);
            assert!(!p.data.is_empty());
        }

        let mut dec = opus_rs::OpusDecoder::new(48_000, channels).unwrap();
        let mut pcm = vec![0f32; 5760 * channels];
        let mut decoded = Vec::new();
        // Skip the first frames (encoder ramp-up) before measuring energy.
        for p in packets.iter().skip(5) {
            let n = dec.decode(&p.data, 5760, &mut pcm).unwrap();
            decoded.extend_from_slice(&pcm[..n * channels]);
        }
        assert!(!decoded.is_empty());

        // Source sine amplitude 0.3 → RMS ≈ 0.3/√2 ≈ 0.212; allow a generous
        // factor both ways (codec noise shaping, DTX off, VBR).
        let rms = rms_f32(&decoded);
        assert!(
            (0.07..=0.6).contains(&rms),
            "decoded RMS {rms} not near source sine RMS ~0.212"
        );
    }

    #[test]
    fn bitrate_handle_reports_running() {
        let params = sine_params();
        let mut pipe = AudioPipeline::new(&params).unwrap();
        let handle = pipe.bitrate_handle();
        assert!(!handle.set_bitrate(32_000), "not running before start");

        let mut rx = pipe.start().unwrap();
        assert!(handle.set_bitrate(32_000), "running after start");
        let packets = recv_n(&mut rx, 5);
        assert_eq!(packets.len(), 5);

        pipe.stop();
        assert!(!handle.set_bitrate(48_000), "not running after stop");
    }

    #[test]
    fn bitrate_changes_packet_size() {
        fn avg_size_at(bitrate: u32) -> f64 {
            let mut params = sine_params();
            params.bitrate = bitrate;
            let mut pipe = AudioPipeline::new(&params).unwrap();
            let mut rx = pipe.start().unwrap();
            let packets = recv_n(&mut rx, 30);
            pipe.stop();
            assert_eq!(packets.len(), 30);
            let sizes: Vec<usize> = packets.iter().skip(5).map(|p| p.data.len()).collect();
            sizes.iter().sum::<usize>() as f64 / sizes.len() as f64
        }

        let avg_16k = avg_size_at(16_000);
        let avg_128k = avg_size_at(128_000);
        assert!(
            avg_128k > avg_16k,
            "avg packet size at 128k ({avg_128k}) should exceed 16k ({avg_16k})"
        );
    }

    #[test]
    fn sine_pacing_realtime() {
        let params = sine_params();
        let mut pipe = AudioPipeline::new(&params).unwrap();
        let mut rx = pipe.start().unwrap();

        let first = rx.blocking_recv().expect("first packet");
        let t0 = Instant::now();
        let rest = recv_n(&mut rx, 49);
        let elapsed = t0.elapsed();
        pipe.stop();
        assert_eq!(rest.len(), 49);
        drop(first);

        // 50 frames of 20 ms ≈ 1 s wall time; generous bounds, no flakiness.
        assert!(
            (Duration::from_millis(800)..=Duration::from_millis(1_500)).contains(&elapsed),
            "49 inter-frame intervals took {elapsed:?}"
        );
    }

    #[cfg(all(feature = "capture-alsa", target_os = "linux"))]
    #[test]
    fn alsa_open_failure_is_synchronous() {
        let mut params = sine_params();
        params.capture_backend = "alsa".into();
        params.capture_device = "live777-test-no-such-device".into();
        let mut pipe = AudioPipeline::new(&params).unwrap();
        let error = match pipe.start() {
            Ok(_) => panic!("an invalid ALSA device must not start"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("snd_pcm_open"));
        assert!(!pipe.bitrate_handle().set_bitrate(32_000));
        pipe.stop();
    }

    #[cfg(all(feature = "capture-alsa", target_os = "linux"))]
    #[test]
    fn alsa_capture_smoke() {
        let mut params = sine_params();
        params.capture_backend = "alsa".into();
        params.capture_device =
            std::env::var("LIVE777_TEST_ALSA_DEVICE").unwrap_or_else(|_| "default".into());
        let mut pipe = AudioPipeline::new(&params).unwrap();
        let mut rx = match pipe.start() {
            Ok(rx) => rx,
            Err(e) => {
                eprintln!("alsa_capture_smoke: no ALSA device present, skipping: {e:#}");
                pipe.stop();
                return;
            }
        };

        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Some(p) = rx.blocking_recv() {
                if done_tx.send(p).is_err() {
                    break;
                }
            }
        });
        let first = match done_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(p) => p,
            Err(_) => {
                eprintln!("alsa_capture_smoke: no ALSA device present, skipping");
                pipe.stop();
                return;
            }
        };
        assert_eq!(first.codec, AUDIO_CODEC_OPUS);
        assert!(!first.data.is_empty());
        // A few more frames should keep flowing.
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).is_ok());
        pipe.stop();
    }
}
