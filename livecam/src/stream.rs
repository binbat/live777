use crate::ffi::*;
use std::ffi::CStr;
use std::ptr;

pub struct StreamHandle {
    raw: TDL_RTSP_Handle,
}

unsafe impl Send for StreamHandle {}
unsafe impl Sync for StreamHandle {}

impl StreamHandle {
    pub fn start_encode_only(params: &TDL_RTSP_Params) -> Result<Self, String> {
        unsafe {
            let mut h: TDL_RTSP_Handle = ptr::null_mut();
            let r = tdl_stream_start_encoded(params as *const _, &mut h as *mut _);
            if r != TDL_RTSP_OK || h.is_null() {
                return Err(format!("tdl_stream_start_encoded failed r={}", r));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            if tdl_rtsp_is_running(h) != 1 {
                let err_msg = CStr::from_ptr(tdl_rtsp_last_error(h))
                    .to_string_lossy()
                    .into_owned();
                tdl_rtsp_destroy(h);
                return Err(format!("tdl_rtsp_is_running check failed: {}", err_msg));
            }
            Ok(Self { raw: h })
        }
    }

    pub fn get_encoded_frame(
        &self,
        timeout_ms: i32,
    ) -> Result<Option<(Vec<u8>, u64, bool)>, String> {
        unsafe {
            let mut need: u32 = 0;
            let mut rc = tdl_stream_get_frame(
                self.raw,
                ptr::null_mut(),
                &mut need,
                timeout_ms,
                ptr::null_mut(),
                ptr::null_mut(),
            );

            if rc == TDL_STREAM_ERR_TIMEOUT {
                return Ok(None);
            }
            if rc == TDL_RTSP_ERR_STATE {
                return Err("Handle stopped or invalid state".into());
            }
            if need == 0 {
                return Ok(None);
            }

            let mut buf = vec![0u8; need as usize];
            let mut size_in = need;
            let mut pts = 0u64;
            let mut is_key_i = 0i32;
            rc = tdl_stream_get_frame(
                self.raw,
                buf.as_mut_ptr(),
                &mut size_in,
                0,
                &mut pts,
                &mut is_key_i,
            );

            if rc == 0 {
                buf.truncate(size_in as usize);
                let is_key = is_key_i != 0;
                Ok(Some((buf, pts, is_key)))
            } else {
                Err(format!("Fetch frame failed rc={}", rc))
            }
        }
    }

    pub fn stop(&self) {
        if !self.raw.is_null() {
            unsafe {
                tdl_rtsp_stop(self.raw);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe {
                tdl_rtsp_destroy(self.raw);
                self.raw = ptr::null_mut();
            }
        }
    }
}
