pub mod channels;
pub mod client;
pub mod sdp;
pub mod server;
pub mod tcp_stream;
pub mod transport_manager;
pub mod types;

pub use channels::RtspChannels;
pub use client::{AuthParams, RtspSession, setup_rtsp_session};
pub use sdp::{extract_h264_params, extract_h265_params, filter_sdp, parse_media_info_from_sdp};
pub use server::{Handler, RtspServer, ServerConfig, ServerSession, setup_rtsp_server_session};
pub use transport_manager::{TransportConfig, TransportManager, UdpPortInfo, UdpSocketPair};
pub use types::{
    AudioCodecParams, CodecInfo, MediaInfo, SessionMode, TransportInfo, VideoCodecParams,
};

pub use client::RtspMode;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_mode() {
        let push = SessionMode::Push;
        let pull = SessionMode::Pull;
        assert_ne!(push, pull);
    }
}
