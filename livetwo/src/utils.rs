use anyhow::{anyhow, Result};
use std::net::{Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, warn};
use url::{Host, Url};
use webrtc::{
    api::{interceptor_registry::register_default_interceptors, media_engine::*, APIBuilder},
    ice_transport::ice_credential_type::RTCIceCredentialType,
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        RTCPeerConnection,
    },
    rtcp,
};

use libwish::Client;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

pub fn parse_host(input: &Url) -> (String, String) {
    let target_host = match input.host() {
        Some(Host::Ipv4(ip)) => ip.to_string(),
        Some(Host::Ipv6(ip)) => ip.to_string(),
        Some(Host::Domain(domain)) => {
            if let Ok(ip) = Ipv6Addr::from_str(domain) {
                ip.to_string()
            } else if let Ok(ip) = Ipv4Addr::from_str(domain) {
                ip.to_string()
            } else {
                match (domain, 0).to_socket_addrs() {
                    Ok(mut addrs) => {
                        if let Some(addr) = addrs.find(|addr| addr.is_ipv6()) {
                            addr.ip().to_string()
                        } else if let Some(addr) = addrs.find(|addr| addr.is_ipv4()) {
                            addr.ip().to_string()
                        } else {
                            warn!(
                                "[UTILS] No valid IP address resolved for domain {}, using default",
                                domain
                            );
                            Ipv4Addr::LOCALHOST.to_string()
                        }
                    }
                    Err(e) => {
                        error!(
                            "[UTILS] Failed to resolve domain {}: {}, using default",
                            domain, e
                        );
                        Ipv4Addr::LOCALHOST.to_string()
                    }
                }
            }
        }
        None => {
            error!("[UTILS] Invalid host for {}, using default", input);
            Ipv4Addr::LOCALHOST.to_string()
        }
    };

    let listen_host = if target_host.parse::<Ipv6Addr>().is_ok() {
        Ipv6Addr::UNSPECIFIED.to_string()
    } else {
        Ipv4Addr::UNSPECIFIED.to_string()
    };

    info!(
        "[UTILS] Host parsed - target: {}, listen: {}",
        target_host, listen_host
    );
    (target_host, listen_host)
}

pub async fn setup_webrtc_connection(
    peer: Arc<RTCPeerConnection>,
    client: &mut Client,
) -> Result<RTCSessionDescription> {
    info!("[UTILS] Creating WebRTC offer");
    let offer = peer.create_offer(None).await?;
    debug!("[UTILS] WebRTC offer created:{:?}", offer);

    let mut gather_complete = peer.gathering_complete_promise().await;
    peer.set_local_description(offer).await?;
    let _ = gather_complete.recv().await;
    info!("[UTILS] WebRTC offer created and local description set");

    let (answer, ice_servers) = client
        .wish(peer.local_description().await.unwrap().sdp)
        .await?;
    debug!("[UTILS] ICE servers from response: {:?}", ice_servers);

    let mut current_config = peer.get_configuration().await;
    current_config.ice_servers.clone_from(&ice_servers);
    peer.set_configuration(current_config.clone()).await?;
    info!("[UTILS] ICE configuration updated");

    peer.set_remote_description(answer.clone())
        .await
        .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;
    info!("[UTILS] Remote description set successfully");

    Ok(answer)
}

pub async fn create_webrtc_api() -> Result<(APIBuilder, RTCConfiguration)> {
    info!("[UTILS] Creating WebRTC API");
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;
    info!("[UTILS] Default codecs registered");

    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;
    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry);
    info!("[UTILS] API builder created with interceptors");

    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            username: "".to_string(),
            credential: "".to_string(),
            credential_type: RTCIceCredentialType::Unspecified,
        }],
        ..Default::default()
    };
    info!("[UTILS] Default ICE configuration created");

    Ok((api, config))
}

pub async fn setup_peer_connection_handlers(
    peer: Arc<RTCPeerConnection>,
    complete_tx: UnboundedSender<()>,
) {
    info!("[UTILS] Setting up peer connection handlers");
    let pc = peer.clone();
    peer.on_peer_connection_state_change(Box::new(move |s| {
        let pc = pc.clone();
        let complete_tx = complete_tx.clone();
        tokio::spawn(async move {
            warn!("[UTILS] Connection state changed: {}", s);
            match s {
                RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                    let _ = pc.close().await;
                    warn!("[UTILS] Connection closed due to failure or disconnection");
                }
                RTCPeerConnectionState::Closed => {
                    let _ = complete_tx.send(());
                    info!("[UTILS] Connection closed normally");
                }
                _ => debug!("[UTILS] Connection state: {}", s),
            };
        });
        Box::pin(async {})
    }));
}

pub async fn rtcp_listener(host: String, rtcp_port: Option<u16>, peer: Arc<RTCPeerConnection>) {
    if let Some(rtcp_port) = rtcp_port {
        let rtcp_listener = match UdpSocket::bind(format!("{}:{}", host, rtcp_port + 1)).await {
            Ok(socket) => {
                info!(
                    "[UTILS] RTCP listener bound to: {}",
                    socket.local_addr().unwrap()
                );
                socket
            }
            Err(e) => {
                error!("[UTILS] Failed to bind RTCP listener: {}", e);
                return;
            }
        };

        let mut rtcp_buf = vec![0u8; 1500];

        loop {
            match rtcp_listener.recv_from(&mut rtcp_buf).await {
                Ok((len, addr)) => {
                    if len > 0 {
                        debug!("[UTILS] Received {} bytes of RTCP data from {}", len, addr);
                        let mut rtcp_data = &rtcp_buf[..len];

                        if let Ok(rtcp_packets) = rtcp::packet::unmarshal(&mut rtcp_data) {
                            for packet in rtcp_packets {
                                debug!("[UTILS] Received RTCP packet from {}: {:?}", addr, packet);
                                if let Err(err) = peer.write_rtcp(&[packet]).await {
                                    warn!("[UTILS] Failed to send RTCP packet: {}", err);
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("[UTILS] Error receiving RTCP data: {}", e);
                }
            }
        }
    }
}

pub async fn rtp_send(
    mut receiver: UnboundedReceiver<Vec<u8>>,
    listen_host: String,
    target_host: String,
    client_port: Option<u16>,
    server_port: Option<u16>,
) {
    if let Some(port) = client_port {
        let server_addr = if let Some(server_port) = server_port {
            format!("{}:{}", listen_host, server_port)
        } else {
            "0.0.0.0:0".to_string()
        };

        let socket = match UdpSocket::bind(&server_addr).await {
            Ok(s) => {
                info!("[UTILS] UDP socket bound to {}", server_addr);
                s
            }
            Err(e) => {
                error!(
                    "[UTILS] Failed to bind UDP socket on {}: {}",
                    server_addr, e
                );
                return;
            }
        };
        let client_addr = format!("{}:{}", target_host, port);
        info!("[UTILS] RTP sender ready to send to {}", client_addr);

        while let Some(data) = receiver.recv().await {
            match socket.send_to(&data, &client_addr).await {
                Ok(_) => {}
                Err(e) => error!("[UTILS] Failed to send data to {}: {}", client_addr, e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use tokio::sync::mpsc;
    use url::Url;
    use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

    #[test]
    fn test_parse_host_ipv4() {
        let url = Url::parse("rtsp://192.168.1.1:554/stream").unwrap();
        println!("url.host(): {:?}", url.host());
        let (target_host, listen_host) = parse_host(&url);
        println!("target_host: {}, listen_host: {}", target_host, listen_host);
        assert_eq!(target_host, "192.168.1.1");
        assert_eq!(listen_host, Ipv4Addr::UNSPECIFIED.to_string());
    }

    #[test]
    fn test_parse_host_ipv6() {
        let url = Url::parse("rtsp://[::1]:554/stream").unwrap();
        let (target_host, listen_host) = parse_host(&url);
        assert_eq!(target_host, "::1");
        assert_eq!(listen_host, Ipv6Addr::UNSPECIFIED.to_string());
    }

    #[test]
    fn test_parse_host_domain() {
        let url = Url::parse("rtsp://localhost:554/stream").unwrap();
        let (target_host, listen_host) = parse_host(&url);

        let is_target_ipv6 = target_host.parse::<Ipv6Addr>().is_ok();
        let is_listen_ipv6 = listen_host.parse::<Ipv6Addr>().is_ok();

        assert_eq!(is_target_ipv6, is_listen_ipv6,);

        if is_target_ipv6 {
            assert_eq!(target_host, Ipv6Addr::LOCALHOST.to_string());
            assert_eq!(listen_host, Ipv6Addr::UNSPECIFIED.to_string());
        } else {
            assert_eq!(target_host, Ipv4Addr::LOCALHOST.to_string());
            assert_eq!(listen_host, Ipv4Addr::UNSPECIFIED.to_string());
        }
    }

    #[test]
    fn test_parse_host_invalid() {
        let url = Url::parse("rtsp:///stream").unwrap();
        let (target_host, listen_host) = parse_host(&url);
        assert_eq!(target_host, Ipv4Addr::LOCALHOST.to_string());
        assert_eq!(listen_host, Ipv4Addr::UNSPECIFIED.to_string());
    }

    #[tokio::test]
    async fn test_create_webrtc_api() {
        let (api_builder, config) = create_webrtc_api().await.unwrap();
        assert_eq!(config.ice_servers.len(), 1);
        assert_eq!(
            config.ice_servers[0].urls,
            vec!["stun:stun.l.google.com:19302"]
        );
        let api = api_builder.build();
        let peer = api.new_peer_connection(config).await.unwrap();
        assert_eq!(peer.connection_state(), RTCPeerConnectionState::New);
    }

    #[tokio::test]
    async fn test_setup_peer_connection_handlers() {
        let (api, config) = create_webrtc_api().await.unwrap();
        let peer = Arc::new(api.build().new_peer_connection(config).await.unwrap());
        let (complete_tx, mut complete_rx) = mpsc::unbounded_channel();

        setup_peer_connection_handlers(peer.clone(), complete_tx.clone()).await;

        peer.on_peer_connection_state_change(Box::new(move |s| {
            let complete_tx = complete_tx.clone();
            tokio::spawn(async move {
                if s == RTCPeerConnectionState::Closed {
                    complete_tx.send(()).unwrap();
                }
            });
            Box::pin(async {})
        }));

        peer.close().await.unwrap();
        assert!(
            complete_rx.recv().await.is_some(),
            "Should receive close signal"
        );
    }
}
