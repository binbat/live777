use anyhow::Result;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{error, info, trace};

#[derive(Debug, Clone)]
pub struct UdpPortInfo {
    pub client_rtp_port: u16,
    pub client_rtcp_port: u16,
    pub server_rtp_port: u16,
    pub server_rtcp_port: u16,
    pub client_addr: SocketAddr,
}

#[derive(Debug, Clone)]
pub enum TransportConfig {
    Tcp { rtp_channel: u8, rtcp_channel: u8 },
    Udp(UdpPortInfo),
}

pub struct UdpSocketPair {
    pub rtp_socket: Arc<UdpSocket>,
    pub rtcp_socket: Arc<UdpSocket>,
}

impl UdpSocketPair {
    pub async fn create_and_connect(port_info: &UdpPortInfo) -> Result<Self> {
        let rtp_socket = UdpSocket::bind(format!("0.0.0.0:{}", port_info.server_rtp_port)).await?;
        let rtcp_socket =
            UdpSocket::bind(format!("0.0.0.0:{}", port_info.server_rtcp_port)).await?;

        info!(
            "UDP sockets bound: RTP={}, RTCP={}",
            port_info.server_rtp_port, port_info.server_rtcp_port
        );

        rtp_socket
            .connect(format!(
                "{}:{}",
                port_info.client_addr.ip(),
                port_info.client_rtp_port
            ))
            .await?;

        rtcp_socket
            .connect(format!(
                "{}:{}",
                port_info.client_addr.ip(),
                port_info.client_rtcp_port
            ))
            .await?;

        info!(
            "UDP sockets connected to {}:{}/{}",
            port_info.client_addr.ip(),
            port_info.client_rtp_port,
            port_info.client_rtcp_port
        );

        Ok(Self {
            rtp_socket: Arc::new(rtp_socket),
            rtcp_socket: Arc::new(rtcp_socket),
        })
    }

    pub fn spawn_rtp_receiver(self, tx: UnboundedSender<Vec<u8>>) -> tokio::task::JoinHandle<()> {
        let socket = Arc::clone(&self.rtp_socket);

        tokio::spawn(async move {
            let mut buf = vec![0u8; 2000];
            info!("UDP RTP receiver started");

            loop {
                match socket.recv(&mut buf).await {
                    Ok(n) => {
                        trace!("Received RTP packet: {} bytes", n);
                        if let Err(e) = tx.send(buf[..n].to_vec()) {
                            error!("Failed to forward RTP: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("RTP receive error: {}", e);
                        break;
                    }
                }
            }

            info!("UDP RTP receiver stopped");
        })
    }

    pub fn spawn_rtcp_handler(
        self,
        rtcp_from_webrtc_rx: UnboundedReceiver<Vec<u8>>,
        rtcp_to_webrtc_tx: UnboundedSender<Vec<u8>>,
    ) -> tokio::task::JoinHandle<()> {
        let rtcp_socket_read = Arc::clone(&self.rtcp_socket);
        let rtcp_socket_write = Arc::clone(&self.rtcp_socket);

        tokio::spawn(async move {
            let read_task = Self::rtcp_read_task(rtcp_socket_read, rtcp_to_webrtc_tx);
            let write_task = Self::rtcp_write_task(rtcp_socket_write, rtcp_from_webrtc_rx);

            let (read_result, write_result) = tokio::join!(read_task, write_task);

            if let Err(e) = read_result {
                error!("RTCP read task error: {}", e);
            }
            if let Err(e) = write_result {
                error!("RTCP write task error: {}", e);
            }
        })
    }

    async fn rtcp_read_task(socket: Arc<UdpSocket>, tx: UnboundedSender<Vec<u8>>) -> Result<()> {
        let mut buf = vec![0u8; 1500];
        info!("RTCP receiver started");

        loop {
            match socket.recv(&mut buf).await {
                Ok(n) => {
                    trace!("Received RTCP packet: {} bytes", n);
                    if let Err(e) = tx.send(buf[..n].to_vec()) {
                        error!("Failed to forward RTCP: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    error!("RTCP receive error: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }

    async fn rtcp_write_task(
        socket: Arc<UdpSocket>,
        mut rx: UnboundedReceiver<Vec<u8>>,
    ) -> Result<()> {
        info!("RTCP sender started");

        while let Some(data) = rx.recv().await {
            trace!("Sending RTCP packet: {} bytes", data.len());
            if let Err(e) = socket.send(&data).await {
                error!("Failed to send RTCP: {}", e);
                break;
            }
        }

        Ok(())
    }
}

pub struct TransportManager {
    config: TransportConfig,
}

impl TransportManager {
    pub fn new(config: TransportConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &TransportConfig {
        &self.config
    }

    pub async fn create_udp_sockets(&self) -> Result<Option<UdpSocketPair>> {
        match &self.config {
            TransportConfig::Udp(port_info) => {
                let pair = UdpSocketPair::create_and_connect(port_info).await?;
                Ok(Some(pair))
            }
            TransportConfig::Tcp { .. } => Ok(None),
        }
    }

    pub fn is_tcp(&self) -> bool {
        matches!(self.config, TransportConfig::Tcp { .. })
    }
}
