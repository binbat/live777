pub mod whep;
pub mod whip;

mod payload;
mod rtspclient;

#[cfg(test)]
mod test;

const PREFIX_LIB: &str = "WEBRTC";
const SCHEME_RTSP_SERVER: &str = "rtsp-listen";
const SCHEME_RTSP_CLIENT: &str = "rtsp";
const SCHEME_RTP_SDP: &str = "sdp";
