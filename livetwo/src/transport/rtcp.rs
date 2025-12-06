use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, error, info, warn};
use webrtc::peer_connection::RTCPeerConnection;

pub async fn spawn_rtcp_listener(host: String, rtcp_port: u16, peer: Arc<RTCPeerConnection>) {
    let rtcp_listener = match UdpSocket::bind(format!("{}:{}", host, rtcp_port)).await {
        Ok(socket) => {
            info!("RTCP listener bound to: {}", socket.local_addr().unwrap());
            socket
        }
        Err(e) => {
            error!("Failed to bind RTCP listener: {}", e);
            return;
        }
    };

    let mut rtcp_buf = vec![0u8; 1500];

    loop {
        match rtcp_listener.recv_from(&mut rtcp_buf).await {
            Ok((len, addr)) => {
                if len > 0 {
                    debug!("Received {} bytes of RTCP data from {}", len, addr);
                    let mut rtcp_data = &rtcp_buf[..len];

                    if let Ok(rtcp_packets) = webrtc::rtcp::packet::unmarshal(&mut rtcp_data) {
                        for packet in rtcp_packets {
                            debug!("Received RTCP packet from {}: {:?}", addr, packet);
                            if let Err(err) = peer.write_rtcp(&[packet]).await {
                                warn!("Failed to send RTCP packet: {}", err);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                error!("Error receiving RTCP data: {}", e);
            }
        }
    }
}
