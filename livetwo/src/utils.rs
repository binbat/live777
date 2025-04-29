use std::net::{Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use tracing::{error,warn};
use url::{Host, Url};

pub fn parse_host(input: &Url) -> (String, String) {
    let target_host = match input.host() {
        Some(Host::Ipv4(ip)) => ip.to_string(),
        Some(Host::Ipv6(ip)) => ip.to_string(),
        Some(Host::Domain(domain)) => match (domain, 0).to_socket_addrs() {
            Ok(mut addrs) => {
                if let Some(addr) = addrs.find(|addr| addr.is_ipv6()) {
                    addr.ip().to_string()
                } else if let Some(addr) = addrs.find(|addr| addr.is_ipv4()) {
                    addr.ip().to_string()
                } else {
                    warn!(
                        "No valid IP address resolved for domain {}, using default.",
                        domain
                    );
                    Ipv4Addr::LOCALHOST.to_string()
                }
            }
            Err(e) => {
                error!("Failed to resolve domain {}: {}, using default.", domain, e);
                Ipv4Addr::LOCALHOST.to_string()
            }
        },
        None => {
            error!("Invalid host for {}, using default.", input);
            Ipv4Addr::LOCALHOST.to_string()
        }
    };

    let listen_host = if target_host.parse::<Ipv6Addr>().is_ok() {
        Ipv6Addr::UNSPECIFIED.to_string()
    } else {
        Ipv4Addr::UNSPECIFIED.to_string()
    };

    (target_host, listen_host)
}
