pub use rtsp_types::{Message, Method, Request, Response, StatusCode, Url, Version};

pub mod headers {
    pub use rtsp_types::headers::*;
}

pub mod channels;
#[cfg(feature = "client")]
pub mod client;
pub mod constants;
pub mod sdp;
#[cfg(feature = "server")]
pub mod server;
pub mod tcp_stream;
pub mod transport_manager;
pub mod types;

pub use channels::{InterleavedChannel, InterleavedData, RtspChannels};
#[cfg(feature = "client")]
pub use client::{AuthParams, RtspMode, RtspSession, setup_rtsp_session};
pub use constants::{
    buffer, client as client_constants, media_type, method_str, server as server_constants, track,
    transport,
};
pub use sdp::{
    extract_h264_params, extract_h265_params, filter_sdp, parse_codecs_from_sdp,
    parse_media_info_from_sdp,
};
#[cfg(feature = "server")]
pub use server::{
    Handler, PortUpdate, RtspServer, ServerConfig, ServerSession, SessionEndpoint, SessionHandler,
    setup_rtsp_server_with_handler,
};
pub use transport_manager::{TransportConfig, TransportManager, UdpPortInfo, UdpSocketPair};
pub use types::{
    AudioCodecParams, CodecFingerprint, CodecInfo, MediaInfo, MediaKind, MediaProfile, SessionMode,
    TransportInfo, VideoCodecParams, video_rtcp_feedback,
};

pub mod prelude {
    pub use crate::{
        AudioCodecParams, CodecFingerprint, CodecInfo, InterleavedChannel, InterleavedData,
        MediaInfo, MediaKind, MediaProfile, Message, Method, Request, Response, RtspChannels,
        SessionMode, StatusCode, TransportConfig, TransportInfo, TransportManager, UdpPortInfo,
        Url, Version, VideoCodecParams,
        constants::{buffer, media_type},
        extract_h264_params, extract_h265_params, filter_sdp, headers, parse_media_info_from_sdp,
    };
    #[cfg(feature = "client")]
    pub use crate::{AuthParams, RtspMode, RtspSession, setup_rtsp_session};
    #[cfg(feature = "server")]
    pub use crate::{
        Handler, PortUpdate, RtspServer, ServerConfig, ServerSession, SessionEndpoint,
        SessionHandler, setup_rtsp_server_with_handler,
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
    #[cfg(feature = "client")]
    fn test_rtsp_mode_conversion() {
        let pull_mode = RtspMode::Pull;
        let session_mode = pull_mode.to_session_mode();
        assert_eq!(session_mode, SessionMode::Pull);
    }
}
