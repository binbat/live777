//! End-to-end tests for the Wayland compositor source (`wayland://`).
//!
//! `wayland_source_streams_h264_over_whep`: a provisioned `wayland://`
//! stream auto-starts the compositor + encode pipeline at boot, and a WHEP
//! subscriber receives an H.264 RTP stream whose IDR frames carry in-band
//! SPS/PPS.
//!
//! `client_content_e2e::wayland_client_pixels_reach_the_whep_decoder`
//! (feature `rsmpeg`): a real Wayland client paints a solid-color toplevel;
//! the same WHEP subscriber decodes the switch from the test pattern to the
//! client's pixels, and the empty background after the client disconnects.

#![cfg(all(feature = "source-wayland", target_os = "linux"))]

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Once, OnceLock},
    time::Duration,
};

use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
    RTCConfigurationBuilder, RTCPeerConnectionState, RTCSessionDescription,
};
use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};

mod common;
use common::shutdown_signal;

static TRACING_INIT: Once = Once::new();
/// The wayland source binds its compositor socket under XDG_RUNTIME_DIR;
/// point it at a temp dir for the test (safe: nextest isolates processes).
static RUNTIME_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();

fn init_liveion_test_environment() {
    TRACING_INIT.call_once(|| {
        // These tests run both WebRTC peers locally. Pin ICE candidates to
        // loopback so CI runners cannot choose an unroutable host interface.
        unsafe {
            std::env::set_var("LIVE777_WEBRTC_ICE_UDP_ADDRS", "127.0.0.1:0");
        }

        let dir = tempfile::tempdir().expect("tempdir for XDG_RUNTIME_DIR");
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", dir.path());
        }
        let _ = RUNTIME_DIR.set(dir);

        let filter = std::env::var("RUST_LOG")
            .unwrap_or_else(|_| "live777=info,liveion=info,livetwo=info".to_string());
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_test_writer()
            .try_init();
    });
}

/// One inbound RTP packet of the WHEP video track, reduced to what the
/// tests inspect (the header's marker bit delimits H.264 access units).
struct InboundRtp {
    payload: Vec<u8>,
    /// Only the rsmpeg-gated e2e test depacketizes (and needs the marker).
    #[allow(dead_code)]
    marker: bool,
}

/// WHEP client handler: report connection state, forward inbound RTP
/// to the test.
#[derive(Clone)]
struct SubscriberHandler {
    state_tx: watch::Sender<RTCPeerConnectionState>,
    payload_tx: mpsc::UnboundedSender<InboundRtp>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for SubscriberHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.state_tx.send(state);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let payload_tx = self.payload_tx.clone();
        tokio::spawn(async move {
            while let Some(event) = track.poll().await {
                if let TrackRemoteEvent::OnRtpPacket(packet) = event {
                    let _ = payload_tx.send(InboundRtp {
                        payload: packet.payload.to_vec(),
                        marker: packet.header.marker,
                    });
                }
            }
        });
    }
}

async fn wait_connected(mut state_rx: watch::Receiver<RTCPeerConnectionState>) -> bool {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            state_rx.changed().await.unwrap();
            match *state_rx.borrow() {
                RTCPeerConnectionState::Connected => return true,
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => return false,
                _ => {}
            }
        }
    })
    .await
    .unwrap_or(false)
}

/// Boot in-process liveion with one provisioned stream sourcing from the
/// given `wayland://` URL.
async fn boot_liveion_with_wayland_stream(stream: &str, url: &str) -> SocketAddr {
    let cfg: liveion::config::Config = toml::from_str(&format!(
        r#"
        [stream.{stream}]
        [[stream.{stream}.sources]]
        url = "{url}"
        "#,
    ))
    .unwrap();
    cfg.validate().unwrap();

    let listener = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(liveion::serve(cfg, listener, shutdown_signal()));
    addr
}

/// Start a WHEP subscriber on `stream`; the returned receiver yields the
/// inbound video RTP (payload + marker bit).
async fn start_whep_subscriber(
    addr: SocketAddr,
    stream: &str,
) -> (
    Arc<dyn PeerConnection>,
    watch::Receiver<RTCPeerConnectionState>,
    mpsc::UnboundedReceiver<InboundRtp>,
) {
    let (sub_state_tx, sub_state_rx) = watch::channel(RTCPeerConnectionState::New);
    let (payload_tx, payload_rx) = mpsc::unbounded_channel::<InboundRtp>();
    let sub_handler: Arc<dyn PeerConnectionEventHandler> = Arc::new(SubscriberHandler {
        state_tx: sub_state_tx,
        payload_tx,
    });
    let mut media_engine = MediaEngine::default();
    media_engine.register_default_codecs().unwrap();
    let sub_peer: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::<SocketAddr>::new()
            .with_media_engine(media_engine)
            .with_handler(sub_handler)
            .with_udp_addrs(livetwo::utils::webrtc::ice_udp_addrs())
            .with_configuration(RTCConfigurationBuilder::new().build())
            .build()
            .await
            .unwrap(),
    );
    sub_peer
        .add_transceiver_from_kind(
            RtpCodecKind::Video,
            Some(RTCRtpTransceiverInit {
                direction: RTCRtpTransceiverDirection::Recvonly,
                streams: vec![],
                send_encodings: vec![],
            }),
        )
        .await
        .unwrap();
    let offer = sub_peer.create_offer(None).await.unwrap();
    sub_peer.set_local_description(offer).await.unwrap();
    // Gather fully so the offer carries candidates (no trickle needed).
    tokio::time::sleep(Duration::from_millis(500)).await;
    let offer_sdp = sub_peer.local_description().await.unwrap().sdp;

    let res = reqwest::Client::new()
        .post(format!("http://{addr}{}", api::path::whep(stream)))
        .header(http::header::CONTENT_TYPE, "application/sdp")
        .body(offer_sdp)
        .send()
        .await
        .unwrap();
    let status = res.status();
    let body = res.text().await.unwrap();
    assert_eq!(
        status,
        http::StatusCode::CREATED,
        "WHEP POST failed: {body}"
    );
    sub_peer
        .set_remote_description(RTCSessionDescription::answer(body).unwrap())
        .await
        .unwrap();

    (sub_peer, sub_state_rx, payload_rx)
}

/// H.264 NAL bookkeeping over RTP payloads: single NAL units, FU-A
/// fragments, and STAP-A aggregates (rtc-rtp's H264 payloader packs SPS/PPS
/// into a STAP-A ahead of each IDR).
#[derive(Default)]
struct H264Seen {
    packets: u64,
    sps: bool,
    pps: bool,
    idr: bool,
    /// First-byte histogram (NAL header or FU-A indicator) for debugging.
    types: std::collections::BTreeMap<u8, u64>,
}

impl H264Seen {
    fn observe(&mut self, payload: &[u8]) {
        if payload.len() < 2 {
            return;
        }
        self.packets += 1;
        *self.types.entry(payload[0] & 0x1f).or_default() += 1;
        match payload[0] & 0x1f {
            7 => self.sps = true,
            8 => self.pps = true,
            5 => self.idr = true,
            // STAP-A: one header byte, then u16 length-prefixed NAL units.
            24 => {
                let mut off = 1;
                while off + 2 <= payload.len() {
                    let len = u16::from_be_bytes([payload[off], payload[off + 1]]) as usize;
                    off += 2;
                    if len == 0 || off + len > payload.len() {
                        break;
                    }
                    match payload[off] & 0x1f {
                        7 => self.sps = true,
                        8 => self.pps = true,
                        _ => {}
                    }
                    off += len;
                }
            }
            // FU-A: inner NAL type in the second byte; the start bit marks
            // the fragment carrying the NAL header of an IDR slice.
            28 if payload[1] & 0x80 != 0 && payload[1] & 0x1f == 5 => self.idr = true,
            _ => {}
        }
    }

    fn ready(&self) -> bool {
        self.sps && self.pps && self.idr
    }
}

#[tokio::test]
async fn wayland_source_streams_h264_over_whep() {
    init_liveion_test_environment();

    let addr = boot_liveion_with_wayland_stream(
        "wayland-test",
        "wayland://test-display?width=320&height=240&fps=15&bitrate=500000",
    )
    .await;
    let (sub_peer, sub_state_rx, mut payload_rx) =
        start_whep_subscriber(addr, "wayland-test").await;

    assert!(
        wait_connected(sub_state_rx.clone()).await,
        "WHEP subscriber did not reach Connected; last state: {:?}",
        sub_state_rx.borrow()
    );

    // The startup PLI burst (subscribe.rs) drives an IDR out of the encoder
    // right after attach; with in-band SPS/PPS ahead of every IDR the full
    // config+keyframe set lands well inside this budget.
    let mut seen = H264Seen::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !seen.ready() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let packet = tokio::time::timeout(remaining, payload_rx.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timed out waiting for SPS/PPS + IDR from the wayland source; received {} packets, NAL types: {:?}",
                    seen.packets, seen.types
                )
            })
            .expect("RTP stream ended");
        seen.observe(&packet.payload);
    }

    // Keep receiving: the stream must keep flowing, not just deliver one IDR.
    let before = seen.packets;
    let drain_deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        let remaining = drain_deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, payload_rx.recv()).await {
            Ok(Some(packet)) => seen.observe(&packet.payload),
            _ => break,
        }
    }
    assert!(
        seen.packets > before,
        "no packets after the first IDR ({} total)",
        seen.packets
    );

    sub_peer.close().await.unwrap();
}

/// Full pixel-path verification: a real Wayland client's SHM buffer must
/// survive compositor → OpenH264 → RTP → WHEP → H.264 decode. Gated on
/// `rsmpeg` for the FFmpeg decoder.
#[cfg(feature = "rsmpeg")]
mod client_content_e2e {
    use super::*;
    use std::io::Write as _;
    use std::os::fd::AsFd;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use anyhow::{Context as _, Result, anyhow};
    use rsmpeg::avcodec::{AVCodecContext, AVPacket};
    use rsmpeg::avutil::{AVDictionary, AVFrame};
    use rsmpeg::error::RsmpegError;
    use rsmpeg::ffi;
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{
        wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
    };
    use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
    use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

    /// The client paints solid red, full range RGB.
    const RED: (u8, u8, u8) = (255, 0, 0);

    // ---------------------------------------------------------------------
    // H.264 RTP → Annex-B access units
    // ---------------------------------------------------------------------

    /// Reassembles H.264 RTP payloads (single NAL units, STAP-A aggregates,
    /// FU-A fragments) into Annex-B access units. An AU is complete when the
    /// RTP marker bit arrives.
    #[derive(Default)]
    struct H264Depacketizer {
        /// Annex-B bytes of the access unit being assembled.
        au: Vec<u8>,
        /// In-progress FU-A NAL unit (start code + reconstructed header).
        fu_nal: Vec<u8>,
    }

    impl H264Depacketizer {
        fn push(&mut self, payload: &[u8], marker: bool) -> Option<Vec<u8>> {
            if payload.len() >= 2 {
                match payload[0] & 0x1f {
                    // Single NAL unit.
                    1..=23 => {
                        self.au.extend_from_slice(&[0, 0, 0, 1]);
                        self.au.extend_from_slice(payload);
                    }
                    // STAP-A: one header byte, then u16 length-prefixed NALs.
                    24 => {
                        let mut off = 1;
                        while off + 2 <= payload.len() {
                            let len = u16::from_be_bytes([payload[off], payload[off + 1]]) as usize;
                            off += 2;
                            if len == 0 || off + len > payload.len() {
                                break;
                            }
                            self.au.extend_from_slice(&[0, 0, 0, 1]);
                            self.au.extend_from_slice(&payload[off..off + len]);
                            off += len;
                        }
                    }
                    // FU-A: indicator + header byte; the header carries the
                    // start/end bits and the real NAL type.
                    28 => {
                        let fu_header = payload[1];
                        if fu_header & 0x80 != 0 {
                            self.fu_nal.clear();
                            let nal_header = (payload[0] & 0xe0) | (fu_header & 0x1f);
                            self.fu_nal.extend_from_slice(&[0, 0, 0, 1, nal_header]);
                        }
                        if !self.fu_nal.is_empty() {
                            self.fu_nal.extend_from_slice(&payload[2..]);
                            if fu_header & 0x40 != 0 {
                                self.au.append(&mut self.fu_nal);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if marker && !self.au.is_empty() {
                Some(std::mem::take(&mut self.au))
            } else {
                None
            }
        }
    }

    // ---------------------------------------------------------------------
    // FFmpeg H.264 decode
    // ---------------------------------------------------------------------

    struct H264Decoder {
        ctx: AVCodecContext,
    }

    impl H264Decoder {
        fn new() -> Result<Self> {
            let codec = rsmpeg::avcodec::AVCodec::find_decoder(ffi::AV_CODEC_ID_H264)
                .ok_or_else(|| anyhow!("FFmpeg H.264 decoder not found"))?;
            let mut ctx = AVCodecContext::new(&codec);
            // Single-threaded: no frame-threading delay line, so the last
            // frame fed (the background canvas after the client disconnects)
            // comes out without needing a decoder flush.
            let opts = AVDictionary::new(c"threads", c"1", 0);
            ctx.open(Some(opts))
                .context("failed to open the H.264 decoder")?;
            Ok(Self { ctx })
        }

        /// Feed one Annex-B access unit; returns every frame it produces.
        /// Malformed/unreferenced AUs (e.g. P-frames ahead of the first
        /// IDR) are logged and skipped, never fatal.
        fn decode_au(&mut self, au: &[u8]) -> Vec<DecodedFrame> {
            let mut frames = Vec::new();
            let packet = match av_packet_from_bytes(au) {
                Ok(packet) => packet,
                Err(e) => {
                    tracing::warn!("failed to wrap an access unit: {e}");
                    return frames;
                }
            };
            if let Err(e) = self.ctx.send_packet(Some(&packet)) {
                tracing::warn!("decoder rejected an access unit: {e}");
                return frames;
            }
            loop {
                match self.ctx.receive_frame() {
                    Ok(frame) => {
                        if let Some(frame) = DecodedFrame::from_avframe(&frame) {
                            frames.push(frame);
                        }
                    }
                    Err(RsmpegError::DecoderDrainError | RsmpegError::DecoderFlushedError) => break,
                    Err(e) => {
                        tracing::warn!("decoder frame error: {e}");
                        break;
                    }
                }
            }
            frames
        }
    }

    /// Create an AVPacket that owns a copy of the provided bytes (mirrors
    /// livetwo's probe decoder).
    fn av_packet_from_bytes(data: &[u8]) -> Result<AVPacket> {
        let len_i32 = i32::try_from(data.len())
            .map_err(|_| anyhow!("access unit too large for AVPacket: {} bytes", data.len()))?;
        let mut packet = AVPacket::new();
        // SAFETY: av_new_packet allocates a fresh buffer of len_i32 bytes and
        // makes packet own it. The pointer is valid until packet is dropped.
        let ret = unsafe { ffi::av_new_packet(packet.as_mut_ptr(), len_i32) };
        if ret < 0 {
            return Err(anyhow!("av_new_packet failed with {ret}"));
        }
        // SAFETY: av_new_packet succeeded and should guarantee a valid buffer.
        // We still defensively check for a null data pointer before copying.
        unsafe {
            let pkt = packet.as_mut_ptr();
            let dst = (*pkt).data;
            if dst.is_null() {
                return Err(anyhow!(
                    "av_new_packet returned null data pointer for {} bytes",
                    data.len()
                ));
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        Ok(packet)
    }

    // ---------------------------------------------------------------------
    // Decoded frames: RGB conversion, color verdicts, review artifact
    // ---------------------------------------------------------------------

    struct DecodedFrame {
        width: usize,
        height: usize,
        /// RGB24 conversion of the decoded YUV (BT.601 limited).
        rgb: Vec<u8>,
        /// Average (r, g, b) over the whole frame.
        avg: (f64, f64, f64),
    }

    impl DecodedFrame {
        fn from_avframe(frame: &AVFrame) -> Option<Self> {
            if frame.format != ffi::AV_PIX_FMT_YUV420P {
                tracing::warn!("unexpected decoded pixel format {}", frame.format);
                return None;
            }
            let (width, height) = (frame.width as usize, frame.height as usize);
            let rgb = yuv420p_to_rgb24(frame, width, height);
            let avg = average_rgb(&rgb);
            Some(Self {
                width,
                height,
                rgb,
                avg,
            })
        }

        /// Solid red, with slack for BT.601 limited range and H.264
        /// quantization ((255,0,0) lands at Y≈81/U≈90/V≈240).
        fn is_red(&self) -> bool {
            self.avg.0 >= 180.0 && self.avg.1 <= 75.0 && self.avg.2 <= 75.0
        }

        /// The empty canvas (black), same slack.
        fn is_black(&self) -> bool {
            self.avg.0 <= 45.0 && self.avg.1 <= 45.0 && self.avg.2 <= 45.0
        }
    }

    fn plane_row(frame: &AVFrame, plane: usize, row: usize, len: usize) -> &[u8] {
        // SAFETY: for AV_PIX_FMT_YUV420P planes 0..=2 are valid allocations
        // of linesize * plane_height bytes owned by `frame`, which outlives
        // the returned slice. Decoder output linesizes are positive.
        unsafe {
            std::slice::from_raw_parts(
                frame.data[plane].add(row * frame.linesize[plane] as usize),
                len,
            )
        }
    }

    /// BT.601 limited-range YUV420P → RGB24 (the inverse of the source's
    /// conversion; test tooling at 320x240, no SIMD needed).
    fn yuv420p_to_rgb24(frame: &AVFrame, width: usize, height: usize) -> Vec<u8> {
        let mut rgb = vec![0u8; width * height * 3];
        for (row, out_row) in rgb.chunks_exact_mut(width * 3).enumerate() {
            let ys = plane_row(frame, 0, row, width);
            let us = plane_row(frame, 1, row / 2, width / 2);
            let vs = plane_row(frame, 2, row / 2, width / 2);
            for (col, px) in out_row.chunks_exact_mut(3).enumerate() {
                let c = i32::from(ys[col]) - 16;
                let d = i32::from(us[col / 2]) - 128;
                let e = i32::from(vs[col / 2]) - 128;
                px[0] = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
                px[1] = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
                px[2] = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
            }
        }
        rgb
    }

    fn average_rgb(rgb: &[u8]) -> (f64, f64, f64) {
        let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
        for px in rgb.chunks_exact(3) {
            r += u64::from(px[0]);
            g += u64::from(px[1]);
            b += u64::from(px[2]);
        }
        let n = (rgb.len() / 3) as f64;
        (r as f64 / n, g as f64 / n, b as f64 / n)
    }

    /// Dump a decoded frame as binary PPM for human review.
    fn write_ppm(frame: &DecodedFrame, path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut out = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
        out.extend_from_slice(&frame.rgb);
        std::fs::write(path, &out).unwrap();
    }

    // ---------------------------------------------------------------------
    // The Wayland client (own thread: wayland-client is blocking)
    // ---------------------------------------------------------------------

    #[derive(Default)]
    struct ClientState {
        configured: bool,
        surface: Option<wl_surface::WlSurface>,
        buffer: Option<wl_buffer::WlBuffer>,
    }

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ClientState {
        fn event(
            _state: &mut Self,
            _registry: &wl_registry::WlRegistry,
            _event: wl_registry::Event,
            _data: &GlobalListContents,
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<xdg_surface::XdgSurface, ()> for ClientState {
        fn event(
            state: &mut Self,
            xdg: &xdg_surface::XdgSurface,
            event: xdg_surface::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if let xdg_surface::Event::Configure { serial } = event {
                xdg.ack_configure(serial);
                if let (Some(surface), Some(buffer)) = (&state.surface, &state.buffer) {
                    surface.attach(Some(buffer), 0, 0);
                    surface.commit();
                }
                state.configured = true;
            }
        }
    }

    delegate_noop!(ClientState: wl_compositor::WlCompositor);
    delegate_noop!(ClientState: wl_shm_pool::WlShmPool);
    // These interfaces deliver events (formats, release, configure, ...):
    // consume and ignore them.
    delegate_noop!(ClientState: ignore wl_shm::WlShm);
    delegate_noop!(ClientState: ignore wl_buffer::WlBuffer);
    delegate_noop!(ClientState: ignore wl_surface::WlSurface);
    delegate_noop!(ClientState: ignore xdg_wm_base::XdgWmBase);
    delegate_noop!(ClientState: ignore xdg_toplevel::XdgToplevel);

    /// A Wayland client holding one xdg toplevel filled with solid red.
    /// Re-commits at ~10 Hz so the compositor keeps emitting frames like a
    /// live GUI app (it only produces frames on commits). [`shutdown`]
    /// disconnects the client.
    struct RedClient {
        stop_flag: Arc<AtomicBool>,
        committed: std::sync::mpsc::Receiver<()>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl RedClient {
        fn spawn(width: i32, height: i32) -> Self {
            let stop_flag = Arc::new(AtomicBool::new(false));
            let (committed_tx, committed) = std::sync::mpsc::channel();
            let thread_stop = stop_flag.clone();
            let thread = std::thread::Builder::new()
                .name("wayland-red-client".into())
                .spawn(move || run_client(width, height, thread_stop, committed_tx))
                .expect("spawn the Wayland client thread");
            Self {
                stop_flag,
                committed,
                thread: Some(thread),
            }
        }

        /// Block until the client's buffer commit reached the compositor.
        fn wait_committed(&self, timeout: Duration) {
            self.committed
                .recv_timeout(timeout)
                .expect("the Wayland client never committed its buffer");
        }

        /// Disconnect the client (drop its connection) and join the thread.
        async fn shutdown(mut self) {
            self.stop_flag.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                tokio::task::spawn_blocking(move || thread.join())
                    .await
                    .expect("join the Wayland client thread")
                    .expect("the Wayland client thread panicked");
            }
        }
    }

    fn run_client(
        width: i32,
        height: i32,
        stop: Arc<AtomicBool>,
        committed: std::sync::mpsc::Sender<()>,
    ) {
        let conn = Connection::connect_to_env().expect("connect to the compositor");
        let (globals, mut queue) =
            registry_queue_init::<ClientState>(&conn).expect("registry init");
        let qh = queue.handle();

        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=5, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();

        let surface = compositor.create_surface(&qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_title("live777-e2e".into());

        // Solid red XRGB8888 (LE byte order B,G,R,X).
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        for px in pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&[RED.2, RED.1, RED.0, 255]);
        }
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&pixels).unwrap();
        let pool = shm.create_pool(file.as_fd(), width * height * 4, &qh, ());
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            width * 4,
            wl_shm::Format::Xrgb8888,
            &qh,
            (),
        );

        let mut state = ClientState {
            configured: false,
            surface: Some(surface),
            buffer: Some(buffer),
        };
        // Initial commit (no buffer), then wait for the configure: the
        // handler acks, attaches the buffer and commits again.
        state.surface.as_ref().unwrap().commit();
        while !state.configured {
            queue.blocking_dispatch(&mut state).unwrap();
        }
        // blocking_dispatch returns without flushing when it dispatched
        // events; the handler's attach+commit would otherwise sit in the
        // write buffer forever.
        conn.flush().unwrap();
        let _ = committed.send(());

        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            state.surface.as_ref().unwrap().commit();
            if let Err(e) = conn.flush() {
                tracing::warn!("wayland client flush failed: {e}");
                break;
            }
        }
        // Dropping the connection disconnects the client; the compositor
        // composites the empty canvas from then on.
    }

    // ---------------------------------------------------------------------
    // The test
    // ---------------------------------------------------------------------

    /// Receive → depacketize → decode until `pred` matches a decoded frame.
    async fn decode_until(
        payload_rx: &mut mpsc::UnboundedReceiver<InboundRtp>,
        depacker: &mut H264Depacketizer,
        decoder: &mut H264Decoder,
        packets: &mut u64,
        deadline: Instant,
        what: &str,
        mut pred: impl FnMut(&DecodedFrame) -> bool,
    ) -> DecodedFrame {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for {what} ({packets} RTP packets seen)"
            );
            let packet = tokio::time::timeout(remaining, payload_rx.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!("timed out waiting for {what} ({packets} RTP packets seen)")
                })
                .expect("the RTP stream ended");
            *packets += 1;
            let Some(au) = depacker.push(&packet.payload, packet.marker) else {
                continue;
            };
            for frame in decoder.decode_au(&au) {
                if pred(&frame) {
                    return frame;
                }
            }
        }
    }

    #[tokio::test]
    async fn wayland_client_pixels_reach_the_whep_decoder() {
        init_liveion_test_environment();

        let stream = "wayland-e2e";
        let socket_name = "e2e-display";
        let (width, height) = (320u32, 240u32);
        let addr = boot_liveion_with_wayland_stream(
            stream,
            &format!("wayland://{socket_name}?width={width}&height={height}&fps=15&bitrate=500000"),
        )
        .await;
        let (sub_peer, sub_state_rx, mut payload_rx) = start_whep_subscriber(addr, stream).await;
        assert!(
            wait_connected(sub_state_rx.clone()).await,
            "WHEP subscriber did not reach Connected; last state: {:?}",
            sub_state_rx.borrow()
        );

        let mut depacker = H264Depacketizer::default();
        let mut decoder = H264Decoder::new().expect("an FFmpeg H.264 decoder");
        let mut packets = 0u64;

        // Phase 1: the synthetic test pattern flows and decodes before any
        // client connects.
        let mut decoded = 0u64;
        decode_until(
            &mut payload_rx,
            &mut depacker,
            &mut decoder,
            &mut packets,
            Instant::now() + Duration::from_secs(30),
            "decoded test-pattern frames",
            |_| {
                decoded += 1;
                decoded >= 3
            },
        )
        .await;

        // Phase 2: a Wayland client paints solid red; the same subscriber
        // must keep decoding and see the client's pixels — no reconnect,
        // no renegotiation. Safe here: nextest isolates test processes.
        unsafe { std::env::set_var("WAYLAND_DISPLAY", socket_name) };
        let client = RedClient::spawn(width as i32, height as i32);
        client.wait_committed(Duration::from_secs(10));
        let packets_at_commit = packets;

        let red = decode_until(
            &mut payload_rx,
            &mut depacker,
            &mut decoder,
            &mut packets,
            Instant::now() + Duration::from_secs(30),
            "a decoded red client frame",
            DecodedFrame::is_red,
        )
        .await;
        assert!(
            packets > packets_at_commit,
            "the RTP flow stalled across the pattern→client switch"
        );
        let artifact =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/tmp/wayland_e2e_red_frame.ppm");
        write_ppm(&red, &artifact);
        println!("decoded red frame written to {}", artifact.display());

        // Phase 3: the client disconnects; the compositor keeps streaming
        // the now-empty canvas and the decoder sees the black background,
        // still on the same WHEP session.
        client.shutdown().await;
        decode_until(
            &mut payload_rx,
            &mut depacker,
            &mut decoder,
            &mut packets,
            Instant::now() + Duration::from_secs(30),
            "a decoded background frame after the client disconnect",
            DecodedFrame::is_black,
        )
        .await;

        sub_peer.close().await.unwrap();
        // nextest runs each test of this binary in its own process, so
        // removing the shared runtime dir here cannot race another test.
        if let Some(dir) = RUNTIME_DIR.get() {
            let _ = std::fs::remove_dir_all(dir.path());
        }
    }
}
