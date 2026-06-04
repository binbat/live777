//! Safe Rust wrapper around the C++ SourcePipeline FFI.
//!
//! Data flow:
//!   C++ SourcePipeline → EncodedPacketFFI callback → copy → mpsc channel
//!
//! The `EncodedPacketFFI.data` pointer is valid only within the FFI callback.
//! Data is immediately copied into `EncodedPacket` and sent through the channel.

use crate::native_ffi::*;
use crate::types::{EncodedPacket, NativeSourceParams};
use anyhow::Result;
use std::ffi::{CString, c_char};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

const ERR_BUF_LEN: usize = 256;

// ---------------------------------------------------------------------------
// Raw handle wrapper
// ---------------------------------------------------------------------------

/// Wraps a raw C++ `SourcePipelineHandle` pointer so that the outer
/// `Arc<Mutex<Option<…>>>` no longer directly contains `*mut ()`.
///
/// # Safety
///
/// The raw pointer is only accessed while protected by
/// [`SharedPipelineHandle`]'s mutex.  `stop` / `Drop` take the handle
/// out of the `Option`, so the pointer is never used after it has been
/// freed.
struct PipelineHandlePtr(*mut SourcePipelineHandle);

// Raw pointers are Copy; a Copy wrapper derives Clone trivially.
impl Copy for PipelineHandlePtr {}
impl Clone for PipelineHandlePtr {
    fn clone(&self) -> Self {
        *self
    }
}

// SAFETY: see struct-level doc.
unsafe impl Send for PipelineHandlePtr {}
unsafe impl Sync for PipelineHandlePtr {}

// ---------------------------------------------------------------------------
// Shared pipeline handle
// ---------------------------------------------------------------------------

/// The raw FFI handle is only accessed through this mutex.
/// Send/Sync are implemented below with the guarantee that stop/free
/// are serialized and the handle is not used after being taken.
#[derive(Clone)]
struct SharedPipelineHandle {
    inner: Arc<Mutex<Option<PipelineHandlePtr>>>,
}

unsafe impl Send for SharedPipelineHandle {}
unsafe impl Sync for SharedPipelineHandle {}

impl SharedPipelineHandle {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    fn set(&self, h: *mut SourcePipelineHandle) {
        *self.inner.lock().unwrap() = Some(PipelineHandlePtr(h));
    }

    fn take(&self) -> Option<PipelineHandlePtr> {
        self.inner.lock().unwrap().take()
    }

    fn request_keyframe(&self) {
        let guard = self.inner.lock().unwrap();
        if let Some(h) = guard.as_ref() {
            unsafe { source_pipeline_request_keyframe(h.0) };
        }
    }
}

// ---------------------------------------------------------------------------
// FFI callback context
// ---------------------------------------------------------------------------

struct CallbackCtx {
    tx: Mutex<Option<mpsc::UnboundedSender<EncodedPacket>>>,
}

/// FFI callback — invoked from C++ encoder thread.
///
/// Data is copied immediately; the `pkt.data` pointer is invalid after return.
/// Uses `UnboundedSender::send()` which is synchronous (no .await needed).
unsafe extern "C" fn on_encoded_packet(
    pkt: *const EncodedPacketFFI,
    user_data: *mut std::ffi::c_void,
) {
    if pkt.is_null() || user_data.is_null() {
        return;
    }

    let pkt = unsafe { &*pkt };
    let ctx = unsafe { &*(user_data as *const CallbackCtx) };

    // Copy immediately — pkt.data is invalid after return
    let data = if pkt.size > 0 && !pkt.data.is_null() {
        unsafe { std::slice::from_raw_parts(pkt.data, pkt.size) }.to_vec()
    } else {
        return;
    };

    let encoded = EncodedPacket {
        codec: pkt.codec,
        data,
        pts_us: pkt.pts_us,
        dts_us: pkt.dts_us,
        flags: pkt.flags,
    };

    let guard = ctx.tx.lock().unwrap();
    if let Some(tx) = guard.as_ref() {
        let _ = tx.send(encoded);
    }
}

// ---------------------------------------------------------------------------
// NativePipeline
// ---------------------------------------------------------------------------

/// Owns a C++ SourcePipeline and delivers `EncodedPacket` frames through a
/// channel.
///
/// # Safety / lifecycle
///
/// - All FFI raw-pointer operations are scoped within `{ }` blocks.
/// - `CallbackCtx` is owned via `Box` — no raw pointers to Rust objects leak
///   across FFI.
/// - `Drop` stops the pipeline and frees C++ resources.
pub struct NativePipeline {
    handle: SharedPipelineHandle,
    ctx: Option<Box<CallbackCtx>>,
    _cstrs: Vec<CString>,
    rx: Option<mpsc::UnboundedReceiver<EncodedPacket>>,
}

unsafe impl Send for NativePipeline {}
unsafe impl Sync for NativePipeline {}

impl NativePipeline {
    /// Create a new pipeline from the given parameters.
    ///
    /// The pipeline is configured but **not started** — call [`start`] to
    /// begin streaming.
    pub fn new(params: &NativeSourceParams) -> Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();

        let ctx = Box::new(CallbackCtx {
            tx: Mutex::new(Some(tx)),
        });
        let ctx_ptr = Box::into_raw(ctx);
        let user_data = ctx_ptr as *mut std::ffi::c_void;

        let hooks = SourcePipelineHooksFFI {
            on_packet: Some(on_encoded_packet),
            user_data,
        };

        let (ffi_cfg, cstrs) = build_ffi_config(params);

        let mut errbuf: [c_char; ERR_BUF_LEN] = [0; ERR_BUF_LEN];

        // Scope raw-pointer locals so they are dropped before we hand back
        // the Result — avoids raw pointers living across potential Send
        // boundaries.
        let raw_handle = {
            let _keep_cstrs = &cstrs;
            unsafe {
                source_pipeline_create(
                    &ffi_cfg as *const _,
                    &hooks as *const _,
                    errbuf.as_mut_ptr(),
                    ERR_BUF_LEN,
                )
            }
        };

        if raw_handle.is_null() {
            // Free CallbackCtx — pipeline creation failed, no C++ callbacks
            // will fire.
            unsafe {
                drop(Box::from_raw(ctx_ptr));
            }
            let err_str = unsafe { std::ffi::CStr::from_ptr(errbuf.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            anyhow::bail!("source_pipeline_create failed: {}", err_str);
        }

        // Reconstruct ctx Box — C++ now owns a copy of user_data for
        // callbacks.
        let ctx = unsafe { Box::from_raw(ctx_ptr) };

        let handle = SharedPipelineHandle::new();
        handle.set(raw_handle);

        Ok(Self {
            handle,
            ctx: Some(ctx),
            _cstrs: cstrs,
            rx: Some(rx),
        })
    }

    /// Start streaming.
    ///
    /// Returns a receiver that yields [`EncodedPacket`] frames from the
    /// C++ encoder thread.  The pipeline is stopped when this
    /// `NativePipeline` is dropped.
    pub fn start(&mut self) -> Result<mpsc::UnboundedReceiver<EncodedPacket>> {
        let raw_handle = {
            let guard = self.handle.inner.lock().unwrap();
            guard.ok_or_else(|| anyhow::anyhow!("pipeline not initialised"))?
        };

        if !unsafe { source_pipeline_start(raw_handle.0) } {
            anyhow::bail!("source_pipeline_start failed");
        }

        Ok(self.rx.take().expect("start called twice"))
    }

    /// Stop streaming and free C++ resources.
    pub fn stop(&mut self) {
        if let Some(raw_handle) = self.handle.take() {
            unsafe {
                source_pipeline_stop(raw_handle.0);
                source_pipeline_free(raw_handle.0);
            }
        }
        // Drop the sender side so the receiver knows we're done.
        if let Some(ctx) = self.ctx.take()
            && let Ok(mut guard) = ctx.tx.lock()
        {
            *guard = None;
        }
    }

    /// Request an IDR keyframe from the encoder.
    pub fn request_keyframe(&self) {
        self.handle.request_keyframe();
    }

    /// Return a cloneable handle that can be sent to other tasks for
    /// requesting keyframes (e.g. RTCP PLI handler).
    pub fn keyframe_handle(&self) -> KeyframeHandle {
        KeyframeHandle {
            handle: self.handle.clone(),
        }
    }
}

/// Cloneable handle for requesting keyframes from other tasks.
#[derive(Clone)]
pub struct KeyframeHandle {
    handle: SharedPipelineHandle,
}

impl KeyframeHandle {
    pub fn request_keyframe(&self) {
        self.handle.request_keyframe();
    }
}

impl Drop for NativePipeline {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// FFI config builder
// ---------------------------------------------------------------------------

fn build_ffi_config(params: &NativeSourceParams) -> (SourcePipelineConfigFFI, Vec<CString>) {
    let mut cstrs = Vec::new();

    let cap_backend = CString::new(params.capture_backend.as_str()).unwrap();
    let cap_device = CString::new(params.capture_device.as_str()).unwrap();
    let enc_backend = CString::new(params.encoder_backend.as_str()).unwrap();
    let profile = CString::new(params.profile.as_str()).unwrap();

    let cfg = SourcePipelineConfigFFI {
        capture: CaptureConfigFFI {
            backend: cap_backend.as_ptr(),
            device: cap_device.as_ptr(),
            width: params.width,
            height: params.height,
            fps: params.fps,
            pixel_format: params.capture_pixel_format,
            prefer_dmabuf: params.capture_prefer_dmabuf,
        },
        encoder: EncoderConfigFFI {
            backend: enc_backend.as_ptr(),
            codec: params.codec,
            width: params.width,
            height: params.height,
            fps: params.fps,
            bitrate: params.bitrate,
            profile: profile.as_ptr(),
            gop: params.gop,
            prefer_dmabuf: params.encoder_prefer_dmabuf,
        },
        payload_type: params.payload_type,
        clock_rate: params.clock_rate,
    };

    cstrs.push(cap_backend);
    cstrs.push(cap_device);
    cstrs.push(enc_backend);
    cstrs.push(profile);

    (cfg, cstrs)
}
