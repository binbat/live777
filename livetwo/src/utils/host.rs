use std::net::{Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::str::FromStr;
use tracing::{debug, error, warn};
use url::{Host, Url};

pub fn parse_host(input: &Url) -> (String, String) {
    let target_host = extract_target_host(input);
    let listen_host = derive_listen_host(&target_host);

    debug!(
        "Host parsed - target: {}, listen: {}",
        target_host, listen_host
    );

    (target_host, listen_host)
}

fn extract_target_host(input: &Url) -> String {
    match input.host() {
        Some(Host::Ipv4(ip)) => {
            debug!("Detected IPv4 address: {}", ip);
            ip.to_string()
        }
        Some(Host::Ipv6(ip)) => {
            debug!("Detected IPv6 address: {}", ip);
            ip.to_string()
        }
        Some(Host::Domain(domain)) => {
            debug!("Resolving domain: {}", domain);
            resolve_domain(domain)
        }
        None => {
            warn!("No host in URL: {}, using IPv4 localhost", input);
            Ipv4Addr::LOCALHOST.to_string()
        }
    }
}

fn resolve_domain(domain: &str) -> String {
    if let Ok(ip) = Ipv6Addr::from_str(domain) {
        debug!("Domain is IPv6 address: {}", ip);
        return ip.to_string();
    }
    if let Ok(ip) = Ipv4Addr::from_str(domain) {
        debug!("Domain is IPv4 address: {}", ip);
        return ip.to_string();
    }

    match (domain, 0).to_socket_addrs() {
        Ok(mut addrs) => {
            if let Some(addr) = addrs.find(|addr| addr.is_ipv6()) {
                debug!("Resolved {} to IPv6: {}", domain, addr.ip());
                return addr.ip().to_string();
            }

            if let Some(addr) = (domain, 0)
                .to_socket_addrs()
                .ok()
                .and_then(|mut addrs| addrs.find(|addr| addr.is_ipv4()))
            {
                debug!("Resolved {} to IPv4: {}", domain, addr.ip());
                return addr.ip().to_string();
            }

            warn!("No valid IP resolved for {}, using IPv4 localhost", domain);
            Ipv4Addr::LOCALHOST.to_string()
        }
        Err(e) => {
            error!("Failed to resolve {}: {}, using IPv4 localhost", domain, e);
            Ipv4Addr::LOCALHOST.to_string()
        }
    }
}

pub fn derive_listen_host(target_host: &str) -> String {
    if target_host.parse::<Ipv6Addr>().is_ok() {
        debug!("Target is IPv6, using :: for listening");
        Ipv6Addr::UNSPECIFIED.to_string()
    } else {
        debug!("Target is IPv4, using 0.0.0.0 for listening");
        Ipv4Addr::UNSPECIFIED.to_string()
    }
}

pub fn parse_host_from_sdp(connection_address: &str) -> (String, String) {
    debug!(
        "Parsing host from SDP connection address: {}",
        connection_address
    );

    let target_host = if let Ok(ip) = Ipv6Addr::from_str(connection_address) {
        debug!("SDP contains IPv6 address: {}", ip);
        ip.to_string()
    } else if let Ok(ip) = Ipv4Addr::from_str(connection_address) {
        debug!("SDP contains IPv4 address: {}", ip);
        ip.to_string()
    } else {
        warn!(
            "Invalid IP in SDP: {}, using IPv4 localhost",
            connection_address
        );
        Ipv4Addr::LOCALHOST.to_string()
    };

    let listen_host = derive_listen_host(&target_host);

    debug!(
        "SDP host parsed - target: {}, listen: {}",
        target_host, listen_host
    );

    (target_host, listen_host)
}

pub fn is_ipv6(addr: &str) -> bool {
    addr.parse::<Ipv6Addr>().is_ok()
}

pub fn is_ipv4(addr: &str) -> bool {
    addr.parse::<Ipv4Addr>().is_ok()
}

pub fn format_bind_addr(host: &str, port: u16) -> String {
    if is_ipv6(host) {
        format!("[{}]:{}", host, port)
    } else {
        format!("{}:{}", host, port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_host_ipv4() {
        let url = Url::parse("rtsp://192.168.1.1:8554/stream").unwrap();
        let (target_host, listen_host) = parse_host(&url);
        assert_eq!(target_host, "192.168.1.1");
        assert_eq!(listen_host, "0.0.0.0");
    }

    #[test]
    fn test_parse_host_ipv6() {
        let url = Url::parse("rtsp://[2001:db8::1]:8554/stream").unwrap();
        let (target_host, listen_host) = parse_host(&url);
        assert_eq!(target_host, "2001:db8::1");
        assert_eq!(listen_host, "::");
    }

    #[test]
    fn test_parse_host_ipv6_localhost() {
        let url = Url::parse("rtsp://[::1]:8554/stream").unwrap();
        let (target_host, listen_host) = parse_host(&url);
        assert_eq!(target_host, "::1");
        assert_eq!(listen_host, "::");
    }

    #[test]
    fn test_parse_host_domain() {
        let url = Url::parse("rtsp://localhost:8554/stream").unwrap();
        let (target_host, listen_host) = parse_host(&url);

        let is_ipv6 = target_host.parse::<Ipv6Addr>().is_ok();
        let is_ipv4 = target_host.parse::<Ipv4Addr>().is_ok();

        assert!(is_ipv6 || is_ipv4);

        if is_ipv6 {
            assert_eq!(listen_host, "::");
        } else {
            assert_eq!(listen_host, "0.0.0.0");
        }
    }

    #[test]
    fn test_parse_host_from_sdp_ipv4() {
        let (target, listen) = parse_host_from_sdp("192.168.1.100");
        assert_eq!(target, "192.168.1.100");
        assert_eq!(listen, "0.0.0.0");
    }

    #[test]
    fn test_parse_host_from_sdp_ipv6() {
        let (target, listen) = parse_host_from_sdp("2001:db8::1");
        assert_eq!(target, "2001:db8::1");
        assert_eq!(listen, "::");
    }

    #[test]
    fn test_format_bind_addr_ipv4() {
        assert_eq!(format_bind_addr("0.0.0.0", 8554), "0.0.0.0:8554");
        assert_eq!(format_bind_addr("192.168.1.1", 8554), "192.168.1.1:8554");
    }

    #[test]
    fn test_format_bind_addr_ipv6() {
        assert_eq!(format_bind_addr("::", 8554), "[::]:8554");
        assert_eq!(format_bind_addr("::1", 8554), "[::1]:8554");
        assert_eq!(format_bind_addr("2001:db8::1", 8554), "[2001:db8::1]:8554");
    }
}
