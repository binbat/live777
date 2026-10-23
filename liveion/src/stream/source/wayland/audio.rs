//! Desktop audio capture for the wayland source: tap a PipeWire sink's
//! monitor ports, encode to Opus, broadcast pre-packetized RTP.
//!
//! One thread owns a PipeWire main loop. Startup runs the loop for a single
//! roundtrip to prove the daemon connection and enumerate `Audio/Sink`
//! nodes; if that fails (no daemon, no permission) the source stays
//! video-only. Afterwards the loop runs for good: when no sink exists yet
//! the registry listener wakes the loop up (`mainloop.quit()` as a wakeup)
//! to connect the capture stream as soon as one appears. The process
//! callback accumulates F32LE samples and emits one Opus RTP packet per 20
//! ms frame.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use pipewire as pw;
use pw::spa::param::ParamType;
use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};
use pw::spa::pod::serialize::PodSerializer;
use pw::spa::pod::{Object, Pod, Value};
use pw::spa::utils::{Direction, SpaTypes};
use pw::stream::{StreamBox, StreamFlags};
use rtc_rtp::codec::opus::OpusPayloader;
use rtc_rtp::packetizer::{Packetizer, new_packetizer};
use rtc_rtp::sequence::new_random_sequencer;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::WaylandUrlConfig;
use crate::stream::source::MediaPacket;

pub const AUDIO_PAYLOAD_TYPE: u8 = 111;
pub const AUDIO_CLOCK_RATE: u32 = 48_000;
const AUDIO_CHANNELS: u32 = 2;
/// Opus frame size: 20 ms at 48 kHz.
const FRAME_SAMPLES: usize = 960;
/// Largest Opus packet (max duration 120 ms is ~1.5 kB; 20 ms is ~0.5 kB).
const OPUS_OUT_BYTES: usize = 4000;

/// A running audio capture. Dropping it cancels and joins the PipeWire
/// thread (the stream, and with it the source, stops capturing).
pub struct AudioHandle {
    cancel: CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Set once the PipeWire daemon connection succeeded; gates the audio
    /// codec advertisement.
    running: Arc<AtomicBool>,
}

impl AudioHandle {
    pub fn running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("wayland audio: capture thread panicked");
        }
    }
}

/// Start PipeWire capture. Synchronous handshake (5 s cap): daemon connect
/// failures return an error so the caller can degrade to video-only.
pub fn spawn(
    stream_id: &str,
    config: &WaylandUrlConfig,
    rtp_tx: broadcast::Sender<MediaPacket>,
    cancel: CancellationToken,
) -> Result<AudioHandle> {
    let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
    let running = Arc::new(AtomicBool::new(false));
    let thread_cancel = cancel.clone();
    let thread_running = running.clone();
    let thread_config = config.clone();
    let thread_stream_id = stream_id.to_string();
    let thread = std::thread::Builder::new()
        .name(format!("wayland-audio-{stream_id}"))
        .spawn(move || {
            run(
                thread_stream_id,
                thread_config,
                rtp_tx,
                thread_cancel,
                init_tx,
                thread_running,
            )
        })
        .map_err(|e| anyhow::anyhow!("failed to spawn the audio capture thread: {e}"))?;

    match init_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(AudioHandle {
            cancel,
            thread: Some(thread),
            running,
        }),
        Ok(Err(e)) => {
            cancel.cancel();
            // Not joined: a thread stuck in a half-open daemon handshake
            // must not hang the source's stop path; it exits on its own.
            std::mem::forget(thread);
            Err(anyhow::anyhow!("{e}"))
        }
        Err(_) => {
            cancel.cancel();
            std::mem::forget(thread);
            Err(anyhow::anyhow!("PipeWire handshake timed out"))
        }
    }
}

/// Per-stream encoder state, owned by the process listener's user data.
struct AudioState {
    stream_id: String,
    rtp_tx: broadcast::Sender<MediaPacket>,
    packetizer: Box<dyn Packetizer>,
    encoder: opus::Encoder,
    /// Accumulated interleaved F32LE samples; drained in 960*2 chunks.
    pending: std::collections::VecDeque<f32>,
    logged_frame: bool,
}

impl AudioState {
    fn emit_frames(&mut self) {
        while self.pending.len() >= FRAME_SAMPLES * AUDIO_CHANNELS as usize {
            let frame: Vec<f32> = self
                .pending
                .drain(..FRAME_SAMPLES * AUDIO_CHANNELS as usize)
                .collect();
            let mut out = [0u8; OPUS_OUT_BYTES];
            let len = match self.encoder.encode_float(&frame, &mut out) {
                Ok(len) => len,
                Err(e) => {
                    tracing::warn!("[{}] opus encode failed: {e}", self.stream_id);
                    continue;
                }
            };
            if !self.logged_frame {
                tracing::info!(
                    "[{}] wayland audio: first Opus frame encoded",
                    self.stream_id
                );
                self.logged_frame = true;
            }
            match self.packetizer.packetize(
                std::time::Instant::now(),
                &bytes::Bytes::copy_from_slice(&out[..len]),
                FRAME_SAMPLES as u32,
            ) {
                Ok(packets) => {
                    for packet in packets {
                        let _ = self
                            .rtp_tx
                            .send(MediaPacket::RtpAudioPacket(Arc::new(packet)));
                    }
                }
                Err(e) => tracing::warn!("[{}] audio RTP packetize failed: {e}", self.stream_id),
            }
        }
    }
}

fn read_capture_buffer(stream: &pw::stream::Stream, state: &mut AudioState) {
    while let Some(mut buffer) = stream.dequeue_buffer() {
        let datas = buffer.datas_mut();
        let Some(data) = datas.first_mut() else {
            continue;
        };
        let chunk = data.chunk();
        let (offset, size) = (chunk.offset() as usize, chunk.size() as usize);
        let Some(all) = data.data() else {
            continue;
        };
        let end = offset.saturating_add(size).min(all.len());
        if end <= offset {
            continue;
        }
        // F32LE interleaved; the negotiated format is pinned at connect.
        for sample in all[offset..end].as_chunks::<4>().0 {
            state.pending.push_back(f32::from_le_bytes(*sample));
        }
        // Buffer::drop returns it to the stream's pool.
    }
    state.emit_frames();
}

fn run(
    stream_id: String,
    config: WaylandUrlConfig,
    rtp_tx: broadcast::Sender<MediaPacket>,
    cancel: CancellationToken,
    init_tx: std::sync::mpsc::SyncSender<Result<(), String>>,
    running: Arc<AtomicBool>,
) {
    let fail = |msg: String| {
        let _ = init_tx.send(Err(msg));
    };

    let mainloop = match pw::main_loop::MainLoopRc::new(None) {
        Ok(ml) => ml,
        Err(e) => return fail(format!("PipeWire main loop: {e}")),
    };
    let context = match pw::context::ContextBox::new(mainloop.loop_(), None) {
        Ok(c) => c,
        Err(e) => return fail(format!("PipeWire context: {e}")),
    };
    let core = match context.connect(None) {
        Ok(c) => c,
        Err(e) => {
            return fail(format!(
                "no PipeWire daemon at $XDG_RUNTIME_DIR/pipewire-0 ({e})"
            ));
        }
    };

    // Roundtrip: surface fatal connect errors and enumerate sinks.
    let init_error: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let done = Rc::new(Cell::new(false));
    let _core_listener = core
        .add_listener_local()
        .error({
            let init_error = init_error.clone();
            let mainloop = mainloop.clone();
            move |_id, _seq, _res, msg| {
                *init_error.borrow_mut() = Some(msg.to_string());
                mainloop.quit();
            }
        })
        .done({
            let done = done.clone();
            let mainloop = mainloop.clone();
            move |_, _| {
                done.set(true);
                mainloop.quit();
            }
        })
        .register();

    let registry = match core.get_registry() {
        Ok(r) => r,
        Err(e) => return fail(format!("PipeWire registry: {e}")),
    };
    let sink_filter = config.audio_sink.clone();
    let found_sink: Rc<Cell<Option<u32>>> = Rc::new(Cell::new(None));
    let _registry_listener = registry
        .add_listener_local()
        .global({
            let found_sink = found_sink.clone();
            let sink_filter = sink_filter.clone();
            let init_done = done.clone();
            let mainloop = mainloop.clone();
            move |global| {
                if global.type_ != pw::types::ObjectType::Node || found_sink.get().is_some() {
                    return;
                }
                let Some(props) = &global.props else { return };
                if props.get("media.class") != Some("Audio/Sink") {
                    return;
                }
                if let Some(filter) = &sink_filter
                    && !props.get("node.name").is_some_and(|n| n.contains(filter))
                {
                    return;
                }
                found_sink.set(Some(global.id));
                // Past the handshake roundtrip this is the steady-state
                // wakeup: the outer loop reconnects when a sink appears.
                if init_done.get() {
                    mainloop.quit();
                }
            }
        })
        .register();

    if core.sync(0).is_err() {
        return fail("PipeWire sync failed".to_string());
    }
    mainloop.run();

    if let Some(err) = init_error.borrow_mut().take() {
        return fail(format!("PipeWire daemon error: {err}"));
    }
    if init_tx.send(Ok(())).is_err() {
        return; // caller gave up
    }
    running.store(true, Ordering::Relaxed);
    tracing::info!("[{stream_id}] wayland audio: PipeWire connected");

    // Capture stream: F32LE 48 kHz stereo, connected to the sink once one
    // exists (the registry listener above wakes the loop on new sinks).
    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::F32LE);
    audio_info.set_rate(AUDIO_CLOCK_RATE);
    audio_info.set_channels(AUDIO_CHANNELS);
    audio_info.set_position({
        let mut pos = [0u32; 64];
        pos[0] = 3; // FL
        pos[1] = 4; // FR
        pos
    });
    let format_pod = match PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &Value::Object(Object {
            type_: SpaTypes::ObjectParamFormat.as_raw(),
            id: ParamType::EnumFormat.as_raw(),
            properties: audio_info.into(),
        }),
    ) {
        Ok((cursor, _)) => cursor.into_inner(),
        Err(e) => {
            tracing::error!("[{stream_id}] wayland audio: format pod build failed: {e}");
            return;
        }
    };

    let stream = match StreamBox::new(&core, &format!("live777-{stream_id}"), {
        let mut props = pw::properties::PropertiesBox::new();
        props.insert(*pw::keys::MEDIA_TYPE, "Audio");
        props.insert(*pw::keys::MEDIA_CATEGORY, "Capture");
        props.insert(*pw::keys::MEDIA_ROLE, "DSP");
        props
    }) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[{stream_id}] wayland audio: stream create failed: {e}");
            return;
        }
    };

    let ssrc: u32 = rand::random();
    let audio_state = AudioState {
        stream_id: stream_id.clone(),
        rtp_tx,
        packetizer: Box::new(new_packetizer(
            std::time::Instant::now(),
            1400,
            AUDIO_PAYLOAD_TYPE,
            ssrc,
            Box::<OpusPayloader>::default(),
            Box::new(new_random_sequencer()),
            AUDIO_CLOCK_RATE,
        )),
        encoder: match opus::Encoder::new(
            AUDIO_CLOCK_RATE,
            opus::Channels::Stereo,
            opus::Application::Audio,
        ) {
            Ok(mut enc) => {
                if let Err(e) = enc.set_bitrate(opus::Bitrate::Bits(config.audio_bitrate as i32)) {
                    tracing::warn!("[{stream_id}] wayland audio: set_bitrate failed: {e}");
                }
                enc
            }
            Err(e) => {
                tracing::error!("[{stream_id}] wayland audio: opus encoder init failed: {e}");
                return;
            }
        },
        pending: std::collections::VecDeque::with_capacity(FRAME_SAMPLES * 8),
        logged_frame: false,
    };

    let _stream_listener = match stream
        .add_local_listener_with_user_data(audio_state)
        .process(read_capture_buffer)
        .register()
    {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("[{stream_id}] wayland audio: listener register failed: {e}");
            return;
        }
    };

    // Cancel → quit the loop: a pipewire channel is the safe way to wake a
    // foreign main loop. The watcher is a plain std thread — this thread has
    // no tokio runtime.
    let (quit_tx, quit_rx) = pw::channel::channel::<()>();
    let _quit_attach = quit_rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |_| mainloop.quit()
    });
    std::thread::spawn({
        let cancel = cancel.clone();
        move || {
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(25));
            }
            let _ = quit_tx.send(());
        }
    });

    // Steady state: (re)connect whenever a sink is known, then run the loop.
    // The registry listener above quits the loop when a sink appears, which
    // doubles as the wakeup to connect. Cancel also quits, ending the loop.
    let mut connected = false;
    loop {
        if !connected && let Some(id) = found_sink.get() {
            let mut params = [Pod::from_bytes(&format_pod).expect("format pod")];
            match stream.connect(
                Direction::Input,
                Some(id),
                StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
                &mut params,
            ) {
                Ok(()) => {
                    connected = true;
                    tracing::info!("[{stream_id}] wayland audio: capturing sink #{id}");
                }
                Err(e) => {
                    tracing::warn!(
                        "[{stream_id}] wayland audio: connect failed: {e}; retrying on change"
                    );
                    found_sink.set(None);
                }
            }
        }
        if cancel.is_cancelled() {
            break;
        }
        mainloop.run();
        if cancel.is_cancelled() {
            break;
        }
        // Woken by quit(): a sink appeared (loop around to connect) or
        // cancel arrived (break above).
        if !connected && found_sink.get().is_none() {
            // Spurious wakeup; keep waiting.
            continue;
        }
    }

    running.store(false, Ordering::Relaxed);
    tracing::info!("[{stream_id}] wayland audio: capture stopped");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opus encode + packetize: 20 ms of stereo samples become one RTP
    /// packet, advancing the 48 kHz timestamp by 960 per frame.
    #[test]
    fn emit_frames_packetizes_opus_at_20ms_cadence() {
        let (rtp_tx, mut rtp_rx) = broadcast::channel(8);
        let mut state = AudioState {
            stream_id: "t".into(),
            rtp_tx,
            packetizer: Box::new(new_packetizer(
                std::time::Instant::now(),
                1400,
                AUDIO_PAYLOAD_TYPE,
                42,
                Box::<OpusPayloader>::default(),
                Box::new(new_random_sequencer()),
                AUDIO_CLOCK_RATE,
            )),
            encoder: opus::Encoder::new(
                AUDIO_CLOCK_RATE,
                opus::Channels::Stereo,
                opus::Application::Audio,
            )
            .unwrap(),
            pending: Default::default(),
            logged_frame: false,
        };
        // A 440 Hz stereo tone, two and a half frames worth → exactly two
        // packets; the tail stays buffered.
        for i in 0..FRAME_SAMPLES * 5 / 2 {
            let t = i as f32 / AUDIO_CLOCK_RATE as f32;
            let s = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            state.pending.push_back(s);
            state.pending.push_back(s);
        }
        state.emit_frames();

        let mut timestamps = Vec::new();
        while let Ok(MediaPacket::RtpAudioPacket(p)) = rtp_rx.try_recv() {
            assert_eq!(p.header.payload_type, AUDIO_PAYLOAD_TYPE);
            assert_eq!(p.header.ssrc, 42);
            assert!(!p.payload.is_empty());
            timestamps.push(p.header.timestamp);
        }
        assert_eq!(timestamps.len(), 2);
        assert_eq!(timestamps[1].wrapping_sub(timestamps[0]), 960);
    }
}
