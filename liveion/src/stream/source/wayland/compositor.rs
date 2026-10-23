//! Headless Wayland compositor (smithay, no GPU output) producing
//! [`ArgbFrame`]s.
//!
//! Runs on a dedicated thread with a calloop event loop; [`spawn`] binds the
//! Wayland socket synchronously so [`WaylandSource`](super::WaylandSource)'s
//! `start()` fails cleanly on setup errors. Visible windows' current buffers
//! are composited onto a fixed-size canvas in stacking order; frames are
//! produced on a timer only when a commit happened since the last emitted
//! frame. Windows are xdg toplevels (native Wayland clients) or, when
//! XWayland is enabled and available, X11 windows managed by the built-in
//! XWM — X11 buffers flow through the same SHM/dmabuf paths. Compositing
//! uses the GL path ([`gl`]) when a GPU/EGL renderer is available
//! (advertising `zwp_linux_dmabuf_v1` so GL clients can submit dmabuf
//! buffers), and falls back to a CPU blit of SHM buffers otherwise.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use smithay::backend::allocator::Buffer as _;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::input::{Axis, AxisSource, ButtonState, KeyState};
use smithay::input::keyboard::{FilterResult, Keycode};
use smithay::input::pointer::{AxisFrame, ButtonEvent, MotionEvent};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, Mode as LoopMode, PostAction, channel};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_output, wl_shm};
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle};
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, Damage,
    SurfaceAttributes, with_states,
};
use smithay::wayland::dmabuf::{
    DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier, get_dmabuf,
};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
};
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, SurfaceCachedState, ToplevelSurface, XdgShellHandler,
    XdgShellState,
};
use smithay::wayland::shm::{BufferData, ShmHandler, ShmState, with_buffer_contents};
use smithay::wayland::socket::ListeningSocketSource;
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge, WmWindowProperty, XwmId};
use smithay::xwayland::{
    X11Surface, X11Wm, XWayland, XWaylandClientData, XWaylandEvent, XwmHandler,
};
use smithay::{
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_output, delegate_seat,
    delegate_shm, delegate_xdg_shell, delegate_xwayland_shell,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::gl::{DrawItem, GlPipeline};
use super::input::InputEvent;
use super::{ArgbFrame, WaylandUrlConfig};

/// How long the compositor may stay silent once it has taken over from the
/// test pattern before it repaints the (possibly unchanged) canvas anyway,
/// giving the encoder a GOP boundary for late-joining subscribers.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(2);

/// A running headless compositor. The thread exits when `cancel` is
/// cancelled; dropping the [`CompositorHandle`] also cancels (so the thread
/// can never outlive its owner), and [`join`](Self::join) awaits it.
pub struct CompositorHandle {
    socket_name: String,
    thread: Option<std::thread::JoinHandle<()>>,
    cancel: CancellationToken,
    /// Set by the event-loop thread once GL compositing is up (dmabuf
    /// advertised); false when running the CPU blit fallback.
    /// Read by the compositor integration tests.
    #[allow(dead_code)]
    gl_active: Arc<AtomicBool>,
    /// The X11 display number once XWayland is ready (`DISPLAY=:N` for X11
    /// clients); `None` while XWayland is starting or when it is
    /// disabled/unavailable.
    /// Read by the compositor integration tests.
    #[allow(dead_code)]
    x11_display: Arc<Mutex<Option<u32>>>,
    /// Remote-input sink (`input=1`): parsed events from subscriber
    /// datachannels are injected into the seat by the event loop.
    input_tx: Option<channel::Sender<InputEvent>>,
}

impl CompositorHandle {
    /// Wayland display name clients should use (`WAYLAND_DISPLAY`).
    pub fn socket_name(&self) -> &str {
        &self.socket_name
    }

    /// Whether the compositor initialized GL compositing (dmabuf support).
    /// Read by the compositor integration tests.
    #[allow(dead_code)]
    pub fn gl_active(&self) -> bool {
        self.gl_active.load(Ordering::Relaxed)
    }

    /// X11 display number for X11 clients (`DISPLAY=:N`), once XWayland is
    /// up. `None` when X11 support is off or XWayland is not ready (yet).
    /// Read by the compositor integration tests.
    #[allow(dead_code)]
    pub fn x11_display(&self) -> Option<u32> {
        *self.x11_display.lock().unwrap()
    }

    /// Remote-input event sink; `Some` only when the source has `input=1`.
    pub fn input_sender(&self) -> Option<channel::Sender<InputEvent>> {
        self.input_tx.clone()
    }

    /// Await the compositor thread's exit.
    pub async fn join(mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = tokio::task::spawn_blocking(move || thread.join()).await;
        }
    }
}

impl Drop for CompositorHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Start the compositor: bind `config.socket_name` under `$XDG_RUNTIME_DIR`
/// and spawn the event-loop thread. Socket bind / display setup failures are
/// returned synchronously so the source's `start()` fails with a clear error.
pub fn spawn(
    config: &WaylandUrlConfig,
    frame_tx: mpsc::Sender<ArgbFrame>,
    cancel: CancellationToken,
    repaint_request: Arc<AtomicBool>,
) -> Result<CompositorHandle> {
    // Pre-check for an actionable error; smithay's bind would surface the
    // same condition as a bare RuntimeDirNotSet.
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute());
    if runtime_dir.is_none() {
        anyhow::bail!(
            "XDG_RUNTIME_DIR is not set to an absolute path; the Wayland compositor needs it \
             to create the '{}' socket (e.g. export XDG_RUNTIME_DIR=/run/user/$(id -u))",
            config.socket_name
        );
    }

    let display: Display<WaylandState> =
        Display::new().map_err(|e| anyhow::anyhow!("failed to create Wayland display: {e}"))?;
    let dh = display.handle();
    let state = WaylandState::new(&dh, config, frame_tx, repaint_request);
    let x11_display = state.x11_display.clone();

    let listener = ListeningSocketSource::with_name(&config.socket_name).map_err(|e| {
        anyhow::anyhow!(
            "failed to bind Wayland socket '{}' in {}: {e}",
            config.socket_name,
            runtime_dir.unwrap().display()
        )
    })?;

    let period = Duration::from_secs_f64(1.0 / f64::from(config.fps));
    let socket_name = config.socket_name.clone();
    let thread_cancel = cancel.clone();
    let gl_active = Arc::new(AtomicBool::new(false));
    let thread_gl_active = gl_active.clone();
    let (input_tx, input_rx) = match config.input {
        true => {
            let (tx, rx) = channel::channel();
            (Some(tx), Some(rx))
        }
        false => (None, None),
    };
    let thread = std::thread::Builder::new()
        .name(format!("wayland-comp-{}", config.socket_name))
        .spawn(move || {
            run(
                display,
                state,
                listener,
                period,
                thread_cancel,
                thread_gl_active,
                input_rx,
            )
        })
        .map_err(|e| anyhow::anyhow!("failed to spawn compositor thread: {e}"))?;

    tracing::info!(
        "wayland compositor listening on WAYLAND_DISPLAY={}",
        socket_name
    );

    Ok(CompositorHandle {
        socket_name,
        thread: Some(thread),
        cancel,
        gl_active,
        x11_display,
        input_tx,
    })
}

/// The compositor thread: build the calloop loop, wire sources, run until
/// cancelled. Everything is dropped here, so the socket file is removed by
/// the `ListeningSocket`'s `Drop` before the thread exits.
fn run(
    display: Display<WaylandState>,
    mut state: WaylandState,
    listener: ListeningSocketSource,
    period: Duration,
    cancel: CancellationToken,
    gl_active: Arc<AtomicBool>,
    input_rx: Option<channel::Channel<InputEvent>>,
) {
    let socket_name = state.socket_name.clone();
    let mut event_loop: EventLoop<WaylandState> = match EventLoop::try_new() {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("wayland compositor '{socket_name}': failed to create event loop: {e}");
            return;
        }
    };
    let loop_signal = event_loop.get_signal();
    let loop_handle = event_loop.handle();

    let listener_token = match loop_handle.insert_source(listener, |stream, _, state| {
        if let Err(e) = state
            .display_handle
            .insert_client(stream, Arc::new(ClientState::default()))
        {
            tracing::warn!("wayland compositor: failed to insert client: {e}");
        }
    }) {
        Ok(token) => token,
        Err(e) => {
            tracing::error!("wayland compositor '{socket_name}': failed to add socket source: {e}");
            return;
        }
    };

    let display_token = match loop_handle.insert_source(
        Generic::new(display, Interest::READ, LoopMode::Level),
        |_, display, state| {
            // Safety: the display lives inside the event loop source and is
            // dropped with it, never through this reference.
            let display = unsafe { display.get_mut() };
            if let Err(e) = display.dispatch_clients(state) {
                tracing::warn!("wayland compositor: client dispatch error: {e}");
            }
            if let Err(e) = state.display_handle.flush_clients() {
                tracing::debug!("wayland compositor: client flush error: {e}");
            }
            Ok(PostAction::Continue)
        },
    ) {
        Ok(token) => token,
        Err(e) => {
            tracing::error!(
                "wayland compositor '{socket_name}': failed to add display source: {e}"
            );
            return;
        }
    };

    // GL compositing (GBM + EGL + GLES): enables dmabuf clients. Must happen
    // on this thread (the renderer is !Send and the GL context stays current
    // here); any failure keeps the CPU path. The dmabuf global is created
    // before the loop starts, so every client sees it in its first registry
    // roundtrip.
    let mut gl = GlPipeline::init(&socket_name, state.width, state.height);
    if let Some(pipeline) = &gl {
        let formats = pipeline.import_formats();
        match DmabufFeedbackBuilder::new(pipeline.drm_node().dev_id(), formats.clone()).build() {
            Ok(feedback) => {
                let global = state
                    .dmabuf_state
                    .create_global_with_default_feedback::<WaylandState>(
                        &state.display_handle,
                        &feedback,
                    );
                state.dmabuf_formats = Some(formats);
                state.dmabuf_global = Some(global);
                tracing::info!(
                    "wayland compositor '{socket_name}': advertising zwp_linux_dmabuf_v1"
                );
            }
            Err(e) => {
                tracing::warn!(
                    "wayland compositor '{socket_name}': dmabuf feedback build failed ({e}); not advertising dmabuf"
                );
            }
        }
    }
    gl_active.store(gl.is_some(), Ordering::Relaxed);

    // X11 support: spawn XWayland as a client of this compositor. Optional —
    // a missing `Xwayland` binary or a failed startup only disables X11
    // clients, the Wayland side keeps running. X11 window frames ride the
    // same buffer paths (SHM / dmabuf) as native clients, so nothing
    // downstream of the window list knows the difference.
    let mut xwayland_token = None;
    if state.xwayland_enabled {
        // The filesystem socket binds under /tmp/.X11-unix; the directory is
        // not guaranteed to exist on servers/containers.
        let _ = std::fs::create_dir_all("/tmp/.X11-unix");
        match XWayland::spawn(
            &state.display_handle,
            state.xdisplay,
            std::iter::empty::<(std::ffi::OsString, std::ffi::OsString)>(),
            true,
            std::process::Stdio::null(),
            if std::env::var_os("LIVE777_WAYLAND_XWAYLAND_LOG").is_some() {
                std::process::Stdio::inherit()
            } else {
                std::process::Stdio::null()
            },
            |_| {},
        ) {
            Ok((xwayland, xwayland_client)) => {
                // The closure's captured LoopHandle (needed by start_wm when
                // XWayland reports ready) forms an Rc cycle with the calloop
                // registry — same as smithay's own X11Source — so the
                // registry skeleton is leaked on shutdown; everything user-
                // visible (socket file, display backend, the Xwayland
                // process) is reclaimed by the explicit source removals at
                // the end of this function.
                let wm_loop_handle = loop_handle.clone();
                match loop_handle.insert_source(xwayland, move |event, _, state| {
                    match event {
                        XWaylandEvent::Ready {
                            x11_socket,
                            display_number,
                        } => {
                            match X11Wm::start_wm(
                                wm_loop_handle.clone(),
                                x11_socket,
                                xwayland_client.clone(),
                            ) {
                                Ok(wm) => {
                                    tracing::info!(
                                        "wayland compositor '{}': XWayland ready, X11 clients use DISPLAY=:{}",
                                        state.socket_name,
                                        display_number
                                    );
                                    *state.x11_display.lock().unwrap() = Some(display_number);
                                    state.xwm = Some(wm);
                                }
                                Err(e) => {
                                    tracing::error!(
                                        "wayland compositor '{}': failed to start the X11 window manager: {e}",
                                        state.socket_name
                                    );
                                }
                            }
                        }
                        XWaylandEvent::Error => {
                            tracing::warn!(
                                "wayland compositor '{}': XWayland failed during startup; X11 clients unsupported",
                                state.socket_name
                            );
                        }
                    }
                }) {
                    Ok(token) => xwayland_token = Some(token),
                    Err(e) => {
                        tracing::warn!(
                            "wayland compositor '{socket_name}': failed to add the XWayland source: {e}; X11 clients unsupported"
                        );
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "wayland compositor '{socket_name}': XWayland unavailable ({e}); X11 clients unsupported"
                );
            }
        }
    }

    // Remote input (input=1): parsed datachannel events injected into the
    // seat, dispatched immediately (not on the frame tick — 33 ms of input
    // latency would be noticeable).
    let input_token = input_rx.and_then(|rx| {
        match loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(event) = event {
                state.handle_input(event);
            }
        }) {
            Ok(token) => Some(token),
            Err(e) => {
                tracing::error!(
                    "wayland compositor '{socket_name}': failed to add input source: {e}"
                );
                None
            }
        }
    });

    let timer_token = match loop_handle.insert_source(Timer::immediate(), move |_, _, state| {
        if cancel.is_cancelled() {
            loop_signal.stop();
            return TimeoutAction::Drop;
        }
        if state.repaint_request.swap(false, Ordering::Relaxed) {
            state.dirty = true;
            state.force_keyframe_next = true;
        }
        if state.started && state.last_emit.elapsed() >= KEEPALIVE_INTERVAL {
            state.dirty = true;
            state.force_keyframe_next = true;
        }
        state.produce_frame(gl.as_mut());
        if let Err(e) = state.display_handle.flush_clients() {
            tracing::debug!("wayland compositor: client flush error: {e}");
        }
        TimeoutAction::ToDuration(period)
    }) {
        Ok(token) => token,
        Err(e) => {
            tracing::error!("wayland compositor '{socket_name}': failed to add frame timer: {e}");
            return;
        }
    };

    tracing::info!("wayland compositor '{socket_name}': event loop started");
    if let Err(e) = event_loop.run(None, &mut state, |_| {}) {
        tracing::error!("wayland compositor '{socket_name}': event loop error: {e}");
    }
    tracing::info!("wayland compositor '{socket_name}': event loop exited");

    // Shutdown ritual. smithay's X11Source closure (and our XWayland one)
    // captures a LoopHandle, so once an XWM ran, the calloop registry sits in
    // an Rc cycle and dropping the event loop alone would leak it — with the
    // display backend, the listening socket (and its on-disk file), and the
    // XWayland handle (whose Drop kills the Xwayland client) inside. Remove
    // every source we inserted so their drops run here, deterministically;
    // dropping the XWM closes the X11 connection, which makes Xwayland exit.
    // The X11 source is smithay-internal and unreachable, so one registry
    // skeleton (an epoll fd plus a channel) stays leaked per compositor
    // lifetime — a documented smithay-0.7 limitation, harmless for the
    // process as a whole as long as sources are not restarted at a high rate.
    state.xwm = None;
    if let Some(token) = xwayland_token {
        loop_handle.remove(token);
    }
    if let Some(token) = input_token {
        loop_handle.remove(token);
    }
    loop_handle.remove(timer_token);
    loop_handle.remove(display_token);
    loop_handle.remove(listener_token);
    drop(loop_handle);
}

// ---------------------------------------------------------------------------
// Compositor state
// ---------------------------------------------------------------------------

struct WaylandState {
    display_handle: DisplayHandle,
    socket_name: String,
    width: u32,
    height: u32,
    frame_tx: mpsc::Sender<ArgbFrame>,
    origin: Instant,
    /// A commit/toplevel change happened since the last emitted frame.
    dirty: bool,
    /// A subscriber asked for a keyframe (via the source's RTCP task): the
    /// next timer tick must repaint and re-emit even without any commit,
    /// or a static desktop would never deliver the requested IDR.
    repaint_request: Arc<AtomicBool>,
    /// Last successful emit; drives the idle keepalive (one repaint every
    /// few seconds) so late-joining subscribers always get a keyframe to
    /// sync on even when the desktop is static.
    last_emit: Instant,
    /// The next emitted frame must be an IDR (set by the timer on repaint
    /// requests and keepalives, moved onto the frame at emit).
    force_keyframe_next: bool,
    /// The first frame with actual client content was emitted; before that,
    /// the test pattern stays on (handled by the source's frame mux).
    started: bool,
    /// xdg toplevels, bottom-to-top stacking order (creation order).
    toplevels: Vec<ToplevelSurface>,
    /// Managed X11 windows, bottom-to-top stacking order. Painted above xdg
    /// toplevels (mixed desktops are rare; X11 override-redirect windows —
    /// menus, tooltips — always paint on top).
    x11_windows: Vec<X11Surface>,
    /// Override-redirect X11 windows (unmanaged; menus/tooltips), mapped
    /// order. Painted above everything else.
    x11_override_redirect: Vec<X11Surface>,
    /// Whether to spawn XWayland (`wayland://...?xwayland=`).
    xwayland_enabled: bool,
    /// Pinned X11 display number, if requested (`xdisplay=`).
    xdisplay: Option<u32>,
    /// The running X11 window manager; set when XWayland reports ready.
    xwm: Option<X11Wm>,
    /// The ready XWayland display number, shared with `CompositorHandle`.
    x11_display: Arc<Mutex<Option<u32>>>,
    logged_unsupported_scale: bool,
    logged_unsupported_transform: bool,
    logged_unsupported_format: bool,
    logged_gl_import_failure: bool,
    /// A toplevel or wl_buffer died: drop the renderer's dmabuf texture
    /// cache at the next frame so the underlying buffers are freed.
    gl_cache_cleanup: bool,

    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    xwayland_shell_state: XWaylandShellState,
    shm_state: ShmState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,
    dmabuf_state: DmabufState,
    /// Alive only while GL compositing is up; created on the event-loop
    /// thread once the renderer initializes.
    dmabuf_global: Option<DmabufGlobal>,
    /// Advertised dmabuf formats, for the `dmabuf_imported` sanity filter
    /// (the renderer itself lives in the frame timer and is unreachable
    /// from protocol dispatch).
    dmabuf_formats: Option<FormatSet>,
    // Kept alive for their globals/objects; not otherwise read.
    _output_manager_state: OutputManagerState,
    /// The seat is read for remote input injection (`handle_input`).
    seat: Seat<Self>,
    _output: Output,
    /// Last known pointer position in canvas coordinates (button events
    /// reference it; browsers send moves and clicks as separate messages).
    pointer_pos: Point<f64, Logical>,
}

impl WaylandState {
    fn new(
        dh: &DisplayHandle,
        config: &WaylandUrlConfig,
        frame_tx: mpsc::Sender<ArgbFrame>,
        repaint_request: Arc<AtomicBool>,
    ) -> Self {
        let compositor_state = CompositorState::new::<Self>(dh);
        let xdg_shell_state = XdgShellState::new::<Self>(dh);
        // The xwayland-shell global is only visible to the XWayland client,
        // so creating it unconditionally is invisible to normal clients.
        let xwayland_shell_state = XWaylandShellState::new::<Self>(dh);
        let shm_state =
            ShmState::new::<Self>(dh, vec![wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(dh);
        let data_device_state = DataDeviceState::new::<Self>(dh);

        let mut seat_state = SeatState::new();
        let mut seat: Seat<Self> = seat_state.new_wl_seat(dh, "seat0");
        if let Err(e) = seat.add_keyboard(Default::default(), 200, 25) {
            tracing::warn!(
                "wayland compositor: no keyboard keymap available ({e}); the seat will have no keyboard capability"
            );
        }
        seat.add_pointer();

        let output = Output::new(
            "live777-0".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "live777".into(),
                model: "wayland-headless".into(),
            },
        );
        let mode = Mode {
            size: (config.width as i32, config.height as i32).into(),
            refresh: (config.fps * 1000) as i32,
        };
        output.create_global::<Self>(dh);
        output.change_current_state(Some(mode), None, None, Some((0, 0).into()));
        output.set_preferred(mode);

        Self {
            display_handle: dh.clone(),
            socket_name: config.socket_name.clone(),
            width: config.width,
            height: config.height,
            frame_tx,
            origin: Instant::now(),
            dirty: false,
            repaint_request,
            last_emit: Instant::now(),
            force_keyframe_next: false,
            started: false,
            toplevels: Vec::new(),
            x11_windows: Vec::new(),
            x11_override_redirect: Vec::new(),
            xwayland_enabled: config.xwayland,
            xdisplay: config.xdisplay,
            xwm: None,
            x11_display: Arc::new(Mutex::new(None)),
            logged_unsupported_scale: false,
            logged_unsupported_transform: false,
            logged_unsupported_format: false,
            logged_gl_import_failure: false,
            gl_cache_cleanup: false,
            compositor_state,
            xdg_shell_state,
            xwayland_shell_state,
            shm_state,
            seat_state,
            data_device_state,
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            dmabuf_formats: None,
            _output_manager_state: output_manager_state,
            seat,
            _output: output,
            pointer_pos: Point::from((0.0, 0.0)),
        }
    }

    /// Frame tick: composite and emit when anything changed since the last
    /// emitted frame. Before the first contentful frame nothing is emitted
    /// (the test pattern stays live); afterwards the canvas is emitted even
    /// when empty (all clients disconnected → plain background).
    fn produce_frame(&mut self, gl: Option<&mut GlPipeline>) {
        if !self.dirty {
            return;
        }

        let mut canvas = vec![0u8; (self.width * self.height * 4) as usize];
        // Bottom-to-top: later windows paint over earlier ones. Materialize
        // the list: compositing needs `&mut self` (log-once flags, caches).
        let windows = self.canvas_windows();
        let mut drew_content = false;
        if let Some(gl) = gl {
            match self.composite_all_gl(gl, &mut canvas, &windows) {
                Ok(drew) => drew_content = drew,
                // Keep the last good frame on the wire rather than
                // broadcasting a partial image; retry on the next tick.
                Err(e) => {
                    tracing::warn!(
                        "wayland compositor '{}': GL compositing failed: {e:#}",
                        self.socket_name
                    );
                    return;
                }
            }
        } else {
            for window in &windows {
                drew_content |= self.composite_window(&mut canvas, window);
            }
        }
        if self.dirty && !drew_content {
            tracing::debug!(
                "produce_frame: dirty tick drew no content ({} windows)",
                windows.len()
            );
        }

        if !self.started && !drew_content {
            self.dirty = false;
            return;
        }

        let frame = ArgbFrame {
            width: self.width,
            height: self.height,
            stride: self.width * 4,
            data: canvas,
            pts_us: self.origin.elapsed().as_micros() as u64,
            force_keyframe: std::mem::take(&mut self.force_keyframe_next),
        };
        if self.frame_tx.try_send(frame).is_ok() {
            self.started = true;
            self.dirty = false;
            self.last_emit = Instant::now();
            self.send_frame_callbacks();
        }
        // Channel full (encoder busy): keep the dirty flag so the next tick
        // retries instead of showing stale content after the next commit.
    }

    /// The current window stack, bottom-to-top: xdg toplevels, then managed
    /// X11 windows, then X11 override-redirect windows (menus/tooltips).
    /// X11 windows whose wl_surface is not associated yet (created but not
    /// committed) are skipped — there is nothing to draw anyway.
    fn canvas_windows(&self) -> Vec<CanvasWindow> {
        let mut out = Vec::with_capacity(
            self.toplevels.len() + self.x11_windows.len() + self.x11_override_redirect.len(),
        );
        for toplevel in &self.toplevels {
            let surface = toplevel.wl_surface().clone();
            let geometry = with_states(&surface, |states| {
                states
                    .cached_state
                    .get::<SurfaceCachedState>()
                    .current()
                    .geometry
            });
            out.push(CanvasWindow {
                surface,
                geometry,
                x11: false,
            });
        }
        for window in self.x11_windows.iter().chain(&self.x11_override_redirect) {
            if let Some(surface) = window.wl_surface() {
                out.push(CanvasWindow {
                    surface,
                    geometry: Some(window.geometry()),
                    x11: true,
                });
            }
        }
        out
    }

    /// GL path: import every window's buffer (dmabuf or SHM) as a texture
    /// and render them onto the canvas. Buffer position math matches the
    /// CPU blit exactly (`canvas_window_pos`).
    fn composite_all_gl(
        &mut self,
        gl: &mut GlPipeline,
        canvas: &mut [u8],
        windows: &[CanvasWindow],
    ) -> Result<bool> {
        if self.gl_cache_cleanup {
            gl.cleanup_caches();
            self.gl_cache_cleanup = false;
        }
        let mut items = Vec::with_capacity(windows.len());
        for window in windows {
            if let Some(item) = self.gl_draw_item(gl, window) {
                items.push(item);
            }
        }
        let drew = !items.is_empty();
        gl.composite(canvas, &items)?;
        Ok(drew)
    }

    /// Import one window's current buffer as a GL texture and compute its
    /// canvas position. `None` when the surface has no buffer or the import
    /// failed (logged once).
    fn gl_draw_item(&mut self, gl: &mut GlPipeline, window: &CanvasWindow) -> Option<DrawItem> {
        let surface = &window.surface;
        with_states(surface, |states| {
            let (buffer, scale, transform, damage) = {
                let mut guard = states.cached_state.get::<SurfaceAttributes>();
                let attrs = guard.current();
                let buffer = match attrs.buffer.as_ref() {
                    // Unmapped (buffer removed) or never attached.
                    Some(BufferAssignment::NewBuffer(buffer)) => buffer.clone(),
                    _ => return None,
                };
                let damage = attrs
                    .damage
                    .iter()
                    .map(|d| match d {
                        Damage::Buffer(rect) => *rect,
                        // Scale is 1 and the transform Normal on this path
                        // (others are only logged and drawn as-is), so
                        // surface coords are buffer coords.
                        Damage::Surface(rect) => Rectangle::new(
                            Point::from((rect.loc.x, rect.loc.y)),
                            Size::from((rect.size.w, rect.size.h)),
                        ),
                    })
                    .collect::<Vec<_>>();
                (buffer, attrs.buffer_scale, attrs.buffer_transform, damage)
            };

            if scale != 1 && !self.logged_unsupported_scale {
                tracing::warn!(
                    "wayland compositor '{}': buffer scale {scale} is unsupported, drawing 1:1",
                    self.socket_name
                );
                self.logged_unsupported_scale = true;
            }
            if transform != wl_output::Transform::Normal && !self.logged_unsupported_transform {
                tracing::warn!(
                    "wayland compositor '{}': buffer transform {transform:?} is unsupported, drawing untransformed",
                    self.socket_name
                );
                self.logged_unsupported_transform = true;
            }

            let buffer_size = match get_dmabuf(&buffer) {
                Ok(dmabuf) => {
                    let size = dmabuf.size();
                    (size.w, size.h)
                }
                Err(_) => {
                    match with_buffer_contents(&buffer, |_, _, data| (data.width, data.height)) {
                        Ok(size) => size,
                        Err(e) => {
                            tracing::debug!("composite skip: buffer access failed: {e}");
                            return None;
                        }
                    }
                }
            };
            let pos =
                canvas_window_pos(window, (self.width as i32, self.height as i32), buffer_size);

            let texture = match get_dmabuf(&buffer) {
                Ok(dmabuf) => gl.import_dmabuf(dmabuf),
                Err(_) => gl.import_shm(&buffer, states, &damage),
            };
            match texture {
                Ok(texture) => Some(DrawItem {
                    texture,
                    pos: Point::from((pos.x, pos.y)),
                }),
                Err(e) => {
                    if !self.logged_gl_import_failure {
                        tracing::warn!(
                            "wayland compositor '{}': buffer import failed ({e:#}); skipping the window",
                            self.socket_name
                        );
                        self.logged_gl_import_failure = true;
                    }
                    None
                }
            }
        })
    }

    /// Drain pending wl_callback frame callbacks of all window surfaces —
    /// clients that throttle on them would otherwise stop drawing.
    fn send_frame_callbacks(&self) {
        let time = self.origin.elapsed().as_millis() as u32;
        for window in self.canvas_windows() {
            with_states(&window.surface, |states| {
                let mut guard = states.cached_state.get::<SurfaceAttributes>();
                for callback in guard.current().frame_callbacks.drain(..) {
                    callback.done(time);
                }
            });
        }
    }

    /// Blit one window's current buffer into the canvas. Returns true when
    /// any pixel was written.
    fn composite_window(&mut self, canvas: &mut [u8], window: &CanvasWindow) -> bool {
        let surface = &window.surface;
        with_states(surface, |states| {
            let (buffer, scale, transform) = {
                let mut guard = states.cached_state.get::<SurfaceAttributes>();
                let attrs = guard.current();
                let buffer = match attrs.buffer.as_ref() {
                    // Unmapped (buffer removed) or never attached.
                    Some(BufferAssignment::NewBuffer(buffer)) => buffer.clone(),
                    other => {
                        tracing::debug!("composite skip: buffer assignment is {other:?}");
                        return false;
                    }
                };
                (buffer, attrs.buffer_scale, attrs.buffer_transform)
            };

            if scale != 1 && !self.logged_unsupported_scale {
                tracing::warn!(
                    "wayland compositor '{}': buffer scale {scale} is unsupported, drawing 1:1",
                    self.socket_name
                );
                self.logged_unsupported_scale = true;
            }
            if transform != wl_output::Transform::Normal && !self.logged_unsupported_transform {
                tracing::warn!(
                    "wayland compositor '{}': buffer transform {transform:?} is unsupported, drawing untransformed",
                    self.socket_name
                );
                self.logged_unsupported_transform = true;
            }

            with_buffer_contents(&buffer, |pool, pool_len, data| {
                self.blit(canvas, pool, pool_len, data, window)
            })
            .unwrap_or_else(|e| {
                tracing::debug!("composite skip: buffer access failed: {e}");
                false
            })
        })
    }

    /// Blit a client SHM buffer onto the canvas at the window's position
    /// (`canvas_window_pos`): xdg windows are centered and their buffer lands
    /// at `position - geometry.offset` (CSD shadow margins sit outside the
    /// window rect); X11 windows draw 1:1 at their root-relative position.
    fn blit(
        &mut self,
        canvas: &mut [u8],
        pool: *const u8,
        pool_len: usize,
        data: BufferData,
        window: &CanvasWindow,
    ) -> bool {
        let (canvas_w, canvas_h) = (self.width as i32, self.height as i32);
        let dst = canvas_window_pos(window, (canvas_w, canvas_h), (data.width, data.height));

        // The pool may be mutated by the client concurrently; this is the
        // access pattern smithay's own CPU paths use inside the guard.
        let pool = unsafe { std::slice::from_raw_parts(pool, pool_len) };

        let alpha = match data.format {
            wl_shm::Format::Argb8888 => PixelAlpha::Premultiplied,
            wl_shm::Format::Xrgb8888 => PixelAlpha::Opaque,
            other => {
                if !self.logged_unsupported_format {
                    tracing::warn!(
                        "wayland compositor '{}': unsupported SHM format {other:?}, buffer skipped",
                        self.socket_name
                    );
                    self.logged_unsupported_format = true;
                }
                return false;
            }
        };

        blit_buffer(canvas, (canvas_w, canvas_h), pool, &data, dst, alpha)
    }

    // ------------------------------------------------------------------
    // Remote input (input=1)
    // ------------------------------------------------------------------

    /// Buffer size of a surface's current buffer (None when unmapped).
    fn surface_buffer_size(&self, surface: &WlSurface) -> Option<(i32, i32)> {
        with_states(surface, |states| {
            let mut guard = states.cached_state.get::<SurfaceAttributes>();
            let buffer = match guard.current().buffer.as_ref() {
                Some(BufferAssignment::NewBuffer(b)) => b.clone(),
                _ => return None,
            };
            if let Ok(dmabuf) = get_dmabuf(&buffer) {
                let size = dmabuf.size();
                Some((size.w, size.h))
            } else {
                with_buffer_contents(&buffer, |_, _, data| (data.width, data.height)).ok()
            }
        })
    }

    /// A window's clickable canvas rect: xdg uses the *window* geometry
    /// (CSD shadow margins are not clickable), X11 the root-relative window
    /// rect (the buffer is the window there).
    fn canvas_window_rect(&self, window: &CanvasWindow) -> Option<Rectangle<f64, Logical>> {
        if window.x11 {
            return window.geometry.map(|g| {
                Rectangle::new(
                    Point::from((g.loc.x as f64, g.loc.y as f64)),
                    Size::from((g.size.w as f64, g.size.h as f64)),
                )
            });
        }
        let buffer = self.surface_buffer_size(&window.surface)?;
        let (canvas_w, canvas_h) = (self.width as i32, self.height as i32);
        let (win_w, win_h) = match window.geometry {
            Some(rect) if rect.size.w > 0 && rect.size.h > 0 => (rect.size.w, rect.size.h),
            _ => buffer,
        };
        Some(Rectangle::new(
            Point::from((
                ((canvas_w - win_w) / 2) as f64,
                ((canvas_h - win_h) / 2) as f64,
            )),
            Size::from((win_w as f64, win_h as f64)),
        ))
    }

    /// Topmost window under a canvas point, with surface-local coordinates
    /// (the focus form wl_pointer motion expects).
    fn hit_test(&self, x: f64, y: f64) -> Option<(WlSurface, Point<f64, Logical>)> {
        let windows = self.canvas_windows();
        for window in windows.iter().rev() {
            let Some(rect) = self.canvas_window_rect(window) else {
                continue;
            };
            if x < rect.loc.x
                || y < rect.loc.y
                || x >= rect.loc.x + rect.size.w
                || y >= rect.loc.y + rect.size.h
            {
                continue;
            }
            let buffer_size = self.surface_buffer_size(&window.surface)?;
            let pos =
                canvas_window_pos(window, (self.width as i32, self.height as i32), buffer_size);
            return Some((
                window.surface.clone(),
                Point::from((x - pos.x as f64, y - pos.y as f64)),
            ));
        }
        None
    }

    /// Raise and activate an X11 window by its wl_surface (click-to-raise).
    fn raise_x11_at(&mut self, surface: &WlSurface) {
        if let Some(pos) = self
            .x11_windows
            .iter()
            .position(|w| w.wl_surface().as_ref() == Some(surface))
        {
            let window = self.x11_windows.remove(pos);
            self.x11_windows.push(window);
            self.activate_x11_top();
            self.dirty = true;
        }
    }

    /// Inject one remote-input event into the seat.
    fn handle_input(&mut self, event: InputEvent) {
        let time = self.origin.elapsed().as_millis() as u32;
        match event {
            InputEvent::PointerMove { x, y } => {
                self.pointer_pos = Point::from((x, y));
                let focus = self.hit_test(x, y);
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.motion(
                        self,
                        focus,
                        &MotionEvent {
                            location: Point::from((x, y)),
                            serial: SERIAL_COUNTER.next_serial(),
                            time,
                        },
                    );
                }
            }
            InputEvent::PointerButton { button, down } => {
                let serial = SERIAL_COUNTER.next_serial();
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.button(
                        self,
                        &ButtonEvent {
                            serial,
                            time,
                            button,
                            state: if down {
                                ButtonState::Pressed
                            } else {
                                ButtonState::Released
                            },
                        },
                    );
                }
                if down {
                    // Focus follows click; X11 windows also raise.
                    let hit = self.hit_test(self.pointer_pos.x, self.pointer_pos.y);
                    if let Some((surface, _)) = hit {
                        if let Some(keyboard) = self.seat.get_keyboard() {
                            keyboard.set_focus(self, Some(surface.clone()), serial);
                        }
                        self.raise_x11_at(&surface);
                    }
                }
            }
            InputEvent::Wheel { dx, dy } => {
                if let Some(pointer) = self.seat.get_pointer() {
                    // dy/dx arrive in wheel notches (lines); wl_pointer
                    // wants a continuous value plus v120 discrete steps.
                    let mut frame = AxisFrame::new(time).source(AxisSource::Wheel);
                    if dy != 0.0 {
                        frame = frame
                            .value(Axis::Vertical, dy * 15.0)
                            .v120(Axis::Vertical, dy.round() as i32 * 120);
                    }
                    if dx != 0.0 {
                        frame = frame
                            .value(Axis::Horizontal, dx * 15.0)
                            .v120(Axis::Horizontal, dx.round() as i32 * 120);
                    }
                    pointer.axis(self, frame);
                }
            }
            InputEvent::Key { keycode, down } => {
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.input(
                        self,
                        // XKB keycodes are evdev + 8.
                        Keycode::new(keycode + 8),
                        if down {
                            KeyState::Pressed
                        } else {
                            KeyState::Released
                        },
                        SERIAL_COUNTER.next_serial(),
                        time,
                        |_, _, _| FilterResult::<()>::Forward,
                    );
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PixelAlpha {
    Opaque,
    Premultiplied,
}

/// One composited window of the canvas stack (`WaylandState::canvas_windows`):
/// the wl_surface carrying the frames, the window geometry (xdg window
/// geometry for toplevels, the X11 root-relative rect for X11 windows), and
/// the positioning policy.
struct CanvasWindow {
    surface: WlSurface,
    geometry: Option<Rectangle<i32, Logical>>,
    /// X11 windows paint at their requested root position; xdg toplevels
    /// are centered on the single output.
    x11: bool,
}

/// The canvas position of a window's buffer top-left, shared by the CPU and
/// the GL path. X11 windows draw 1:1 at their X11 position (the buffer *is*
/// the window contents; no CSD offsets). xdg windows follow
/// [`window_buffer_pos`].
fn canvas_window_pos(
    window: &CanvasWindow,
    canvas: (i32, i32),
    buffer: (i32, i32),
) -> Point<i32, Logical> {
    if window.x11 {
        return window
            .geometry
            .map(|g| g.loc)
            .unwrap_or_else(|| Point::from((0, 0)));
    }
    window_buffer_pos(canvas, buffer, window.geometry)
}

/// The canvas position of a toplevel's buffer top-left, shared by the CPU
/// and the GL path: windows are centered on the single output; when an xdg
/// window geometry is set, the *window* is centered and the buffer lands at
/// `window position - geometry.loc` (CSD shadow margins sit outside the
/// window rect). Without a geometry the buffer itself is the window.
fn window_buffer_pos(
    canvas: (i32, i32),
    buffer: (i32, i32),
    geometry: Option<Rectangle<i32, Logical>>,
) -> Point<i32, Logical> {
    let (win_w, win_h, geo_off) = match geometry {
        Some(rect) if rect.size.w > 0 && rect.size.h > 0 => (rect.size.w, rect.size.h, rect.loc),
        _ => (buffer.0, buffer.1, Point::from((0, 0))),
    };
    Point::from((
        (canvas.0 - win_w) / 2 - geo_off.x,
        (canvas.1 - win_h) / 2 - geo_off.y,
    ))
}

/// Copy/blend an ARGB8888/XRGB8888 (LE: B,G,R,X) buffer onto the canvas at
/// `dst`, clipped to the canvas. Returns true when any pixel was written.
/// Pure pixel math, separated from the Wayland plumbing for testability.
fn blit_buffer(
    canvas: &mut [u8],
    canvas_size: (i32, i32),
    pool: &[u8],
    data: &BufferData,
    dst: Point<i32, Logical>,
    alpha: PixelAlpha,
) -> bool {
    let (canvas_w, canvas_h) = canvas_size;
    let (w, h, stride, offset) = (data.width, data.height, data.stride, data.offset);
    // Reject nonsense geometry up front instead of trusting the client.
    if w <= 0 || h <= 0 || stride < w.saturating_mul(4) || offset < 0 {
        return false;
    }
    let (w, h, stride, offset) = (w as usize, h as usize, stride as usize, offset as usize);
    if offset + stride * (h - 1) + w * 4 > pool.len() {
        return false;
    }

    let x0 = dst.x.max(0);
    let y0 = dst.y.max(0);
    let x1 = (dst.x + w as i32).min(canvas_w);
    let y1 = (dst.y + h as i32).min(canvas_h);
    if x0 >= x1 || y0 >= y1 {
        return false;
    }

    for y in y0..y1 {
        let src_row = offset + (y - dst.y) as usize * stride;
        let dst_row = y as usize * canvas_w as usize * 4;
        for x in x0..x1 {
            let src_px = src_row + (x - dst.x) as usize * 4;
            let dst_px = dst_row + x as usize * 4;
            let (b, g, r, a) = (
                pool[src_px],
                pool[src_px + 1],
                pool[src_px + 2],
                pool[src_px + 3],
            );
            let dst = &mut canvas[dst_px..dst_px + 4];
            match alpha {
                PixelAlpha::Opaque => {
                    dst[0] = b;
                    dst[1] = g;
                    dst[2] = r;
                    dst[3] = 0xff;
                }
                // wl_shm ARGB8888 carries premultiplied alpha.
                PixelAlpha::Premultiplied => {
                    if a == 0xff {
                        dst[0] = b;
                        dst[1] = g;
                        dst[2] = r;
                        dst[3] = 0xff;
                    } else if a != 0 {
                        let a16 = a as u16;
                        let inv = 255 - a16;
                        dst[0] = (b as u16 + (dst[0] as u16 * inv + 127) / 255) as u8;
                        dst[1] = (g as u16 + (dst[1] as u16 * inv + 127) / 255) as u8;
                        dst[2] = (r as u16 + (dst[2] as u16 * inv + 127) / 255) as u8;
                        dst[3] = (a16 + (dst[3] as u16 * inv + 127) / 255) as u8;
                    }
                }
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Smithay handler implementations
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

impl CompositorHandler for WaylandState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // The XWayland server connects as a client carrying
        // `XWaylandClientData` (smithay inserts it), not our `ClientState`.
        if let Some(data) = client.get_data::<XWaylandClientData>() {
            return &data.compositor_state;
        }
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, _surface: &WlSurface) {
        self.dirty = true;
    }

    fn destroyed(&mut self, _surface: &WlSurface) {
        self.dirty = true;
    }
}

delegate_compositor!(WaylandState);

impl BufferHandler for WaylandState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {
        // The renderer may still cache a texture for the destroyed buffer's
        // dmabuf; drop it at the next frame so the buffer is really freed.
        self.gl_cache_cleanup = true;
    }
}

impl ShmHandler for WaylandState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_shm!(WaylandState);

impl DmabufHandler for WaylandState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        // The renderer lives in the frame timer and is unreachable from
        // protocol dispatch, so sanity-check against the advertised formats
        // here; the real import happens per frame at composite time.
        let supported = self
            .dmabuf_formats
            .as_ref()
            .is_some_and(|formats| formats.contains(&dmabuf.format()));
        if supported {
            if let Err(e) = notifier.successful::<Self>() {
                tracing::warn!("wayland compositor: dmabuf import notification failed: {e}");
            }
        } else {
            tracing::warn!(
                "wayland compositor '{}': client submitted a dmabuf outside the advertised formats ({:?})",
                self.socket_name,
                dmabuf.format()
            );
            notifier.failed();
        }
    }
}

delegate_dmabuf!(WaylandState);

impl SeatHandler for WaylandState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
}

delegate_seat!(WaylandState);

impl OutputHandler for WaylandState {}

delegate_output!(WaylandState);

impl SelectionHandler for WaylandState {
    type SelectionUserData = ();
}

impl DataDeviceHandler for WaylandState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for WaylandState {}
impl ServerDndGrabHandler for WaylandState {}

delegate_data_device!(WaylandState);

impl XdgShellHandler for WaylandState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.toplevels.push(surface.clone());
        self.dirty = true;
        // Unblocks the client's initial commit (default state lets the
        // client choose its size).
        surface.send_configure();
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.toplevels.retain(|t| t != &surface);
        self.gl_cache_cleanup = true;
        self.dirty = true;
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        // Popups get their configure so the client proceeds, but are not
        // composited (MVP: toplevels only).
        let _ = surface.send_configure();
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: smithay::utils::Serial) {}

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        let _ = surface.send_configure();
        surface.send_repositioned(token);
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<wl_output::WlOutput>,
    ) {
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some((self.width as i32, self.height as i32).into());
        });
        surface.send_configure();
        self.dirty = true;
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Maximized);
            state.size = Some((self.width as i32, self.height as i32).into());
        });
        surface.send_configure();
        self.dirty = true;
    }
}

delegate_xdg_shell!(WaylandState);

// ---------------------------------------------------------------------------
// XWayland (X11 clients)
// ---------------------------------------------------------------------------
//
// The XWM policy is deliberately minimal — this is a headless fixed-size
// "virtual monitor", not a desktop: managed windows keep their requested
// root-relative geometry (configure requests are granted as-is), fullscreen
// and maximize both mean "canvas-sized at (0,0)", and the stacking order is
// the window list order (last mapped on top, reorder requests honored).
// Interactive move/resize is dropped: there is no pointer input.

impl XWaylandShellHandler for WaylandState {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }
}

delegate_xwayland_shell!(WaylandState);

impl WaylandState {
    /// Topmost managed X11 window gets _NET_ACTIVE_WINDOW-style activation;
    /// there is no keyboard input to focus with yet.
    fn activate_x11_top(&self) {
        for window in &self.x11_windows {
            let _ = window.set_activated(false);
        }
        if let Some(top) = self.x11_windows.last() {
            let _ = top.set_activated(true);
        }
    }
}

impl XwmHandler for WaylandState {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm
            .as_mut()
            .expect("XWM events only arrive after start_wm")
    }

    fn new_window(&mut self, _xwm: XwmId, window: X11Surface) {
        tracing::debug!(
            "wayland compositor '{}': new X11 window {}x{} at {:?} (class '{}')",
            self.socket_name,
            window.geometry().size.w,
            window.geometry().size.h,
            window.geometry().loc,
            window.class()
        );
    }

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        // Grant the requested geometry (clamped to something drawable), then
        // map. XWayland hands us windows with a real size; a 0x0 fallback
        // keeps a pathological client from poisoning the canvas math.
        let mut geometry = window.geometry();
        if geometry.size.w <= 0 || geometry.size.h <= 0 {
            geometry.size = Size::from((640, 480));
        }
        if let Err(e) = window.configure(geometry) {
            tracing::warn!("wayland compositor: X11 configure failed: {e}");
        }
        if let Err(e) = window.set_mapped(true) {
            tracing::warn!("wayland compositor: X11 map failed: {e}");
            return;
        }
        self.x11_windows.push(window);
        self.activate_x11_top();
        self.dirty = true;
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.x11_override_redirect.push(window);
        self.dirty = true;
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        if window.is_override_redirect() {
            self.x11_override_redirect.retain(|w| w != &window);
        } else {
            self.x11_windows.retain(|w| w != &window);
            // Keep the WM state honest for a later re-map.
            let _ = window.set_mapped(false);
            self.activate_x11_top();
        }
        self.gl_cache_cleanup = true;
        self.dirty = true;
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.x11_windows.retain(|w| w != &window);
        self.x11_override_redirect.retain(|w| w != &window);
        self.activate_x11_top();
        self.gl_cache_cleanup = true;
        self.dirty = true;
    }

    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        reorder: Option<Reorder>,
    ) {
        let mut geometry = window.geometry();
        if let Some(x) = x {
            geometry.loc.x = x;
        }
        if let Some(y) = y {
            geometry.loc.y = y;
        }
        if let Some(w) = w {
            geometry.size.w = w as i32;
        }
        if let Some(h) = h {
            geometry.size.h = h as i32;
        }
        if let Err(e) = window.configure(geometry) {
            tracing::warn!("wayland compositor: X11 configure failed: {e}");
        }
        if let Some(reorder) = reorder {
            match reorder {
                Reorder::Top => {
                    self.x11_windows.retain(|w| w != &window);
                    self.x11_windows.push(window.clone());
                }
                Reorder::Bottom => {
                    self.x11_windows.retain(|w| w != &window);
                    self.x11_windows.insert(0, window.clone());
                }
                // Relative reorders resolve against the managed stack.
                Reorder::Above(target) => {
                    self.x11_windows.retain(|w| w != &window);
                    let pos = self
                        .x11_windows
                        .iter()
                        .position(|w| w.window_id() == target)
                        .map(|p| p + 1)
                        .unwrap_or(self.x11_windows.len());
                    self.x11_windows.insert(pos, window.clone());
                }
                Reorder::Below(target) => {
                    self.x11_windows.retain(|w| w != &window);
                    let pos = self
                        .x11_windows
                        .iter()
                        .position(|w| w.window_id() == target)
                        .unwrap_or(0);
                    self.x11_windows.insert(pos, window.clone());
                }
            }
        }
        self.dirty = true;
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _geometry: Rectangle<i32, Logical>,
        _above: Option<smithay::xwayland::xwm::X11Window>,
    ) {
        // Override-redirect windows reconfigure themselves; the composite
        // reads `geometry()` fresh every frame, so just repaint.
        self.dirty = true;
    }

    fn property_notify(&mut self, _xwm: XwmId, _window: X11Surface, _property: WmWindowProperty) {}

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.configure(Rectangle::new(
            Point::from((0, 0)),
            Size::from((self.width as i32, self.height as i32)),
        ));
        let _ = window.set_fullscreen(true);
        self.x11_windows.retain(|w| w != &window);
        self.x11_windows.push(window);
        self.dirty = true;
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.set_fullscreen(false);
        self.dirty = true;
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.configure(Rectangle::new(
            Point::from((0, 0)),
            Size::from((self.width as i32, self.height as i32)),
        ));
        let _ = window.set_maximized(true);
        self.dirty = true;
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        let _ = window.set_maximized(false);
        self.dirty = true;
    }

    // No pointer input: interactive resize/move grabs cannot happen.
    fn resize_request(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _button: u32,
        _edge: ResizeEdge,
    ) {
    }

    fn move_request(&mut self, _xwm: XwmId, _window: X11Surface, _button: u32) {}

    fn disconnected(&mut self, _xwm: XwmId) {
        tracing::error!(
            "wayland compositor '{}': lost the XWM connection (XWayland died); X11 windows dropped",
            self.socket_name
        );
        self.x11_windows.clear();
        self.x11_override_redirect.clear();
        self.gl_cache_cleanup = true;
        self.dirty = true;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(size: (i32, i32)) -> Vec<u8> {
        vec![0u8; (size.0 * size.1 * 4) as usize]
    }

    fn px(buf: &[u8], w: i32, x: i32, y: i32) -> [u8; 4] {
        let i = (y * w * 4 + x * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn buffer_data(width: i32, height: i32) -> BufferData {
        BufferData {
            offset: 0,
            width,
            height,
            stride: width * 4,
            format: wl_shm::Format::Xrgb8888,
        }
    }

    #[test]
    fn blit_opaque_copy_and_clip() {
        // 2x2 red buffer drawn at (1,1) on a 4x4 canvas.
        let mut pool = vec![0u8; 2 * 2 * 4];
        for px in pool.chunks_exact_mut(4) {
            px.copy_from_slice(&[0, 0, 255, 255]); // B,G,R,X
        }
        let mut out = canvas((4, 4));
        assert!(blit_buffer(
            &mut out,
            (4, 4),
            &pool,
            &buffer_data(2, 2),
            Point::from((1, 1)),
            PixelAlpha::Opaque,
        ));
        assert_eq!(px(&out, 4, 0, 0), [0, 0, 0, 0]);
        assert_eq!(px(&out, 4, 1, 1), [0, 0, 255, 255]);
        assert_eq!(px(&out, 4, 2, 2), [0, 0, 255, 255]);
        assert_eq!(px(&out, 4, 3, 3), [0, 0, 0, 0]);

        // Negative destination clips.
        let mut out = canvas((4, 4));
        assert!(blit_buffer(
            &mut out,
            (4, 4),
            &pool,
            &buffer_data(2, 2),
            Point::from((-1, -1)),
            PixelAlpha::Opaque,
        ));
        assert_eq!(px(&out, 4, 0, 0), [0, 0, 255, 255]);
        assert_eq!(px(&out, 4, 1, 1), [0, 0, 0, 0]);

        // Fully outside: nothing drawn.
        let mut out = canvas((4, 4));
        assert!(!blit_buffer(
            &mut out,
            (4, 4),
            &pool,
            &buffer_data(2, 2),
            Point::from((10, 10)),
            PixelAlpha::Opaque,
        ));
    }

    #[test]
    fn blit_premultiplied_alpha_over() {
        // Single 1x1 pixel: 50% alpha red over opaque black canvas.
        let pool = vec![0u8, 0, 127, 127]; // B,G,R,A premultiplied
        let mut out = canvas((1, 1));
        let mut data = buffer_data(1, 1);
        data.format = wl_shm::Format::Argb8888;
        assert!(blit_buffer(
            &mut out,
            (1, 1),
            &pool,
            &data,
            Point::from((0, 0)),
            PixelAlpha::Premultiplied,
        ));
        assert_eq!(px(&out, 1, 0, 0), [0, 0, 127, 127]);
    }

    #[test]
    fn blit_rejects_hostile_geometry() {
        let pool = vec![0u8; 16];
        let mut out = canvas((4, 4));
        // Stride smaller than the row width.
        let mut data = buffer_data(4, 1);
        data.stride = 4;
        assert!(!blit_buffer(
            &mut out,
            (4, 4),
            &pool,
            &data,
            Point::from((0, 0)),
            PixelAlpha::Opaque,
        ));
        // Buffer extends past the pool.
        let mut data = buffer_data(4, 2);
        data.offset = 8;
        assert!(!blit_buffer(
            &mut out,
            (4, 4),
            &pool,
            &data,
            Point::from((0, 0)),
            PixelAlpha::Opaque,
        ));
        // Negative sizes.
        let data = buffer_data(-4, 2);
        assert!(!blit_buffer(
            &mut out,
            (4, 4),
            &pool,
            &data,
            Point::from((0, 0)),
            PixelAlpha::Opaque,
        ));
    }
}

#[cfg(test)]
mod client_tests {
    use super::*;
    use std::io::Write;
    use std::os::fd::AsFd;

    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{
        wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
    };
    use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
    use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

    const RED: [u8; 4] = [0, 0, 255, 255]; // B,G,R,X
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [255, 0, 0, 255];
    const BACKGROUND: [u8; 4] = [0, 0, 0, 0];

    #[derive(Default)]
    struct TestClientState {
        configured: bool,
        surface: Option<wl_surface::WlSurface>,
        buffer: Option<wl_buffer::WlBuffer>,
    }

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for TestClientState {
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

    impl Dispatch<xdg_surface::XdgSurface, ()> for TestClientState {
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

    delegate_noop!(TestClientState: wl_compositor::WlCompositor);
    delegate_noop!(TestClientState: wl_shm_pool::WlShmPool);
    // These interfaces deliver events (formats, release, configure, ...):
    // consume and ignore them.
    delegate_noop!(TestClientState: ignore wl_shm::WlShm);
    delegate_noop!(TestClientState: ignore wl_buffer::WlBuffer);
    delegate_noop!(TestClientState: ignore wl_surface::WlSurface);
    delegate_noop!(TestClientState: ignore xdg_wm_base::XdgWmBase);
    delegate_noop!(TestClientState: ignore xdg_toplevel::XdgToplevel);

    /// A connected client holding a mapped xdg toplevel. Dropping it
    /// disconnects the client (and destroys the toplevel server-side).
    struct TestToplevelClient {
        _conn: Connection,
        _queue: wayland_client::EventQueue<TestClientState>,
        _state: TestClientState,
    }

    fn add_toplevel(
        w: i32,
        h: i32,
        fill: &dyn Fn(&mut [u8], i32, i32),
        window_geometry: Option<(i32, i32, i32, i32)>,
    ) -> TestToplevelClient {
        let conn = Connection::connect_to_env().expect("connect to the compositor");
        let (globals, mut queue) =
            registry_queue_init::<TestClientState>(&conn).expect("registry init");
        let qh = queue.handle();

        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=5, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();

        let surface = compositor.create_surface(&qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_title("live777-test".into());
        if let Some((x, y, gw, gh)) = window_geometry {
            xdg.set_window_geometry(x, y, gw, gh);
        }

        let mut pixels = vec![0u8; (w * h * 4) as usize];
        fill(&mut pixels, w, h);
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&pixels).unwrap();
        let pool = shm.create_pool(file.as_fd(), w * h * 4, &qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, &qh, ());

        let mut state = TestClientState {
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

        TestToplevelClient {
            _conn: conn,
            _queue: queue,
            _state: state,
        }
    }

    fn fill_solid(color: [u8; 4]) -> impl Fn(&mut [u8], i32, i32) {
        move |pixels, _, _| {
            for px in pixels.chunks_exact_mut(4) {
                px.copy_from_slice(&color);
            }
        }
    }

    fn fill_half_green_half_red(pixels: &mut [u8], w: i32, h: i32) {
        for y in 0..h {
            for x in 0..w {
                let i = (y * w * 4 + x * 4) as usize;
                let color = if x < w / 2 { GREEN } else { RED };
                pixels[i..i + 4].copy_from_slice(&color);
            }
        }
    }

    fn fill_top_red_bottom_blue(pixels: &mut [u8], w: i32, h: i32) {
        for y in 0..h {
            let color = if y < h / 2 { RED } else { BLUE };
            for x in 0..w {
                let i = (y * w * 4 + x * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&color);
            }
        }
    }

    fn frame_px(frame: &ArgbFrame, x: u32, y: u32) -> [u8; 4] {
        let i = (y * frame.width * 4 + x * 4) as usize;
        [
            frame.data[i],
            frame.data[i + 1],
            frame.data[i + 2],
            frame.data[i + 3],
        ]
    }

    async fn next_frame(rx: &mut mpsc::Receiver<ArgbFrame>, timeout: Duration) -> ArgbFrame {
        tokio::time::timeout(timeout, rx.recv())
            .await
            .expect("timed out waiting for a composited frame")
            .expect("compositor frame channel closed")
    }

    /// Frames may be in flight from an earlier phase; skip until (x,y)
    /// matches `want` or the deadline passes.
    async fn wait_pixel(
        rx: &mut mpsc::Receiver<ArgbFrame>,
        x: u32,
        y: u32,
        want: [u8; 4],
        timeout: Duration,
    ) -> ArgbFrame {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "pixel ({x},{y}) never became {want:?}"
            );
            let frame = next_frame(rx, remaining).await;
            if frame_px(&frame, x, y) == want {
                return frame;
            }
        }
    }

    #[tokio::test]
    async fn compositor_composites_real_clients() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                std::env::var("RUST_LOG").unwrap_or_else(|_| "liveion=debug".to_string()),
            )
            .with_test_writer()
            .try_init();

        let runtime_dir = tempfile::tempdir().unwrap();
        // Safe in this test: nextest isolates test processes.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", runtime_dir.path());
            std::env::set_var("WAYLAND_DISPLAY", "live777-test");
        }
        let socket_path = runtime_dir.path().join("live777-test");

        let config = WaylandUrlConfig::parse(
            "test",
            "wayland://live777-test?width=320&height=240&fps=30&bitrate=500000",
        )
        .unwrap();
        let (frame_tx, mut frame_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let handle = spawn(
            &config,
            frame_tx,
            cancel.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(socket_path.exists(), "socket file must exist after spawn");

        // No clients: the compositor must stay silent (the pattern generator
        // owns the stream in this phase).
        assert!(
            tokio::time::timeout(Duration::from_millis(500), frame_rx.recv())
                .await
                .is_err(),
            "compositor must not emit frames without client content"
        );

        // Client 1: full-output solid red.
        let client1 = add_toplevel(320, 240, &fill_solid(RED), None);
        let frame = wait_pixel(&mut frame_rx, 160, 120, RED, Duration::from_secs(10)).await;
        assert_eq!((frame.width, frame.height), (320, 240));
        assert_eq!(frame_px(&frame, 0, 0), RED, "corner must be red too");

        // Idle desktop: no commits → no frames.
        assert!(
            tokio::time::timeout(Duration::from_millis(500), frame_rx.recv())
                .await
                .is_err(),
            "an idle desktop must not burn encoder CPU"
        );

        // Client 2 stacked on top: solid blue.
        let client2 = add_toplevel(320, 240, &fill_solid(BLUE), None);
        wait_pixel(&mut frame_rx, 160, 120, BLUE, Duration::from_secs(10)).await;

        // Disconnect client 2: client 1's red is on top again.
        drop(client2);
        wait_pixel(&mut frame_rx, 160, 120, RED, Duration::from_secs(10)).await;

        // Disconnect client 1: empty background.
        drop(client1);
        wait_pixel(&mut frame_rx, 160, 120, BACKGROUND, Duration::from_secs(10)).await;

        // Client 3: 100x100 half-green/half-red buffer with window geometry
        // (30,10,60,80). Window (60x80) centered on 320x240 → top-left
        // (130,90); buffer lands at (130-30, 90-10) = (100,70); the
        // green/red boundary sits at canvas x = 100 + 50 = 150.
        let client3 = add_toplevel(100, 100, &fill_half_green_half_red, Some((30, 10, 60, 80)));
        let frame = wait_pixel(&mut frame_rx, 149, 120, GREEN, Duration::from_secs(10)).await;
        assert_eq!(
            frame_px(&frame, 151, 120),
            RED,
            "right of the buffer's half split must be red"
        );
        assert_eq!(
            frame_px(&frame, 0, 0),
            BACKGROUND,
            "outside the window must stay background"
        );
        assert_eq!(frame_px(&frame, 100, 70), GREEN, "buffer top-left corner");
        drop(client3);
        wait_pixel(&mut frame_rx, 160, 120, BACKGROUND, Duration::from_secs(10)).await;

        // Shutdown: thread exits, socket file is removed.
        cancel.cancel();
        handle.join().await;
        assert!(
            !socket_path.exists(),
            "socket file must be removed when the compositor stops"
        );
    }

    /// SHM buffers are top-down on both compositing paths (CPU blit and the
    /// GL import): a red-top/blue-bottom buffer must land unflipped.
    #[tokio::test]
    async fn shm_buffer_vertical_orientation() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                std::env::var("RUST_LOG").unwrap_or_else(|_| "liveion=debug".to_string()),
            )
            .with_test_writer()
            .try_init();
        let runtime_dir = tempfile::tempdir().unwrap();
        // Safe in this test: nextest isolates test processes.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", runtime_dir.path());
            std::env::set_var("WAYLAND_DISPLAY", "live777-test-shm-orient");
        }

        let config = WaylandUrlConfig::parse(
            "test",
            "wayland://live777-test-shm-orient?width=320&height=240&fps=30&bitrate=500000",
        )
        .unwrap();
        let (frame_tx, mut frame_rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let handle = spawn(
            &config,
            frame_tx,
            cancel.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        // 96x64 centered on the canvas → top-left (112, 88).
        let _client = add_toplevel(96, 64, &fill_top_red_bottom_blue, None);
        let frame = wait_pixel(&mut frame_rx, 160, 100, RED, Duration::from_secs(10)).await;
        assert_eq!(
            frame_px(&frame, 160, 140),
            BLUE,
            "SHM content must be top-down (bottom band stays bottom)"
        );

        cancel.cancel();
        handle.join().await;
    }

    /// A static desktop (one commit, then silence) must still serve new
    /// subscribers: an explicit repaint request re-emits the canvas as an
    /// IDR, and the idle keepalive keeps producing IDRs on its own.
    #[tokio::test]
    async fn compositor_repaints_on_request_and_keepalive() {
        let runtime_dir = tempfile::tempdir().unwrap();
        // Safe in this test: nextest isolates test processes.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", runtime_dir.path());
            std::env::set_var("WAYLAND_DISPLAY", "live777-test-repaint");
        }

        let config = WaylandUrlConfig::parse(
            "test",
            "wayland://live777-test-repaint?width=320&height=240&fps=30&bitrate=500000",
        )
        .unwrap();
        let (frame_tx, mut frame_rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let repaint = Arc::new(AtomicBool::new(false));
        let handle = spawn(&config, frame_tx, cancel.clone(), repaint.clone()).unwrap();

        // Client draws once and goes idle.
        let _client = add_toplevel(320, 240, &fill_solid(RED), None);
        let frame = wait_pixel(&mut frame_rx, 160, 120, RED, Duration::from_secs(10)).await;
        assert!(
            !frame.force_keyframe,
            "an ordinary commit must not force an IDR"
        );

        // Quiesced: nothing new until requested (within the keepalive window).
        assert!(
            tokio::time::timeout(Duration::from_millis(500), frame_rx.recv())
                .await
                .is_err(),
            "a static desktop must not emit unprompted frames"
        );

        // PLI-style repaint request: one IDR with the unchanged content.
        repaint.store(true, Ordering::Relaxed);
        let frame = next_frame(&mut frame_rx, Duration::from_secs(2)).await;
        assert!(
            frame.force_keyframe,
            "a repaint-requested frame must be an IDR"
        );
        assert_eq!(frame_px(&frame, 160, 120), RED, "repaint keeps the canvas");

        // Idle keepalive: another IDR arrives within about two intervals.
        let deadline = Instant::now() + KEEPALIVE_INTERVAL * 2 + Duration::from_millis(500);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "no keepalive IDR arrived");
            let frame = next_frame(&mut frame_rx, remaining).await;
            if frame.force_keyframe {
                assert_eq!(frame_px(&frame, 160, 120), RED);
                break;
            }
        }

        cancel.cancel();
        handle.join().await;
    }

    // ---------------------------------------------------------------------
    // dmabuf clients (GL compositing path)
    // ---------------------------------------------------------------------

    use super::super::gl;
    use wayland_protocols::wp::linux_dmabuf::zv1::client::{
        zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1,
    };

    /// First accessible DRM device from the compositor's candidate list.
    fn open_drm_device() -> Option<std::fs::File> {
        gl::device_candidates().into_iter().find_map(|path| {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .ok()
        })
    }

    #[derive(Default)]
    struct DmabufClientState {
        configured: bool,
        surface: Option<wl_surface::WlSurface>,
        buffer: Option<wl_buffer::WlBuffer>,
        released: u32,
    }

    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for DmabufClientState {
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

    impl Dispatch<xdg_surface::XdgSurface, ()> for DmabufClientState {
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

    impl Dispatch<wl_buffer::WlBuffer, ()> for DmabufClientState {
        fn event(
            state: &mut Self,
            _buffer: &wl_buffer::WlBuffer,
            event: wl_buffer::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<Self>,
        ) {
            if matches!(event, wl_buffer::Event::Release) {
                state.released += 1;
            }
        }
    }

    delegate_noop!(DmabufClientState: wl_compositor::WlCompositor);
    // These interfaces deliver events (configure, release, formats, ...):
    // consume and ignore them.
    delegate_noop!(DmabufClientState: ignore wl_surface::WlSurface);
    delegate_noop!(DmabufClientState: ignore xdg_wm_base::XdgWmBase);
    delegate_noop!(DmabufClientState: ignore xdg_toplevel::XdgToplevel);
    delegate_noop!(DmabufClientState: ignore zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1);
    delegate_noop!(DmabufClientState: ignore zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1);

    /// Fill a mappable GBM buffer with a solid color (B,G,R,X).
    fn fill_bo(bo: &mut gbm::BufferObject<()>, w: u32, h: u32, color: [u8; 4]) {
        bo.map_mut(0, 0, w, h, |mapped| {
            let stride = mapped.stride() as usize;
            for row in mapped.buffer_mut().chunks_exact_mut(stride) {
                for px in row[..w as usize * 4].chunks_exact_mut(4) {
                    px.copy_from_slice(&color);
                }
            }
        })
        .expect("map the GBM buffer");
    }

    /// Fill a mappable GBM buffer with two horizontal bands (top/bottom) —
    /// a vertically asymmetric pattern so a flip is detectable.
    fn fill_bo_halves(
        bo: &mut gbm::BufferObject<()>,
        w: u32,
        h: u32,
        top: [u8; 4],
        bottom: [u8; 4],
    ) {
        bo.map_mut(0, 0, w, h, |mapped| {
            let stride = mapped.stride() as usize;
            for (y, row) in mapped.buffer_mut().chunks_exact_mut(stride).enumerate() {
                let color = if y < h as usize / 2 { top } else { bottom };
                for px in row[..w as usize * 4].chunks_exact_mut(4) {
                    px.copy_from_slice(&color);
                }
            }
        })
        .expect("map the GBM buffer");
    }

    /// A connected dmabuf client holding a mapped xdg toplevel, with two
    /// GBM buffers (double buffering). Dropping it disconnects the client.
    struct DmabufToplevelClient {
        conn: Connection,
        queue: wayland_client::EventQueue<DmabufClientState>,
        state: DmabufClientState,
        dmabuf: zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        qh: QueueHandle<DmabufClientState>,
        bos: [gbm::BufferObject<()>; 2],
        buffers: [wl_buffer::WlBuffer; 2],
        buf_size: (u32, u32),
    }

    impl DmabufToplevelClient {
        /// None when the compositor has no dmabuf global (GL off) or the
        /// host has no usable DRM device for the client-side allocator.
        fn new(size: (u32, u32), color: [u8; 4]) -> Option<Self> {
            let conn = Connection::connect_to_env().expect("connect to the compositor");
            let (globals, mut queue) =
                registry_queue_init::<DmabufClientState>(&conn).expect("registry init");
            let qh = queue.handle();

            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=5, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
            let dmabuf: zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 =
                globals.bind(&qh, 1..=4, ()).ok()?;

            let device_file = open_drm_device()?;
            let gbm = gbm::Device::new(device_file).expect("GBM device");

            let (w, h) = size;
            let flags = gbm::BufferObjectFlags::RENDERING | gbm::BufferObjectFlags::LINEAR;
            let mut bos = [
                gbm.create_buffer_object::<()>(w, h, gbm::Format::Xrgb8888, flags)
                    .expect("GBM buffer A"),
                gbm.create_buffer_object::<()>(w, h, gbm::Format::Xrgb8888, flags)
                    .expect("GBM buffer B"),
            ];
            fill_bo_halves(&mut bos[0], w, h, RED, BLUE);
            fill_bo(&mut bos[1], w, h, color);

            let buffers: Vec<_> = bos
                .iter()
                .map(|bo| create_wl_buffer(&dmabuf, &qh, bo, size, false))
                .collect();

            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_title("live777-dmabuf-test".into());

            let mut state = DmabufClientState {
                configured: false,
                surface: Some(surface),
                buffer: Some(buffers[0].clone()),
                released: 0,
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

            let buffers: [wl_buffer::WlBuffer; 2] = buffers.try_into().unwrap();
            Some(Self {
                conn,
                queue,
                state,
                dmabuf,
                qh,
                bos,
                buffers,
                buf_size: (w, h),
            })
        }

        /// Repaint buffer `i` with `color` and make it the current buffer
        /// (the previously displayed one is released by the compositor),
        /// then roundtrip so release events land in `state.released`.
        fn commit_filled(&mut self, i: usize, color: [u8; 4]) {
            let (w, h) = self.buf_size;
            fill_bo(&mut self.bos[i], w, h, color);
            self.commit_buffer(i);
        }

        /// Same, with a vertically asymmetric top/bottom band pattern.
        fn commit_halves(&mut self, i: usize, top: [u8; 4], bottom: [u8; 4]) {
            let (w, h) = self.buf_size;
            fill_bo_halves(&mut self.bos[i], w, h, top, bottom);
            self.commit_buffer(i);
        }

        /// Repaint buffer `i` and attach it via a *fresh* wl_buffer created
        /// with the given y_invert flag (the flag is per-buffer, so probing
        /// it needs new wl_buffers).
        fn commit_halves_flags(&mut self, i: usize, top: [u8; 4], bottom: [u8; 4], y_invert: bool) {
            let (w, h) = self.buf_size;
            fill_bo_halves(&mut self.bos[i], w, h, top, bottom);
            let buffer = create_wl_buffer(&self.dmabuf, &self.qh, &self.bos[i], (w, h), y_invert);
            let surface = self.state.surface.as_ref().unwrap();
            surface.attach(Some(&buffer), 0, 0);
            surface.commit();
            self.conn.flush().unwrap();
            self.queue.roundtrip(&mut self.state).unwrap();
        }

        fn commit_buffer(&mut self, i: usize) {
            let surface = self.state.surface.as_ref().unwrap();
            surface.attach(Some(&self.buffers[i]), 0, 0);
            surface.commit();
            self.conn.flush().unwrap();
            self.queue.roundtrip(&mut self.state).unwrap();
        }
    }

    /// Create a wl_buffer from a GBM buffer via the dmabuf params object.
    fn create_wl_buffer(
        dmabuf: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        qh: &QueueHandle<DmabufClientState>,
        bo: &gbm::BufferObject<()>,
        size: (u32, u32),
        y_invert: bool,
    ) -> wl_buffer::WlBuffer {
        let (w, h) = size;
        let fd = bo.fd().expect("export the GBM buffer as dmabuf");
        let modifier: u64 = bo.modifier().into();
        let params = dmabuf.create_params(qh, ());
        params.add(
            fd.as_fd(),
            0,
            bo.offset(0),
            bo.stride(),
            (modifier >> 32) as u32,
            modifier as u32,
        );
        let flags = if y_invert {
            zwp_linux_buffer_params_v1::Flags::YInvert
        } else {
            zwp_linux_buffer_params_v1::Flags::empty()
        };
        let buffer = params.create_immed(
            w as i32,
            h as i32,
            gbm::Format::Xrgb8888 as u32,
            flags,
            qh,
            (),
        );
        params.destroy();
        buffer
    }

    #[tokio::test]
    async fn dmabuf_global_is_advertised_exactly_when_gl_is_up() {
        let runtime_dir = tempfile::tempdir().unwrap();
        // Safe in this test: nextest isolates test processes.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", runtime_dir.path());
            std::env::set_var("WAYLAND_DISPLAY", "live777-test-dmabuf-global");
        }

        let config = WaylandUrlConfig::parse(
            "test",
            "wayland://live777-test-dmabuf-global?width=320&height=240&fps=30&bitrate=500000",
        )
        .unwrap();
        let (frame_tx, _frame_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let handle = spawn(
            &config,
            frame_tx,
            cancel.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        // A connected client's first registry roundtrip only completes once
        // the event loop runs, which is after GL init: the global set and
        // the gl_active flag are both final by then.
        let conn = Connection::connect_to_env().unwrap();
        let (globals, _queue) =
            registry_queue_init::<DmabufClientState>(&conn).expect("registry init");
        let advertised = globals
            .contents()
            .clone_list()
            .iter()
            .any(|g| g.interface == "zwp_linux_dmabuf_v1");
        assert_eq!(
            advertised,
            handle.gl_active(),
            "zwp_linux_dmabuf_v1 must be advertised exactly when GL compositing initialized"
        );
        if open_drm_device().is_some() {
            assert!(
                handle.gl_active(),
                "GL must initialize on a host with an accessible DRM device"
            );
        } else {
            assert!(
                !handle.gl_active(),
                "no accessible DRM device: the compositor must fall back to CPU compositing"
            );
        }

        drop(conn);
        cancel.cancel();
        handle.join().await;
    }

    /// Full dmabuf path: a client submitting GBM-allocated dmabuf buffers
    /// through zwp_linux_dmabuf_v1 must land on the canvas (positioned like
    /// the CPU path), and buffer release must keep flowing so a
    /// double-buffered client can repaint and re-commit without starving.
    #[tokio::test]
    async fn dmabuf_client_reaches_the_canvas_and_buffers_cycle() {
        let runtime_dir = tempfile::tempdir().unwrap();
        // Safe in this test: nextest isolates test processes.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", runtime_dir.path());
            std::env::set_var("WAYLAND_DISPLAY", "live777-test-dmabuf");
        }

        let config = WaylandUrlConfig::parse(
            "test",
            "wayland://live777-test-dmabuf?width=320&height=240&fps=30&bitrate=500000",
        )
        .unwrap();
        let (frame_tx, mut frame_rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let handle = spawn(
            &config,
            frame_tx,
            cancel.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        // 96x64 centered on the 320x240 canvas → top-left (112, 88); the
        // first buffer carries a red top band over a blue bottom band, so a
        // vertical flip in the dmabuf import path is detectable.
        let Some(mut client) = DmabufToplevelClient::new((96, 64), GREEN) else {
            eprintln!("skipping the dmabuf client test: no dmabuf global or no DRM device");
            cancel.cancel();
            handle.join().await;
            return;
        };

        // The dmabuf buffer is composited, positioned like the CPU path.
        let frame = wait_pixel(&mut frame_rx, 160, 100, RED, Duration::from_secs(10)).await;
        assert_eq!(frame_px(&frame, 0, 0), BACKGROUND, "outside the window");
        assert_eq!(frame_px(&frame, 112, 88), RED, "window top-left corner");
        assert_eq!(
            frame_px(&frame, 160, 140),
            BLUE,
            "the bottom band must stay the bottom band (vertical flip check)"
        );
        assert_eq!(
            frame_px(&frame, 207, 151),
            BLUE,
            "window bottom-right corner"
        );
        assert_eq!(frame_px(&frame, 208, 152), BACKGROUND, "past the window");

        // Repaint the other buffer: a content switch through the dmabuf path.
        client.commit_filled(1, GREEN);
        wait_pixel(&mut frame_rx, 160, 120, GREEN, Duration::from_secs(10)).await;

        // Reuse the first buffer (released by the compositor when the second
        // commit replaced it): proves the release flow doesn't starve a
        // double-buffered client. Swapped bands re-check orientation.
        client.commit_halves(0, BLUE, RED);
        let frame = wait_pixel(&mut frame_rx, 160, 100, BLUE, Duration::from_secs(10)).await;
        assert_eq!(
            frame_px(&frame, 160, 140),
            RED,
            "repaint must keep its orientation too"
        );

        assert!(
            client.state.released >= 2,
            "the compositor must release replaced buffers (got {})",
            client.state.released
        );

        drop(client);
        wait_pixel(&mut frame_rx, 160, 120, BACKGROUND, Duration::from_secs(10)).await;

        cancel.cancel();
        handle.join().await;
    }

    /// Orientation regression for dmabuf clients: the canvas must show the
    /// buffer as-is when y_invert is unset, and flipped when set (the
    /// protocol semantics), for both content orders. dmabuf imports are
    /// bottom-origin EGL images, so the compositor applies one corrective
    /// vertical flip on top of the flag handling.
    #[tokio::test]
    async fn dmabuf_orientation_matches_y_invert_flag() {
        let runtime_dir = tempfile::tempdir().unwrap();
        // Safe in this test: nextest isolates test processes.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", runtime_dir.path());
            std::env::set_var("WAYLAND_DISPLAY", "live777-test-dmabuf-orient");
        }
        let config = WaylandUrlConfig::parse(
            "test",
            "wayland://live777-test-dmabuf-orient?width=320&height=240&fps=30&bitrate=500000",
        )
        .unwrap();
        let (frame_tx, mut frame_rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let handle = spawn(
            &config,
            frame_tx,
            cancel.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let Some(mut client) = DmabufToplevelClient::new((96, 64), GREEN) else {
            eprintln!("skipping the dmabuf orientation test: no dmabuf global or no DRM device");
            cancel.cancel();
            handle.join().await;
            return;
        };

        // (memory top band, flag) → expected canvas top band: the flag flips.
        for (swapped, flag, want_top) in [
            (false, false, RED),
            (false, true, BLUE),
            (true, false, BLUE),
            (true, true, RED),
        ] {
            let (top, bottom) = if swapped { (BLUE, RED) } else { (RED, BLUE) };
            client.commit_halves_flags(0, top, bottom, flag);
            // Drop frames that predate this commit (the channel buffers
            // them across phases).
            while frame_rx.try_recv().is_ok() {}
            let frame =
                wait_pixel(&mut frame_rx, 160, 100, want_top, Duration::from_secs(10)).await;
            assert_eq!(
                frame_px(&frame, 160, 140),
                if want_top == RED { BLUE } else { RED },
                "bottom band must follow (swapped={swapped}, flag={flag})"
            );
        }
        cancel.cancel();
        handle.join().await;
    }

    /// X11 clients: with XWayland up, a mapped X11 window must be composited
    /// at its X11 position (1:1, not centered like xdg toplevels), repaints
    /// must land, and unmapping must return the canvas to the background.
    /// Skipped without an `Xwayland` binary (CI hosts may not have one).
    #[tokio::test]
    async fn x11_window_reaches_the_canvas() {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{
            ConnectionExt as _, CreateGCAux, CreateWindowAux, Rectangle as XRectangle, WindowClass,
        };

        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                std::env::var("RUST_LOG").unwrap_or_else(|_| "liveion=debug".to_string()),
            )
            .with_test_writer()
            .try_init();

        if std::process::Command::new("Xwayland")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            eprintln!("skipping the X11 test: no Xwayland binary on PATH");
            return;
        }

        let runtime_dir = tempfile::tempdir().unwrap();
        // Safe in this test: nextest isolates test processes.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", runtime_dir.path());
            std::env::set_var("WAYLAND_DISPLAY", "live777-test-x11");
        }

        let config = WaylandUrlConfig::parse(
            "test",
            "wayland://live777-test-x11?width=320&height=240&fps=30&bitrate=500000",
        )
        .unwrap();
        let (frame_tx, mut frame_rx) = mpsc::channel(8);
        let cancel = CancellationToken::new();
        let handle = spawn(
            &config,
            frame_tx,
            cancel.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        // XWayland startup is async (a forked process); wait for the display.
        let display = {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if let Some(display) = handle.x11_display() {
                    break display;
                }
                assert!(Instant::now() < deadline, "XWayland never reported ready");
                std::thread::sleep(Duration::from_millis(50));
            }
        };

        // X11 client: a 200x150 window at (20,20). Painting happens on the
        // first Expose event — the canonical X11 pattern, and the only safe
        // ordering: damage issued before Xwayland realizes the window (and
        // creates its wl_surface) is silently dropped. Red window with a
        // green left half; X11 pixels are 0xRRGGBB.
        let (conn, screen_num) =
            x11rb::connect(Some(&format!(":{display}"))).expect("connect to XWayland");
        let screen = &conn.setup().roots[screen_num];
        let win = conn.generate_id().unwrap();
        conn.create_window(
            screen.root_depth,
            win,
            screen.root,
            20,
            20,
            200,
            150,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new()
                .background_pixel(0x00FF0000)
                .event_mask(x11rb::protocol::xproto::EventMask::EXPOSURE),
        )
        .unwrap();
        conn.map_window(win).unwrap();
        let gc = conn.generate_id().unwrap();
        conn.create_gc(gc, win, &CreateGCAux::new().foreground(0x0000FF00))
            .unwrap();
        conn.flush().unwrap();

        // Wait for the first Expose, then paint the green half.
        loop {
            let event = conn.wait_for_event().expect("X11 event");
            if matches!(event, x11rb::protocol::Event::Expose(_)) {
                break;
            }
        }
        conn.poly_fill_rectangle(
            win,
            gc,
            &[XRectangle {
                x: 0,
                y: 0,
                width: 100,
                height: 150,
            }],
        )
        .unwrap();
        conn.flush().unwrap();

        // Window rect (20,20)-(220,170) on the 320x240 canvas.
        wait_pixel(&mut frame_rx, 70, 95, GREEN, Duration::from_secs(15)).await;
        let frame = wait_pixel(&mut frame_rx, 170, 95, RED, Duration::from_secs(15)).await;
        assert_eq!(frame_px(&frame, 5, 5), BACKGROUND, "outside the window");
        assert_eq!(
            frame_px(&frame, 20, 20),
            GREEN,
            "window top-left corner (X11 position honored, not centered)"
        );
        assert_eq!(frame_px(&frame, 219, 169), RED, "window bottom-right");
        assert_eq!(frame_px(&frame, 220, 170), BACKGROUND, "past the window");

        // Unmap: the canvas returns to background.
        conn.unmap_window(win).unwrap();
        conn.flush().unwrap();
        wait_pixel(&mut frame_rx, 120, 95, BACKGROUND, Duration::from_secs(15)).await;

        drop(conn);
        cancel.cancel();
        handle.join().await;
    }

    /// Remote input: events pushed through the source's input sink must come
    /// out as seat events at the focused client (pointer enter/motion,
    /// button, key). Uses a dedicated client that records seat events.
    #[tokio::test]
    async fn input_events_reach_the_focused_client() {
        use std::sync::Mutex as StdMutex;

        use wayland_client::protocol::{wl_keyboard, wl_pointer, wl_seat};

        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                std::env::var("RUST_LOG").unwrap_or_else(|_| "liveion=debug".to_string()),
            )
            .with_test_writer()
            .try_init();

        #[derive(Default)]
        struct InputClientState {
            configured: bool,
            surface: Option<wl_surface::WlSurface>,
            buffer: Option<wl_buffer::WlBuffer>,
            got_keyboard: bool,
            got_pointer: bool,
            /// (kind, value) — e.g. ("enter", serial), ("motion-x*1000", ..),
            /// ("button", state), ("key", evdev keycode).
            events: Arc<StdMutex<Vec<(String, u32)>>>,
        }

        impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for InputClientState {
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

        impl Dispatch<xdg_surface::XdgSurface, ()> for InputClientState {
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

        impl Dispatch<wl_seat::WlSeat, ()> for InputClientState {
            fn event(
                state: &mut Self,
                seat: &wl_seat::WlSeat,
                event: wl_seat::Event,
                _data: &(),
                _conn: &Connection,
                qh: &QueueHandle<Self>,
            ) {
                if let wl_seat::Event::Capabilities { capabilities } = event {
                    let caps = match capabilities {
                        wayland_client::WEnum::Value(c) => c,
                        wayland_client::WEnum::Unknown(_) => return,
                    };
                    if caps.contains(wl_seat::Capability::Keyboard) {
                        state.got_keyboard = true;
                        seat.get_keyboard(qh, ());
                    }
                    if caps.contains(wl_seat::Capability::Pointer) {
                        state.got_pointer = true;
                        seat.get_pointer(qh, ());
                    }
                }
            }
        }

        impl Dispatch<wl_keyboard::WlKeyboard, ()> for InputClientState {
            fn event(
                state: &mut Self,
                _kb: &wl_keyboard::WlKeyboard,
                event: wl_keyboard::Event,
                _data: &(),
                _conn: &Connection,
                _qh: &QueueHandle<Self>,
            ) {
                if let wl_keyboard::Event::Key { key, state: s, .. } = event {
                    let s = match s {
                        wayland_client::WEnum::Value(wl_keyboard::KeyState::Pressed) => 1,
                        _ => 0,
                    };
                    state
                        .events
                        .lock()
                        .unwrap()
                        .push(("key".into(), key + s * 100000));
                }
            }
        }

        impl Dispatch<wl_pointer::WlPointer, ()> for InputClientState {
            fn event(
                state: &mut Self,
                _ptr: &wl_pointer::WlPointer,
                event: wl_pointer::Event,
                _data: &(),
                _conn: &Connection,
                _qh: &QueueHandle<Self>,
            ) {
                match event {
                    wl_pointer::Event::Enter { .. } => {
                        state.events.lock().unwrap().push(("enter".into(), 1));
                    }
                    wl_pointer::Event::Motion {
                        surface_x,
                        surface_y,
                        ..
                    } => {
                        state
                            .events
                            .lock()
                            .unwrap()
                            .push(("motion".into(), (surface_x * 1000.0 + surface_y) as u32));
                    }
                    wl_pointer::Event::Button { state: s, .. } => {
                        let s = match s {
                            wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed) => 1,
                            _ => 0,
                        };
                        state.events.lock().unwrap().push(("button".into(), s));
                    }
                    _ => {}
                }
            }
        }

        delegate_noop!(InputClientState: wl_compositor::WlCompositor);
        delegate_noop!(InputClientState: wl_shm_pool::WlShmPool);
        delegate_noop!(InputClientState: ignore wl_shm::WlShm);
        delegate_noop!(InputClientState: ignore wl_buffer::WlBuffer);
        delegate_noop!(InputClientState: ignore wl_surface::WlSurface);
        delegate_noop!(InputClientState: ignore xdg_wm_base::XdgWmBase);
        delegate_noop!(InputClientState: ignore xdg_toplevel::XdgToplevel);

        let runtime_dir = tempfile::tempdir().unwrap();
        // Safe in this test: nextest isolates test processes.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", runtime_dir.path());
            std::env::set_var("WAYLAND_DISPLAY", "live777-test-input");
        }

        let config = WaylandUrlConfig::parse(
            "test",
            "wayland://live777-test-input?width=320&height=240&fps=30&bitrate=500000&input=1",
        )
        .unwrap();
        let (frame_tx, _frame_rx) = mpsc::channel(4);
        let cancel = CancellationToken::new();
        let handle = spawn(
            &config,
            frame_tx,
            cancel.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        let input_tx = handle
            .input_sender()
            .expect("input=1 must expose the input sink");

        // Client: 100x100 window, centered on the 320x240 canvas → window
        // rect (110,70)-(210,170).
        let events: Arc<StdMutex<Vec<(String, u32)>>> = Arc::new(StdMutex::new(Vec::new()));
        let conn = Connection::connect_to_env().expect("connect to the compositor");
        let (globals, mut queue) =
            registry_queue_init::<InputClientState>(&conn).expect("registry init");
        let qh = queue.handle();

        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 1..=5, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let _seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();

        let surface = compositor.create_surface(&qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
        let _toplevel = xdg.get_toplevel(&qh, ());

        let (w, h) = (100, 100);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        fill_solid(RED)(&mut pixels, w, h);
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&pixels).unwrap();
        let pool = shm.create_pool(file.as_fd(), w * h * 4, &qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, &qh, ());

        let mut state = InputClientState {
            events: events.clone(),
            ..Default::default()
        };
        state.surface = Some(surface);
        state.buffer = Some(buffer);
        state.surface.as_ref().unwrap().commit();
        while !state.configured {
            queue.blocking_dispatch(&mut state).unwrap();
        }
        conn.flush().unwrap();

        // Wait for the seat's capabilities to arrive, then the events.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !state.got_keyboard || !state.got_pointer {
            assert!(Instant::now() < deadline, "seat capabilities never arrived");
            queue.blocking_dispatch(&mut state).unwrap();
        }

        // Pointer move into the window center (160,120 canvas).
        input_tx
            .send(super::super::input::InputEvent::PointerMove { x: 160.0, y: 120.0 })
            .unwrap();
        // Click: button press sets the keyboard focus.
        input_tx
            .send(super::super::input::InputEvent::PointerButton {
                button: 0x110,
                down: true,
            })
            .unwrap();
        input_tx
            .send(super::super::input::InputEvent::PointerButton {
                button: 0x110,
                down: false,
            })
            .unwrap();
        // Key press + release (KeyA = evdev 30).
        input_tx
            .send(super::super::input::InputEvent::Key {
                keycode: 30,
                down: true,
            })
            .unwrap();
        input_tx
            .send(super::super::input::InputEvent::Key {
                keycode: 30,
                down: false,
            })
            .unwrap();

        // Drain client events until all expected ones arrived.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            {
                let evs = events.lock().unwrap();
                let has = |k: &str, v: u32| evs.iter().any(|(kk, vv)| kk == k && *vv == v);
                if has("enter", 1)
                    && has("button", 1)
                    && has("button", 0)
                    && has("key", 30 + 100000)
                    && has("key", 30)
                {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "input events never arrived: {events:?}"
            );
            queue.blocking_dispatch(&mut state).unwrap();
        }

        cancel.cancel();
        handle.join().await;
    }
}
