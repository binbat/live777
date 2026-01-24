use anyhow::Result;
use std::net::SocketAddr;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub struct UdpMessage {
    pub data: Vec<u8>,
    pub addr: SocketAddr,
}

pub struct UdpServer {
    socket: UdpSocket,
    max_message_size: usize,
    enable_logging: bool,
}

impl UdpServer {
    pub async fn new(listen_addr: &str, port: u16, max_message_size: usize, enable_logging: bool) -> Result<Self> {
        let addr = format!("{}:{}", listen_addr, port);
        let socket = UdpSocket::bind(&addr).await?;
        info!("UDP server listening on {}", addr);
        
        Ok(Self {
            socket,
            max_message_size,
            enable_logging,
        })
    }
    
    pub async fn run(
        &self,
        mut outbound_rx: mpsc::Receiver<UdpMessage>,
        inbound_tx: mpsc::Sender<UdpMessage>,
    ) -> Result<()> {
        let socket = &self.socket;
        let max_size = self.max_message_size;
        let enable_logging = self.enable_logging;
        
        loop {
            tokio::select! {
                // Handle incoming UDP messages
                result = self.receive_message() => {
                    match result {
                        Ok(message) => {
                            if enable_logging {
                                debug!("UDP received from {}: {} bytes", message.addr, message.data.len());
                                if let Ok(text) = String::from_utf8(message.data.clone()) {
                                    debug!("UDP content: {}", text);
                                }
                            }
                            
                            if let Err(e) = inbound_tx.send(message).await {
                                error!("Failed to forward inbound UDP message: {}", e);
                            }
                        }
                        Err(e) => {
                            error!("Failed to receive UDP message: {}", e);
                        }
                    }
                }
                
                // Handle outbound UDP messages
                Some(message) = outbound_rx.recv() => {
                    if message.data.len() > max_size {
                        warn!("Outbound message too large: {} bytes (max: {})", message.data.len(), max_size);
                        continue;
                    }
                    
                    match socket.send_to(&message.data, message.addr).await {
                        Ok(sent) => {
                            if enable_logging {
                                debug!("UDP sent to {}: {} bytes", message.addr, sent);
                                if let Ok(text) = String::from_utf8(message.data.clone()) {
                                    debug!("UDP content: {}", text);
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to send UDP message to {}: {}", message.addr, e);
                        }
                    }
                }
            }
        }
    }
    
    async fn receive_message(&self) -> Result<UdpMessage> {
        let mut buffer = vec![0u8; self.max_message_size];
        let (len, addr) = self.socket.recv_from(&mut buffer).await?;
        buffer.truncate(len);
        
        Ok(UdpMessage {
            data: buffer,
            addr,
        })
    }
}