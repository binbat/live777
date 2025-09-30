#![allow(non_camel_case_types, dead_code)]
use libc::{c_char, c_int, c_uchar, c_void};

pub type TDL_RTSP_Handle = *mut c_void;

pub const TDL_RTSP_OK: c_int = 0;
pub const TDL_RTSP_ERR_GENERAL: c_int = -1;
pub const TDL_RTSP_ERR_PARAM: c_int = -2;
pub const TDL_RTSP_ERR_STATE: c_int = -3;
pub const TDL_RTSP_ERR_INIT: c_int = -4;
pub const TDL_STREAM_ERR_TIMEOUT: c_int = -4;
pub const TDL_STREAM_ERR_BUF_SMALL: c_int = -5;

#[repr(C)]
#[derive(Debug)]
pub struct TDL_RTSP_Params {
    pub rtsp_port: u16,
    pub enc_width: u32,
    pub enc_height: u32,
    pub framerate: u32,
    pub vb_blk_count: u32,
    pub vb_bind: c_uchar,
    pub codec: *const c_char,
    pub ring_capacity: u32,
}

#[cfg(riscv_mode)]
#[link(name = "milkv_stream")]
unsafe extern "C" {
    pub fn tdl_stream_start_encoded(
        params: *const TDL_RTSP_Params,
        out_handle: *mut TDL_RTSP_Handle,
    ) -> c_int;
    pub fn tdl_rtsp_is_running(handle: TDL_RTSP_Handle) -> c_int;
    pub fn tdl_rtsp_last_error(handle: TDL_RTSP_Handle) -> *const c_char;
    pub fn tdl_rtsp_stop(handle: TDL_RTSP_Handle);
    pub fn tdl_rtsp_destroy(handle: TDL_RTSP_Handle);
    pub fn tdl_stream_get_frame(
        handle: TDL_RTSP_Handle,
        buf: *mut u8,
        inout_size: *mut u32,
        timeout_ms: c_int,
        pts: *mut u64,
        is_key: *mut c_int,
    ) -> c_int;
    pub fn tdl_stream_get_drop_count(handle: TDL_RTSP_Handle) -> u64;
}
