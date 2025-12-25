pub use rtsp_types::{Message, Method, Request, Response, StatusCode, Url, Version};

pub mod headers {
    pub use rtsp_types::headers::*;
}

pub mod channels;
pub mod client;
pub mod constants;
pub mod sdp;
pub mod server;
pub mod tcp_stream;
pub mod transport_manager;
pub mod types;

pub use channels::{InterleavedChannel, InterleavedData, RtspChannels};
pub use client::{AuthParams, RtspMode, RtspSession, setup_rtsp_session};
pub use constants::{
    buffer, client as client_constants, media_type, method_str, server as server_constants, track,
    transport,
};
pub use sdp::{
    extract_h264_params, extract_h265_params, filter_sdp, parse_codecs_from_sdp,
    parse_media_info_from_sdp,
};
pub use server::{
    Handler, PortUpdate, RtspServer, ServerConfig, ServerSession, setup_rtsp_server_session,
};
pub use transport_manager::{TransportConfig, TransportManager, UdpPortInfo, UdpSocketPair};
pub use types::{
    AudioCodecParams, CodecInfo, MediaInfo, SessionMode, TransportInfo, VideoCodecParams,
};

pub mod prelude {
    pub use crate::{
        AudioCodecParams, AuthParams, CodecInfo, Handler, InterleavedChannel, InterleavedData,
        MediaInfo, Message, Method, PortUpdate, Request, Response, RtspChannels, RtspMode,
        RtspServer, RtspSession, ServerConfig, ServerSession, SessionMode, StatusCode,
        TransportConfig, TransportInfo, TransportManager, UdpPortInfo, Url, Version,
        VideoCodecParams,
        constants::{buffer, media_type},
        extract_h264_params, extract_h265_params, filter_sdp, headers, parse_media_info_from_sdp,
        setup_rtsp_server_session, setup_rtsp_session,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_mode() {
        let push = SessionMode::Push;
        let pull = SessionMode::Pull;
        assert_ne!(push, pull);
    }

    #[test]
    fn test_method_conversion() {
        let method = Method::Describe;
        let method_str: &str = (&method).into();
        assert_eq!(method_str, "DESCRIBE");
    }

    #[test]
    fn test_rtsp_mode_conversion() {
        let pull_mode = RtspMode::Pull;
        let session_mode = pull_mode.to_session_mode();
        assert_eq!(session_mode, SessionMode::Pull);
    }
}
