pub mod payload;
pub mod protocol;
pub mod transport;
pub mod utils;
pub mod whep;
pub mod whip;

#[cfg(test)]
mod test;

pub const PREFIX_LIB: &str = "WEBRTC";
pub const SCHEME_RTSP_SERVER: &str = "rtsp-listen";
pub const SCHEME_RTSP_CLIENT: &str = "rtsp";
pub const SCHEME_RTP_SDP: &str = "sdp";
