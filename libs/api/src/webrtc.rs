use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub const DEFAULT_WEBRTC_ICE_UDP_ADDR: &str = "auto";
const LOOPBACK_WEBRTC_ICE_UDP_ADDR: &str = "127.0.0.1:0";
const WEBRTC_ICE_UDP_ADDRS_ENV: &str = "LIVE777_WEBRTC_ICE_UDP_ADDRS";
const LEGACY_WEBRTC_ICE_UDP_ADDR_ENV: &str = "LIVE777_WEBRTC_ICE_UDP_ADDR";
const LEGACY_LIVETWO_WEBRTC_ICE_UDP_ADDR_ENV: &str = "LIVETWO_WEBRTC_ICE_UDP_ADDR";

pub fn resolve_webrtc_ice_udp_addrs(configured: Option<Vec<String>>) -> Vec<SocketAddr> {
    let raw_addrs = std::env::var(WEBRTC_ICE_UDP_ADDRS_ENV)
        .or_else(|_| std::env::var(LEGACY_WEBRTC_ICE_UDP_ADDR_ENV))
        .or_else(|_| std::env::var(LEGACY_LIVETWO_WEBRTC_ICE_UDP_ADDR_ENV))
        .ok()
        .map(|addr| vec![addr])
        .or(configured)
        .unwrap_or_else(|| vec![DEFAULT_WEBRTC_ICE_UDP_ADDR.to_string()]);

    let addrs = raw_addrs
        .iter()
        .flat_map(|value| value.split(','))
        .flat_map(|addr| {
            let addr = addr.trim();
            if addr.is_empty() {
                return Vec::new();
            }
            if addr.eq_ignore_ascii_case("auto") {
                return discover_webrtc_ice_udp_addrs();
            }
            let parsed = match addr.parse::<SocketAddr>() {
                Ok(addr) if addr.ip().is_unspecified() => {
                    tracing::warn!(
                        "Ignoring unspecified WebRTC ICE UDP address '{addr}': unspecified address is not suitable as an ICE candidate address"
                    );
                    None
                }
                Ok(addr) => Some(addr),
                Err(error) => {
                    tracing::warn!("Ignoring invalid WebRTC ICE UDP address '{addr}': {error}");
                    None
                }
            };
            parsed.into_iter().collect()
        })
        .collect::<Vec<_>>();

    if addrs.is_empty() {
        fallback_webrtc_ice_udp_addrs()
    } else {
        addrs
    }
}

fn discover_webrtc_ice_udp_addrs() -> Vec<SocketAddr> {
    let mut addrs = [
        discover_local_addr("8.8.8.8:80"),
        discover_local_addr("[2001:4860:4860::8888]:80"),
    ]
    .into_iter()
    .flatten()
    .filter(|addr| is_usable_auto_candidate_ip(addr.ip()))
    .map(|addr| SocketAddr::new(addr.ip(), 0))
    .collect::<Vec<_>>();

    addrs.sort_unstable();
    addrs.dedup();
    addrs
}

fn discover_local_addr(remote: &str) -> Option<SocketAddr> {
    let remote = remote.parse::<SocketAddr>().ok()?;
    let bind_addr = if remote.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = std::net::UdpSocket::bind(bind_addr).ok()?;
    socket.connect(remote).ok()?;
    socket.local_addr().ok()
}

fn is_usable_auto_candidate_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            !ip.is_unspecified()
                && !ip.is_loopback()
                && !ip.is_multicast()
                && !is_benchmarking_ipv4(ip)
        }
        IpAddr::V6(ip) => !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast(),
    }
}

fn is_benchmarking_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
}

fn fallback_webrtc_ice_udp_addrs() -> Vec<SocketAddr> {
    tracing::warn!(
        "No non-loopback WebRTC ICE UDP address discovered; falling back to {LOOPBACK_WEBRTC_ICE_UDP_ADDR}. Remote browsers may fail unless ice_udp_addrs is configured explicitly."
    );
    vec![
        LOOPBACK_WEBRTC_ICE_UDP_ADDR
            .parse()
            .expect("loopback WebRTC ICE UDP addr is valid"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_clean_env(test: impl FnOnce()) {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_addrs = std::env::var(WEBRTC_ICE_UDP_ADDRS_ENV).ok();
        let old_addr = std::env::var(LEGACY_WEBRTC_ICE_UDP_ADDR_ENV).ok();
        let old_livetwo_addr = std::env::var(LEGACY_LIVETWO_WEBRTC_ICE_UDP_ADDR_ENV).ok();

        unsafe {
            std::env::remove_var(WEBRTC_ICE_UDP_ADDRS_ENV);
            std::env::remove_var(LEGACY_WEBRTC_ICE_UDP_ADDR_ENV);
            std::env::remove_var(LEGACY_LIVETWO_WEBRTC_ICE_UDP_ADDR_ENV);
        }

        test();

        unsafe {
            match old_addrs {
                Some(value) => std::env::set_var(WEBRTC_ICE_UDP_ADDRS_ENV, value),
                None => std::env::remove_var(WEBRTC_ICE_UDP_ADDRS_ENV),
            }
            match old_addr {
                Some(value) => std::env::set_var(LEGACY_WEBRTC_ICE_UDP_ADDR_ENV, value),
                None => std::env::remove_var(LEGACY_WEBRTC_ICE_UDP_ADDR_ENV),
            }
            match old_livetwo_addr {
                Some(value) => std::env::set_var(LEGACY_LIVETWO_WEBRTC_ICE_UDP_ADDR_ENV, value),
                None => std::env::remove_var(LEGACY_LIVETWO_WEBRTC_ICE_UDP_ADDR_ENV),
            }
        }
    }

    #[test]
    fn defaults_to_auto_without_benchmarking_udp_addr_without_config_or_env() {
        with_clean_env(|| {
            let addrs = resolve_webrtc_ice_udp_addrs(None);
            assert!(
                addrs
                    .iter()
                    .all(|addr| !matches!(addr.ip(), IpAddr::V4(ip) if is_benchmarking_ipv4(ip))),
                "default WebRTC ICE UDP addrs must not advertise benchmarking candidates: {addrs:?}"
            );
        });
    }

    #[test]
    fn uses_configured_udp_addr_without_env() {
        with_clean_env(|| {
            assert_eq!(
                resolve_webrtc_ice_udp_addrs(Some(vec!["127.0.0.1:0".to_string()])),
                vec!["127.0.0.1:0".parse::<SocketAddr>().unwrap()]
            );
        });
    }

    #[test]
    fn env_overrides_configured_udp_addr() {
        with_clean_env(|| {
            unsafe {
                std::env::set_var(WEBRTC_ICE_UDP_ADDRS_ENV, "127.0.0.1:0");
            }

            assert_eq!(
                resolve_webrtc_ice_udp_addrs(Some(vec!["192.168.1.10:0".to_string()])),
                vec!["127.0.0.1:0".parse::<SocketAddr>().unwrap()]
            );
        });
    }

    #[test]
    fn parses_multiple_udp_addrs_from_comma_separated_env() {
        with_clean_env(|| {
            unsafe {
                std::env::set_var(WEBRTC_ICE_UDP_ADDRS_ENV, "127.0.0.1:0,192.168.1.10:0");
            }

            assert_eq!(
                resolve_webrtc_ice_udp_addrs(None),
                vec![
                    "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
                    "192.168.1.10:0".parse::<SocketAddr>().unwrap(),
                ]
            );
        });
    }

    #[test]
    fn skips_invalid_udp_addrs_and_keeps_valid_entries() {
        with_clean_env(|| {
            assert_eq!(
                resolve_webrtc_ice_udp_addrs(Some(vec![
                    "not-a-socket".to_string(),
                    "127.0.0.1:0".to_string(),
                ])),
                vec!["127.0.0.1:0".parse::<SocketAddr>().unwrap()]
            );
        });
    }

    #[test]
    fn falls_back_to_default_when_all_udp_addrs_are_invalid() {
        with_clean_env(|| {
            assert_eq!(
                resolve_webrtc_ice_udp_addrs(Some(vec!["not-a-socket".to_string()])),
                fallback_webrtc_ice_udp_addrs()
            );
        });
    }

    #[test]
    fn unspecified_ipv4_udp_addr_is_not_returned_as_candidate_addr() {
        with_clean_env(|| {
            let addrs = resolve_webrtc_ice_udp_addrs(Some(vec!["0.0.0.0:0".to_string()]));

            assert_eq!(addrs, fallback_webrtc_ice_udp_addrs());
            assert!(!addrs.iter().any(|addr| addr.ip().is_unspecified()));
        });
    }

    #[test]
    fn unspecified_ipv6_udp_addr_is_not_returned_as_candidate_addr() {
        with_clean_env(|| {
            let addrs = resolve_webrtc_ice_udp_addrs(Some(vec!["[::]:0".to_string()]));

            assert_eq!(addrs, fallback_webrtc_ice_udp_addrs());
            assert!(!addrs.iter().any(|addr| addr.ip().is_unspecified()));
        });
    }

    #[test]
    fn benchmarking_ipv4_addr_is_not_usable_auto_candidate() {
        let ip = "198.18.0.1".parse().unwrap();

        assert!(!is_usable_auto_candidate_ip(ip));
    }
}
