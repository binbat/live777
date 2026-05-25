//! Rust FFI bindings for the pure-C source_pipeline_* API.
//!
//! Mirrors `livesrc/libcamera-bridge/include/source_pipeline_ffi.h`.
//! All structs are `#[repr(C)]` and use only C ABI-safe types.
//! C `bool` is mapped to Rust `u8` (0 = false, non-0 = true).

use std::os::raw::{c_char, c_void};

// ---------------------------------------------------------------------------
// FFI config structs
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct CaptureConfigFFI {
    pub backend: *const c_char,  // "libcamera" or "v4l2"
    pub device: *const c_char,   // "/dev/video0" or camera_id
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub pixel_format: u32,       // RawPixelFormat enum value
    pub prefer_dmabuf: u8,
}

#[repr(C)]
pub struct EncoderConfigFFI {
    pub backend: *const c_char,  // "v4l2_m2m" or "rdk_x5"
    pub codec: u32,              // VideoCodec enum value
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub profile: *const c_char,  // "42001f"
    pub gop: u32,
    pub prefer_dmabuf: u8,
}

#[repr(C)]
pub struct SourcePipelineConfigFFI {
    pub capture: CaptureConfigFFI,
    pub encoder: EncoderConfigFFI,
    pub payload_type: u32,
    pub clock_rate: u32,
}

// ---------------------------------------------------------------------------
// EncodedPacketFFI — the only data crossing to Rust
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct EncodedPacketFFI {
    pub codec: u32,          // VideoCodec enum value
    pub data: *const u8,     // valid only during callback
    pub size: usize,
    pub pts_us: u64,
    pub dts_us: u64,
    pub flags: u32,          // EncodedFlags bitmask
}

// ---------------------------------------------------------------------------
// Callback and hooks
// ---------------------------------------------------------------------------

pub type EncodedPacketCallbackFFI =
    unsafe extern "C" fn(packet: *const EncodedPacketFFI, user_data: *mut c_void);

#[repr(C)]
pub struct SourcePipelineHooksFFI {
    pub on_packet: Option<EncodedPacketCallbackFFI>,
    pub user_data: *mut c_void,
}

// ---------------------------------------------------------------------------
// Opaque handle
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct SourcePipelineHandle {
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

    pub fn source_pipeline_start(h: *mut SourcePipelineHandle) -> bool;
    pub fn source_pipeline_stop(h: *mut SourcePipelineHandle);
    pub fn source_pipeline_is_running(h: *mut SourcePipelineHandle) -> bool;
    pub fn source_pipeline_request_keyframe(h: *mut SourcePipelineHandle);
    pub fn source_pipeline_free(h: *mut SourcePipelineHandle);
}
