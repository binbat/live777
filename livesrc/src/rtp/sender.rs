// RTP Sender interface
// Abstracts the transport layer for sending RTP packets

use anyhow::Result;
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;

use super::h264_packetizer::RtpPacket;

/// RTP Sender trait
/// Different implementations for different transports
pub trait RtpSender: Send + Sync {
    fn send(&self, packet: &RtpPacket) -> Result<()>;
}

/// UDP RTP Sender (for testing and local transmission)
pub struct UdpRtpSender {
    socket: UdpSocket,
    dest_addr: SocketAddr,
}

impl UdpRtpSender {
    pub fn new(dest_addr: SocketAddr) -> Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        Ok(Self { socket, dest_addr })
    }
}

impl RtpSender for UdpRtpSender {
    fn send(&self, packet: &RtpPacket) -> Result<()> {
        let bytes = packet.to_bytes();
        self.socket.send_to(&bytes, self.dest_addr)?;
        Ok(())
    }
}

/// Liveion RTP Sender (placeholder for future implementation)
/// Will be implemented once liveion provides the RTP receiver interface
pub struct LiveionRtpSender {
    // TODO: Add fields based on liveion's interface
    // Possible options:
    // - gRPC channel
    // - Unix domain socket
    // - Shared memory
    // - Custom protocol
}

impl LiveionRtpSender {
    pub fn new() -> Result<Self> {
        // TODO: Initialize connection to liveion
        todo!("Waiting for liveion RTP receiver interface specification")
    }
}

impl RtpSender for LiveionRtpSender {
    fn send(&self, _packet: &RtpPacket) -> Result<()> {
        // TODO: Implement based on liveion's interface
        todo!("Waiting for liveion RTP receiver interface specification")
    }
}

/// Create RTP sender based on configuration
pub fn create_rtp_sender(mode: &str, address: Option<&str>) -> Result<Arc<dyn RtpSender>> {
    match mode {
        "udp" => {
            let addr = address
                .ok_or_else(|| anyhow::anyhow!("UDP mode requires address"))?
                .parse()?;
            Ok(Arc::new(UdpRtpSender::new(addr)?))
        }
        "liveion" => {
            Ok(Arc::new(LiveionRtpSender::new()?))
        }
        _ => anyhow::bail!("Unknown RTP sender mode: {}", mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtp::h264_packetizer::*;

    #[test]
    fn test_create_udp_sender() {
        let sender = create_rtp_sender("udp", Some("127.0.0.1:5004"));
        assert!(sender.is_ok());
    }

    #[test]
    fn test_udp_sender_send() {
        let sender = UdpRtpSender::new("127.0.0.1:5004".parse().unwrap()).unwrap();
        
        let packet = RtpPacket {
            header: RtpHeader {
                version: 2,
                padding: false,
                extension: false,
                marker: true,
                payload_type: 96,
                sequence: 1234,
                timestamp: 90000,
                ssrc: 0x12345678,
            },
            payload: vec![0x67, 0x42, 0x00, 0x1e],
        };

        // This will send to localhost - should not error
        let result = sender.send(&packet);
        assert!(result.is_ok());
    }
}
