pub mod payload;
pub mod probe;
pub mod protocol;
pub mod source;
pub mod transport;
pub mod utils;
pub mod whep;
pub mod whip;
#[cfg(feature = "rsmpeg")]
pub mod whipsynth;

#[cfg(test)]
mod test;

pub const PREFIX_LIB: &str = "WEBRTC";
pub const SCHEME_RTSP_CLIENT: &str = "rtsp";
pub const SCHEME_RTP_SDP: &str = "sdp";
pub const SCHEME_SYNTH: &str = "synth";
