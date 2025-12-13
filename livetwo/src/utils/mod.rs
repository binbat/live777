pub mod host;
pub mod shutdown;
pub mod stats;
pub mod webrtc;

use anyhow::Result;
use url::Url;

pub use host::{format_bind_addr, is_ipv4, is_ipv6, parse_host, parse_host_from_sdp};
pub use shutdown::{ShutdownSignal, wait_for_shutdown};
pub use webrtc::{create_api, setup_connection, setup_handlers};

pub fn parse_input_url(target_url: &str) -> Result<Url> {
    Ok(Url::parse(target_url).unwrap_or_else(|_| {
        Url::parse(&format!(
            "{}://{}:0/{}",
            crate::SCHEME_RTP_SDP,
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            target_url
        ))
        .unwrap()
    }))
}
