//! Headless Wayland compositor (smithay, SHM-only, no GPU) producing
//! [`ArgbFrame`]s.
//!
//! Runs on a dedicated thread with a calloop event loop; [`spawn`] binds the
//! Wayland socket synchronously so [`WaylandSource`](super::WaylandSource)'s
//! `start()` fails cleanly on setup errors. Visible xdg toplevels' current
//! SHM buffers are blitted onto a fixed-size canvas (alpha-over for
//! ARGB8888, plain copy for XRGB8888) in stacking order; frames are produced
//! on a timer only when a commit happened since the last emitted frame.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, Mode as LoopMode, PostAction};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_seat::WlSeat;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_output, wl_shm};
use smithay::reexports::wayland_server::{Client, Display, DisplayHandle};
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes,
    with_states,
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
use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{ArgbFrame, WaylandUrlConfig};

/// A running headless compositor. The thread exits when `cancel` is
/// cancelled; dropping the [`CompositorHandle`] also cancels (so the thread
/// can never outlive its owner), and [`join`](Self::join) awaits it.
pub struct CompositorHandle {
    socket_name: String,
    thread: Option<std::thread::JoinHandle<()>>,
    cancel: CancellationToken,
}

impl CompositorHandle {
    /// Wayland display name clients should use (`WAYLAND_DISPLAY`).
    pub fn socket_name(&self) -> &str {
        &self.socket_name
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
    let state = WaylandState::new(&dh, config, frame_tx);

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
    let thread = std::thread::Builder::new()
        .name(format!("wayland-comp-{}", config.socket_name))
        .spawn(move || run(display, state, listener, period, thread_cancel))
        .map_err(|e| anyhow::anyhow!("failed to spawn compositor thread: {e}"))?;

    tracing::info!(
        "wayland compositor listening on WAYLAND_DISPLAY={}",
        socket_name
    );

    Ok(CompositorHandle {
        socket_name,
        thread: Some(thread),
        cancel,
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

    if let Err(e) = loop_handle.insert_source(listener, |stream, _, state| {
        if let Err(e) = state
            .display_handle
            .insert_client(stream, Arc::new(ClientState::default()))
        {
            tracing::warn!("wayland compositor: failed to insert client: {e}");
        }
    }) {
        tracing::error!("wayland compositor '{socket_name}': failed to add socket source: {e}");
        return;
    }

    if let Err(e) = loop_handle.insert_source(
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
        tracing::error!("wayland compositor '{socket_name}': failed to add display source: {e}");
        return;
    }

    if let Err(e) = loop_handle.insert_source(Timer::immediate(), move |_, _, state| {
        if cancel.is_cancelled() {
            loop_signal.stop();
            return TimeoutAction::Drop;
        }
        state.produce_frame();
        if let Err(e) = state.display_handle.flush_clients() {
            tracing::debug!("wayland compositor: client flush error: {e}");
        }
        TimeoutAction::ToDuration(period)
    }) {
        tracing::error!("wayland compositor '{socket_name}': failed to add frame timer: {e}");
        return;
    }

    tracing::info!("wayland compositor '{socket_name}': event loop started");
    if let Err(e) = event_loop.run(None, &mut state, |_| {}) {
        tracing::error!("wayland compositor '{socket_name}': event loop error: {e}");
    }
    tracing::info!("wayland compositor '{socket_name}': event loop exited");
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
    /// The first frame with actual client content was emitted; before that,
    /// the test pattern stays on (handled by the source's frame mux).
    started: bool,
    /// xdg toplevels, bottom-to-top stacking order (creation order).
    toplevels: Vec<ToplevelSurface>,
    logged_unsupported_scale: bool,
    logged_unsupported_transform: bool,
    logged_unsupported_format: bool,

    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    seat_state: SeatState<Self>,
    data_device_state: DataDeviceState,
    // Kept alive for their globals/objects; not otherwise read.
    _output_manager_state: OutputManagerState,
    _seat: Seat<Self>,
    _output: Output,
}

impl WaylandState {
    fn new(
        dh: &DisplayHandle,
        config: &WaylandUrlConfig,
        frame_tx: mpsc::Sender<ArgbFrame>,
    ) -> Self {
        let compositor_state = CompositorState::new::<Self>(dh);
        let xdg_shell_state = XdgShellState::new::<Self>(dh);
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
            started: false,
            toplevels: Vec::new(),
            logged_unsupported_scale: false,
            logged_unsupported_transform: false,
            logged_unsupported_format: false,
            compositor_state,
            xdg_shell_state,
            shm_state,
            seat_state,
            data_device_state,
            _output_manager_state: output_manager_state,
            _seat: seat,
            _output: output,
        }
    }

    /// Frame tick: composite and emit when anything changed since the last
    /// emitted frame. Before the first contentful frame nothing is emitted
    /// (the test pattern stays live); afterwards the canvas is emitted even
    /// when empty (all clients disconnected → plain background).
    fn produce_frame(&mut self) {
        if !self.dirty {
            return;
        }

        let mut canvas = vec![0u8; (self.width * self.height * 4) as usize];
        let mut drew_content = false;
        // Bottom-to-top: later toplevels paint over earlier ones. Clone the
        // list: compositing needs `&mut self` (log-once flags).
        let toplevels = self.toplevels.clone();
        for toplevel in &toplevels {
            drew_content |= self.composite_toplevel(&mut canvas, toplevel);
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
        };
        if self.frame_tx.try_send(frame).is_ok() {
            self.started = true;
            self.dirty = false;
            self.send_frame_callbacks();
        }
        // Channel full (encoder busy): keep the dirty flag so the next tick
        // retries instead of showing stale content after the next commit.
    }

    /// Drain pending wl_callback frame callbacks of all toplevel surfaces —
    /// clients that throttle on them would otherwise stop drawing.
    fn send_frame_callbacks(&self) {
        let time = self.origin.elapsed().as_millis() as u32;
        for toplevel in &self.toplevels {
            with_states(toplevel.wl_surface(), |states| {
                let mut guard = states.cached_state.get::<SurfaceAttributes>();
                for callback in guard.current().frame_callbacks.drain(..) {
                    callback.done(time);
                }
            });
        }
    }

    /// Blit one toplevel's current buffer into the canvas. Returns true when
    /// any pixel was written.
    fn composite_toplevel(&mut self, canvas: &mut [u8], toplevel: &ToplevelSurface) -> bool {
        let surface = toplevel.wl_surface();
        with_states(surface, |states| {
            let (buffer, scale, transform) = {
                let mut guard = states.cached_state.get::<SurfaceAttributes>();
                let attrs = guard.current();
                let buffer = match attrs.buffer.as_ref() {
                    // Unmapped (buffer removed) or never attached.
                    Some(BufferAssignment::NewBuffer(buffer)) => buffer.clone(),
                    _ => return false,
                };
                (buffer, attrs.buffer_scale, attrs.buffer_transform)
            };
            let geometry = states
                .cached_state
                .get::<SurfaceCachedState>()
                .current()
                .geometry;

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
                self.blit(canvas, pool, pool_len, data, geometry)
            })
            .unwrap_or(false)
        })
    }

    /// Blit a client SHM buffer onto the canvas, honoring the xdg window
    /// geometry: the canvas position is the *window's* top-left, so the
    /// buffer lands at `position - geometry.offset` (CSD shadow margins sit
    /// outside the window rect).
    fn blit(
        &mut self,
        canvas: &mut [u8],
        pool: *const u8,
        pool_len: usize,
        data: BufferData,
        geometry: Option<Rectangle<i32, Logical>>,
    ) -> bool {
        let (canvas_w, canvas_h) = (self.width as i32, self.height as i32);
        let (win_w, win_h, geo_off) = match geometry {
            Some(rect) if rect.size.w > 0 && rect.size.h > 0 => {
                (rect.size.w, rect.size.h, rect.loc)
            }
            _ => (data.width, data.height, Point::from((0, 0))),
        };
        // Windows are centered on the single output.
        let dst = Point::from((
            (canvas_w - win_w) / 2 - geo_off.x,
            (canvas_h - win_h) / 2 - geo_off.y,
        ));

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PixelAlpha {
    Opaque,
    Premultiplied,
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
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for WaylandState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_shm!(WaylandState);

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
        let handle = spawn(&config, frame_tx, cancel.clone()).unwrap();
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
}
