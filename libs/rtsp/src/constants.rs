pub mod media_type {
    pub const VIDEO: &str = "video";

    pub const AUDIO: &str = "audio";

    pub const APPLICATION: &str = "application/sdp";
}

pub mod track {
    pub const VIDEO_TRACK_ID: &str = "trackID=0";

    pub const AUDIO_TRACK_ID: &str = "trackID=1";

    pub const VIDEO_STREAM_ID: &str = "streamid=0";

    pub const AUDIO_STREAM_ID: &str = "streamid=1";
}

pub mod method_str {
    pub const OPTIONS: &str = "OPTIONS";
    pub const DESCRIBE: &str = "DESCRIBE";
    pub const SETUP: &str = "SETUP";
    pub const PLAY: &str = "PLAY";
    pub const RECORD: &str = "RECORD";
    pub const TEARDOWN: &str = "TEARDOWN";
    pub const ANNOUNCE: &str = "ANNOUNCE";
    pub const GET_PARAMETER: &str = "GET_PARAMETER";
    pub const SET_PARAMETER: &str = "SET_PARAMETER";
}

pub mod buffer {
    pub const RTSP_RESPONSE_BUFFER_SIZE: usize = 4096;

    pub const RTSP_REQUEST_BUFFER_SIZE: usize = 8192;

    pub const TCP_READ_BUFFER_SIZE: usize = 64 * 1024;

    pub const MAX_BUFFER_SIZE: usize = 2 * 1024 * 1024;

    pub const MAX_FRAME_SIZE: usize = 1024 * 1024;

    pub const INTERLEAVED_HEADER_SIZE: usize = 4;
}

pub mod transport {
    pub const ETHERNET_MTU: usize = 1500;

    pub const RTP_BUFFER_SIZE: usize = ETHERNET_MTU;

    pub const RTCP_BUFFER_SIZE: usize = ETHERNET_MTU;
}

pub mod client {
    pub const USER_AGENT: &str = "livetwo";

    pub const KEEP_ALIVE_INTERVAL_SECS: u64 = 30;
}

pub mod server {
    pub const DEFAULT_SESSION_TIMEOUT: u64 = 60;

    pub const DEFAULT_MAX_CONNECTIONS: usize = 100;
}

pub mod net {
    use anyhow::{Result, anyhow};
    use std::net::{IpAddr, SocketAddr};
    use url::Url;

    pub fn unspecified_for(addr: &SocketAddr) -> IpAddr {
        match addr {
            SocketAddr::V4(_) => IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            SocketAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
        }
    }

    pub fn unspecified_for_ip(ip: &IpAddr) -> IpAddr {
        match ip {
            IpAddr::V4(_) => IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
        }
    }

    pub fn bind_addr_for(addr: &SocketAddr, port: u16) -> String {
        match addr {
            SocketAddr::V4(_) => format!("0.0.0.0:{}", port),
            SocketAddr::V6(_) => format!("[::]:{}", port),
        }
    }

    pub fn bind_any_for(addr: &SocketAddr) -> String {
        match addr {
            SocketAddr::V4(_) => "0.0.0.0:0".to_string(),
            SocketAddr::V6(_) => "[::]:0".to_string(),
        }
    }

    pub fn extract_ip_from_url(url: &Url) -> Result<IpAddr> {
        match url.host() {
            Some(url::Host::Ipv4(ip)) => Ok(IpAddr::V4(ip)),
            Some(url::Host::Ipv6(ip)) => Ok(IpAddr::V6(ip)),
            Some(url::Host::Domain(domain)) => domain
                .parse::<IpAddr>()
                .map_err(|e| anyhow!("Failed to parse domain '{}' as IP: {}", domain, e)),
            None => Err(anyhow!("No host in URL")),
        }
    }
}
