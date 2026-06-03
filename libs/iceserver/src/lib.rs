#[cfg(feature = "cloudflare")]
pub mod cloudflare;

#[cfg(feature = "coturn")]
pub mod coturn;

use rtc::peer_connection::transport::RTCIceServer;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use tracing::warn;
use webrtc::error::Error;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct IceServer {
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub credential: String,
}

impl IceServer {
    pub(crate) fn parse_url(&self, url_str: &str) -> webrtc::error::Result<rtc_ice::url::Url> {
        rtc_ice::url::Url::parse_url(url_str)
    }

    pub fn validate(&self) -> webrtc::error::Result<()> {
        self.urls()?;
        Ok(())
    }

    fn urls(&self) -> webrtc::error::Result<Vec<rtc_ice::url::Url>> {
        let mut urls = vec![];

        for url_str in &self.urls {
            let mut url = self.parse_url(url_str)?;
            if url.scheme == rtc_ice::url::SchemeType::Turn
                || url.scheme == rtc_ice::url::SchemeType::Turns
            {
                // https://www.w3.org/TR/webrtc/#set-the-configuration (step #11.3.2)
                if self.username.is_empty() || self.credential.is_empty() {
                    return Err(Error::ErrNoTurnCredentials);
                }
                url.username.clone_from(&self.username);
                url.password.clone_from(&self.credential);
            }

            urls.push(url);
        }

        Ok(urls)
    }

    /// Parse, validate, and normalize all URLs. Invalid URLs are logged and skipped.
    fn normalized_urls(&self) -> Vec<String> {
        let mut result = Vec::new();
        for url_str in &self.urls {
            if url_str.trim().is_empty() {
                warn!("ICE server: skipping empty URL");
                continue;
            }
            match self.parse_url(url_str) {
                Ok(url) => {
                    // Normalize to scheme:host:port format so the ICE agent
                    // receives a consistent address (e.g. stun:host:3478).
                    result.push(url.to_string());
                }
                Err(e) => {
                    warn!("ICE server: skipping invalid URL '{url_str}': {e}");
                }
            }
        }
        result
    }

    fn rtc_urls(&self) -> Vec<String> {
        self.normalized_urls()
            .into_iter()
            .filter(|url_str| match self.parse_url(url_str) {
                Ok(url) if ice_server_host_resolves_to_benchmarking_ip(&url) => {
                    warn!(
                        "ICE server: skipping URL '{url_str}' because host resolves to 198.18.0.0/15, likely a proxy/VPN fake-ip address"
                    );
                    false
                }
                _ => true,
            })
            .collect()
    }
}

fn ice_server_host_resolves_to_benchmarking_ip(url: &rtc_ice::url::Url) -> bool {
    match (url.host.as_str(), url.port).to_socket_addrs() {
        Ok(addrs) => {
            let addrs = addrs.collect::<Vec<_>>();
            !addrs.is_empty() && addrs.iter().all(|addr| is_benchmarking_ip(addr.ip()))
        }
        Err(_) => false,
    }
}

fn is_benchmarking_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_benchmarking_ipv4(ip),
        IpAddr::V6(_) => false,
    }
}

fn is_benchmarking_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
}

impl From<IceServer> for RTCIceServer {
    fn from(val: IceServer) -> Self {
        let urls = val.rtc_urls();
        RTCIceServer {
            urls,
            username: val.username,
            credential: val.credential,
        }
    }
}

pub fn format_iceserver(urls: Vec<String>, username: String, password: String) -> IceServer {
    IceServer {
        urls,
        username,
        credential: password,
    }
}

pub fn default_ice_servers() -> Vec<IceServer> {
    vec![IceServer {
        urls: vec!["stun:stun.l.google.com:19302".to_string()],
        username: "".to_string(),
        credential: "".to_string(),
    }]
}

pub fn link_header(ice_servers: Vec<IceServer>) -> Vec<String> {
    ice_servers
        .into_iter()
        .flat_map(|server| {
            let mut username = server.username;
            let mut credential = server.credential;
            if !username.is_empty() {
                username = string_encoder(&username);
                credential = string_encoder(&credential);
            }
            server.urls.into_iter().map(move |url| {
                let mut link = format!("<{url}>; rel=\"ice-server\"");
                if !username.is_empty() {
                    link = format!(
                        "{}; username=\"{}\"; credential=\"{}\"",
                        link, username, credential
                    );
                }
                link
            })
        })
        .collect()
}

fn string_encoder(s: &impl ToString) -> String {
    let s = serde_json::to_string(&s.to_string()).unwrap();
    s[1..s.len() - 1].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- validate() ---

    #[test]
    fn validate_stun_with_port() {
        let s = IceServer {
            urls: vec!["stun:stun.l.google.com:19302".into()],
            ..Default::default()
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_stun_without_port() {
        let s = IceServer {
            urls: vec!["stun:stun.22333.fun".into()],
            ..Default::default()
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_turn_with_credentials() {
        let s = IceServer {
            urls: vec!["turn:turn.example.com:3478".into()],
            username: "user".into(),
            credential: "pass".into(),
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn validate_turn_without_credentials_fails() {
        let s = IceServer {
            urls: vec!["turn:turn.example.com:3478".into()],
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn validate_invalid_scheme_fails() {
        let s = IceServer {
            urls: vec!["http:example.com".into()],
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn validate_double_slash_fails() {
        let s = IceServer {
            urls: vec!["stun://stun.l.google.com:19302".into()],
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    // --- Default ports ---

    #[test]
    fn stun_default_port_3478() {
        let urls = IceServer {
            urls: vec!["stun:stun.example.com".into()],
            ..Default::default()
        }
        .normalized_urls();
        assert_eq!(urls, vec!["stun:stun.example.com:3478"]);
    }

    #[test]
    fn turn_default_port_3478() {
        let urls = IceServer {
            urls: vec!["turn:turn.example.com".into()],
            username: "u".into(),
            credential: "c".into(),
        }
        .normalized_urls();
        assert_eq!(urls, vec!["turn:turn.example.com:3478?transport=udp"]);
    }

    #[test]
    fn stuns_default_port_5349() {
        let urls = IceServer {
            urls: vec!["stuns:stun.example.com".into()],
            ..Default::default()
        }
        .normalized_urls();
        assert_eq!(urls, vec!["stuns:stun.example.com:5349"]);
    }

    #[test]
    fn turns_default_port_5349() {
        let urls = IceServer {
            urls: vec!["turns:turn.example.com".into()],
            username: "u".into(),
            credential: "c".into(),
        }
        .normalized_urls();
        assert_eq!(urls, vec!["turns:turn.example.com:5349?transport=tcp"]);
    }

    // --- TURN transport query preservation ---

    #[test]
    fn turn_transport_udp_preserved() {
        let urls = IceServer {
            urls: vec!["turn:turn.example.com:3478?transport=udp".into()],
            username: "u".into(),
            credential: "c".into(),
        }
        .normalized_urls();
        assert_eq!(urls, vec!["turn:turn.example.com:3478?transport=udp"]);
    }

    #[test]
    fn turn_transport_tcp_preserved() {
        let urls = IceServer {
            urls: vec!["turn:turn.example.com:3478?transport=tcp".into()],
            username: "u".into(),
            credential: "c".into(),
        }
        .normalized_urls();
        assert_eq!(urls, vec!["turn:turn.example.com:3478?transport=tcp"]);
    }

    #[test]
    fn turns_transport_tcp_preserved() {
        let urls = IceServer {
            urls: vec!["turns:turn.example.com?transport=tcp".into()],
            username: "u".into(),
            credential: "c".into(),
        }
        .normalized_urls();
        assert_eq!(urls, vec!["turns:turn.example.com:5349?transport=tcp"]);
    }

    // --- IPv6 bracket preservation ---

    #[test]
    fn stun_ipv6_loopback() {
        let urls = IceServer {
            urls: vec!["stun:[::1]:3478".into()],
            ..Default::default()
        }
        .normalized_urls();
        assert_eq!(urls, vec!["stun:[::1]:3478"]);
    }

    #[test]
    fn turn_ipv6_global() {
        let urls = IceServer {
            urls: vec!["turn:[2001:db8::1]:3478?transport=udp".into()],
            username: "u".into(),
            credential: "c".into(),
        }
        .normalized_urls();
        assert_eq!(urls, vec!["turn:[2001:db8::1]:3478?transport=udp"]);
    }

    #[test]
    fn stun_ipv6_default_port() {
        let urls = IceServer {
            urls: vec!["stun:[::1]".into()],
            ..Default::default()
        }
        .normalized_urls();
        assert_eq!(urls, vec!["stun:[::1]:3478"]);
    }

    // --- Filter invalid URLs ---

    #[test]
    fn normalized_urls_skips_invalid() {
        let server = IceServer {
            urls: vec![
                "stun:stun.l.google.com:19302".into(),
                "stun://invalid.example.com".into(),
                "".into(),
                "stun:stun.22333.fun".into(),
            ],
            ..Default::default()
        };
        let urls = server.normalized_urls();
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "stun:stun.l.google.com:19302");
        assert_eq!(urls[1], "stun:stun.22333.fun:3478");
    }

    #[test]
    fn normalized_urls_skips_benchmarking_fake_ip() {
        let server = IceServer {
            urls: vec![
                "stun:198.18.0.19:19302".into(),
                "stun:stun.example.com:3478".into(),
            ],
            ..Default::default()
        };
        let urls = server.normalized_urls();
        assert_eq!(
            urls,
            vec!["stun:198.18.0.19:19302", "stun:stun.example.com:3478"]
        );
    }

    #[test]
    fn rtc_urls_skip_benchmarking_fake_ip() {
        let server = IceServer {
            urls: vec![
                "stun:198.18.0.19:19302".into(),
                "stun:203.0.113.1:3478".into(),
            ],
            ..Default::default()
        };
        let urls = server.rtc_urls();
        assert_eq!(urls, vec!["stun:203.0.113.1:3478"]);
    }

    #[test]
    fn empty_urls_produces_empty_result() {
        let server = IceServer {
            urls: vec![],
            ..Default::default()
        };
        assert!(server.normalized_urls().is_empty());
    }

    // --- Credential passthrough ---

    #[test]
    fn from_ice_server_preserves_credentials() {
        let server = IceServer {
            urls: vec!["turn:203.0.113.1:3478?transport=tcp".into()],
            username: "myuser".into(),
            credential: "mysecret".into(),
        };
        let rtc: RTCIceServer = server.into();
        assert_eq!(rtc.urls, vec!["turn:203.0.113.1:3478?transport=tcp"]);
        assert_eq!(rtc.username, "myuser");
        assert_eq!(rtc.credential, "mysecret");
    }

    #[test]
    fn from_ice_server_normalizes_urls() {
        let server = IceServer {
            urls: vec!["stun:203.0.113.1".into(), "stun:203.0.113.2:19302".into()],
            ..Default::default()
        };
        let rtc: RTCIceServer = server.into();
        assert_eq!(rtc.urls.len(), 2);
        assert_eq!(rtc.urls[0], "stun:203.0.113.1:3478");
        assert_eq!(rtc.urls[1], "stun:203.0.113.2:19302");
        assert!(rtc.username.is_empty());
        assert!(rtc.credential.is_empty());
    }
}
