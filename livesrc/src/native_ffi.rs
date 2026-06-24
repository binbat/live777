//! Crate-private FFI bindings for the pure-C source_pipeline_* API.
//!
//! Mirrors `libcamera-bridge/include/source_pipeline_ffi.h`.
//! All structs are `#[repr(C)]` and use only C ABI-safe types.
//! C `bool` is mapped to Rust `u8` (0 = false, non-0 = true).
//!
//! This module is **not** public — livesrc exposes only `NativePipeline`,
//! `NativeSourceParams`, and `EncodedPacket`.

use std::os::raw::{c_char, c_void};

// ---------------------------------------------------------------------------
// FFI config structs
// ---------------------------------------------------------------------------

#[repr(C)]
pub(crate) struct CaptureConfigFFI {
    pub backend: *const c_char,
    pub device: *const c_char,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub pixel_format: u32,
    pub prefer_dmabuf: u8,
}

#[repr(C)]
pub(crate) struct EncoderConfigFFI {
    pub backend: *const c_char,
    pub codec: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub profile: *const c_char,
    pub gop: u32,
    pub prefer_dmabuf: u8,
}

#[repr(C)]
pub(crate) struct SourcePipelineConfigFFI {
    pub capture: CaptureConfigFFI,
    pub encoder: EncoderConfigFFI,
    pub payload_type: u32,
    pub clock_rate: u32,
}

// ---------------------------------------------------------------------------
// EncodedPacketFFI — the only data crossing from C++ to Rust
// ---------------------------------------------------------------------------

#[repr(C)]
pub(crate) struct EncodedPacketFFI {
    pub codec: u32,
    pub data: *const u8, // valid only during callback
    pub size: usize,
    pub pts_us: u64,
    pub dts_us: u64,
    pub flags: u32,
}

// ---------------------------------------------------------------------------
// Callback and hooks
// ---------------------------------------------------------------------------

pub(crate) type EncodedPacketCallbackFFI =
    unsafe extern "C" fn(packet: *const EncodedPacketFFI, user_data: *mut c_void);

#[repr(C)]
pub(crate) struct SourcePipelineHooksFFI {
    pub on_packet: Option<EncodedPacketCallbackFFI>,
    pub user_data: *mut c_void,
}

// ---------------------------------------------------------------------------
// Opaque handle
// ---------------------------------------------------------------------------

#[repr(C)]
pub(crate) struct SourcePipelineHandle {
    _private: [u8; 0],
}

// ---------------------------------------------------------------------------
// C API
// ---------------------------------------------------------------------------

unsafe extern "C" {
    pub fn source_pipeline_create(
        cfg: *const SourcePipelineConfigFFI,
        hooks: *const SourcePipelineHooksFFI,
        errbuf: *mut c_char,
        errbuf_len: usize,
    ) -> *mut SourcePipelineHandle;

    pub fn source_pipeline_start(
        h: *mut SourcePipelineHandle,
        errbuf: *mut c_char,
        errbuf_len: usize,
    ) -> bool;
    pub fn source_pipeline_stop(h: *mut SourcePipelineHandle);
    #[allow(dead_code)]
    pub fn source_pipeline_is_running(h: *mut SourcePipelineHandle) -> bool;
    pub fn source_pipeline_request_keyframe(h: *mut SourcePipelineHandle);
    pub fn source_pipeline_free(h: *mut SourcePipelineHandle);
}
