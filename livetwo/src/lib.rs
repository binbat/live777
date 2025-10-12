pub mod whep;
pub mod whip;

pub mod payload;
mod rtspclient;
mod utils;

#[cfg(test)]
mod test;

const PREFIX_LIB: &str = "WEBRTC";
pub const SCHEME_RTSP_SERVER: &str = "rtsp-listen";
pub const SCHEME_RTSP_CLIENT: &str = "rtsp";
pub const SCHEME_RTP_SDP: &str = "sdp";
