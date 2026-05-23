use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, error, info};
use webrtc::peer_connection::PeerConnection;

const RTCP_BUFFER_SIZE: usize = 1500;

pub async fn spawn_rtcp_listener(host: String, rtcp_port: u16, _peer: Arc<dyn PeerConnection>) {
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

    let mut rtcp_buf = vec![0u8; RTCP_BUFFER_SIZE];

    loop {
        match rtcp_listener.recv_from(&mut rtcp_buf).await {
            Ok((len, addr)) => {
                if len > 0 {
                    debug!("Received {} bytes of RTCP data from {}", len, addr);
                    let mut rtcp_data = &rtcp_buf[..len];

                    // Parse RTCP packets using rtc_rtcp
                    match rtc_rtcp::packet::unmarshal(&mut rtcp_data) {
                        Ok(rtcp_packets) => {
                            for packet in rtcp_packets {
                                debug!("Received RTCP packet from {}: {:?}", addr, packet);
                                // Note: In the new API, write_rtcp is on TrackLocal/TrackRemote,
                                // not on PeerConnection. RTCP forwarding is handled elsewhere.
                            }
                        }
                        Err(e) => {
                            debug!("Failed to parse RTCP from {}: {}", addr, e);
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
