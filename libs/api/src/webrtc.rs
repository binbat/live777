use std::net::SocketAddr;

pub const DEFAULT_WEBRTC_ICE_UDP_ADDR: &str = "127.0.0.1:0";
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
        .filter_map(|addr| {
            let addr = addr.trim();
            if addr.is_empty() {
                return None;
            }
            match addr.parse::<SocketAddr>() {
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
            }
        })
        .collect::<Vec<_>>();

    if addrs.is_empty() {
        vec![
            DEFAULT_WEBRTC_ICE_UDP_ADDR
                .parse()
                .expect("default WebRTC ICE UDP addr is valid"),
        ]
    } else {
        addrs
    }
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
    fn defaults_to_loopback_udp_addr_without_config_or_env() {
        with_clean_env(|| {
            assert_eq!(
                resolve_webrtc_ice_udp_addrs(None),
                vec![DEFAULT_WEBRTC_ICE_UDP_ADDR.parse::<SocketAddr>().unwrap()]
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
                vec![DEFAULT_WEBRTC_ICE_UDP_ADDR.parse::<SocketAddr>().unwrap()]
            );
        });
    }

    #[test]
    fn unspecified_ipv4_udp_addr_is_not_returned_as_candidate_addr() {
        with_clean_env(|| {
            let addrs = resolve_webrtc_ice_udp_addrs(Some(vec!["0.0.0.0:0".to_string()]));

            assert_eq!(
                addrs,
                vec![DEFAULT_WEBRTC_ICE_UDP_ADDR.parse::<SocketAddr>().unwrap()]
            );
            assert!(!addrs.iter().any(|addr| addr.ip().is_unspecified()));
        });
    }

    #[test]
    fn unspecified_ipv6_udp_addr_is_not_returned_as_candidate_addr() {
        with_clean_env(|| {
            let addrs = resolve_webrtc_ice_udp_addrs(Some(vec!["[::]:0".to_string()]));

            assert_eq!(
                addrs,
                vec![DEFAULT_WEBRTC_ICE_UDP_ADDR.parse::<SocketAddr>().unwrap()]
            );
            assert!(!addrs.iter().any(|addr| addr.ip().is_unspecified()));
        });
    }
}
