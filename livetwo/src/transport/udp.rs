use anyhow::Result;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, trace, warn};
use webrtc::{peer_connection::RTCPeerConnection, rtp_transceiver::rtp_codec::RTPCodecType};

use crate::utils;
pub struct UdpHandler;

pub const RTP_BUFFER_SIZE: usize = 1500;

impl UdpHandler {
    pub fn new() -> Self {
        Self
    }

    pub async fn spawn_input_to_webrtc(
        &self,
        media_info: &rtsp::MediaInfo,
        listen_host: &str,
        video_sender: Option<UnboundedSender<Vec<u8>>>,
        audio_sender: Option<UnboundedSender<Vec<u8>>>,
    ) -> Result<()> {
        let mut normalized_info = media_info.clone();
        normalized_info.normalize_audio_only();

        if let Some(rtsp::TransportInfo::Udp {
            rtp_recv_port: Some(video_port),
            ..
        }) = normalized_info.video_transport
            && let Some(sender) = video_sender
        {
            self.spawn_rtp_listener(listen_host, video_port, sender, "video")
                .await?;
            info!("Video UDP RTP listener started on port {}", video_port);
        }

        if let Some(rtsp::TransportInfo::Udp {
            rtp_recv_port: Some(audio_port),
            ..
        }) = normalized_info.audio_transport
            && let Some(sender) = audio_sender
        {
            self.spawn_rtp_listener(listen_host, audio_port, sender, "audio")
                .await?;
            info!("Audio UDP RTP listener started on port {}", audio_port);
        }

        Ok(())
    }

    pub async fn spawn_webrtc_to_output(
        &self,
        video_recv: UnboundedReceiver<Vec<u8>>,
        audio_recv: UnboundedReceiver<Vec<u8>>,
        media_info: &rtsp::MediaInfo,
        target_host: &str,
    ) {
        if let Some(rtsp::TransportInfo::Udp {
            rtp_send_port: Some(target_port),
            server_addr,
            ..
        }) = &media_info.video_transport
        {
            let target_addr = resolve_target_address(server_addr, target_host);
            let listen_host = utils::host::derive_listen_host(&target_addr);

            info!(
                "Starting video RTP sender to {}:{}",
                target_addr, target_port
            );

            tokio::spawn(Self::rtp_sender_task(
                video_recv,
                listen_host,
                target_addr,
                *target_port,
                "video",
            ));
        } else {
            warn!("No video RTP send port configured");
        }

        if let Some(rtsp::TransportInfo::Udp {
            rtp_send_port: Some(target_port),
            server_addr,
            ..
        }) = &media_info.audio_transport
        {
            let target_addr = resolve_target_address(server_addr, target_host);
            let listen_host = utils::host::derive_listen_host(&target_addr);

            info!(
                "Starting audio RTP sender to {}:{}",
                target_addr, target_port
            );

            tokio::spawn(Self::rtp_sender_task(
                audio_recv,
                listen_host,
                target_addr,
                *target_port,
                "audio",
            ));
        } else {
            warn!("No audio RTP send port configured");
        }
    }

    pub async fn spawn_webrtc_rtcp_to_output(
        &self,
        media_info: &rtsp::MediaInfo,
        target_host: &str,
        peer: Arc<RTCPeerConnection>,
    ) -> Result<()> {
        let senders = peer.get_senders().await;

        if let Some(rtsp::TransportInfo::Udp {
            rtcp_send_port: Some(target_port),
            server_addr,
            ..
        }) = &media_info.video_transport
        {
            let bind_host = resolve_target_address(server_addr, target_host);
            let listen_host = utils::host::derive_listen_host(&bind_host);

            for sender in &senders {
                if let Some(track) = sender.track().await
                    && track.kind() == RTPCodecType::Video
                {
                    info!(
                        "Starting video RTCP sender to {}:{}",
                        bind_host, target_port
                    );
                    tokio::spawn(Self::rtcp_sender_task(
                        sender.clone(),
                        listen_host.to_string(),
                        bind_host.clone(),
                        *target_port,
                        "video",
                    ));
                }
            }
        } else {
            debug!("No video RTCP send port configured");
        }

        if let Some(rtsp::TransportInfo::Udp {
            rtcp_send_port: Some(target_port),
            server_addr,
            ..
        }) = &media_info.audio_transport
        {
            let bind_host = resolve_target_address(server_addr, target_host);
            let listen_host = utils::host::derive_listen_host(&bind_host);

            for sender in &senders {
                if let Some(track) = sender.track().await
                    && track.kind() == RTPCodecType::Audio
                {
                    info!(
                        "Starting audio RTCP sender to {}:{}",
                        bind_host, target_port
                    );
                    tokio::spawn(Self::rtcp_sender_task(
                        sender.clone(),
                        listen_host.to_string(),
                        bind_host.clone(),
                        *target_port,
                        "audio",
                    ));
                }
            }
        } else {
            debug!("No audio RTCP send port configured");
        }

        Ok(())
    }

    pub async fn spawn_output_rtcp_to_webrtc(
        &self,
        media_info: &rtsp::MediaInfo,
        target_host: &str,
        peer: Arc<RTCPeerConnection>,
    ) {
        let mut normalized_info = media_info.clone();
        normalized_info.normalize_audio_only();

        if let Some(rtsp::TransportInfo::Udp {
            rtcp_recv_port: Some(port),
            server_addr,
            ..
        }) = &normalized_info.video_transport
        {
            let bind_host = resolve_target_address(server_addr, target_host);
            let listen_host = utils::host::derive_listen_host(&bind_host);

            info!("Starting video RTCP listener on port {}", port);
            tokio::spawn(super::rtcp::spawn_rtcp_listener(
                listen_host,
                *port,
                peer.clone(),
            ));
        } else {
            debug!("No video RTCP receive port configured");
        }

        if let Some(rtsp::TransportInfo::Udp {
            rtcp_recv_port: Some(port),
            server_addr,
            ..
        }) = &normalized_info.audio_transport
        {
            let client_addr = resolve_target_address(server_addr, target_host);
            let listen_host = utils::host::derive_listen_host(&client_addr);

            info!("Starting audio RTCP listener on port {}", port);
            tokio::spawn(super::rtcp::spawn_rtcp_listener(
                listen_host,
                *port,
                peer.clone(),
            ));
        } else {
            debug!("No audio RTCP receive port configured");
        }
    }

    async fn spawn_rtp_listener(
        &self,
        listen_host: &str,
        port: u16,
        sender: UnboundedSender<Vec<u8>>,
        media_type: &'static str,
    ) -> Result<()> {
        let bind_addr = utils::format_bind_addr(listen_host, port);
        let socket = UdpSocket::bind(&bind_addr).await?;

        info!(
            "{} RTP listener bound to {} (local)",
            media_type,
            socket.local_addr()?
        );

        tokio::spawn(async move {
            let mut buf = vec![0u8; RTP_BUFFER_SIZE];

            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((n, addr)) => {
                        trace!("Received {} RTP from {}: {} bytes", media_type, addr, n);

                        if let Err(e) = sender.send(buf[..n].to_vec()) {
                            error!("Failed to forward {} RTP: {}", media_type, e);
                            break;
                        }
                    }
                    Err(e) => {
                        error!("{} RTP receive error: {}", media_type, e);
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    async fn rtp_sender_task(
        mut receiver: UnboundedReceiver<Vec<u8>>,
        listen_host: String,
        target_host: String,
        target_port: u16,
        media_type: &'static str,
    ) {
        let bind_addr = utils::format_bind_addr(&listen_host, 0);

        let socket = match UdpSocket::bind(&bind_addr).await {
            Ok(s) => {
                info!(
                    "{} RTP sender bound to {} (local)",
                    media_type,
                    s.local_addr().unwrap()
                );
                s
            }
            Err(e) => {
                error!("Failed to bind {} RTP socket: {}", media_type, e);
                return;
            }
        };

        let target_addr = utils::format_bind_addr(&target_host, target_port);
        info!(
            "{} RTP sender ready to send to {} (remote)",
            media_type, target_addr
        );

        while let Some(data) = receiver.recv().await {
            match socket.send_to(&data, &target_addr).await {
                Ok(_) => {
                    trace!("Sent {} bytes to {}", data.len(), target_addr);
                }
                Err(e) => {
                    error!(
                        "Failed to send {} RTP to {}: {}",
                        media_type, target_addr, e
                    );
                }
            }
        }
    }

    async fn rtcp_sender_task(
        sender: Arc<webrtc::rtp_transceiver::rtp_sender::RTCRtpSender>,
        listen_host: String,
        target_host: String,
        target_port: u16,
        media_type: &'static str,
    ) -> Result<()> {
        let bind_addr = utils::format_bind_addr(&listen_host, 0);
        let udp_socket = UdpSocket::bind(&bind_addr).await?;

        info!(
            "{} RTCP sender bound to {} (local)",
            media_type,
            udp_socket.local_addr().unwrap()
        );

        let target_addr = utils::format_bind_addr(&target_host, target_port);
        info!(
            "{} RTCP sender ready to send to {} (remote)",
            media_type, target_addr
        );

        let mut packet_count = 0;
        loop {
            match sender.read_rtcp().await {
                Ok((packets, _)) => {
                    for packet in packets {
                        trace!("Received {} RTCP from WebRTC: {:?}", media_type, packet);

                        if let Ok(data) = packet.marshal() {
                            packet_count += 1;
                            if packet_count % 10 == 0 {
                                debug!(
                                    "Sent {} {} RTCP packets to {}",
                                    packet_count, media_type, target_addr
                                );
                            }

                            if let Err(err) = udp_socket.send_to(&data, &target_addr).await {
                                warn!(
                                    "Failed to send {} RTCP to {}: {}",
                                    media_type, target_addr, err
                                );
                            } else {
                                trace!("Sent {} RTCP to {}", media_type, target_addr);
                            }
                        }
                    }
                }
                Err(err) => {
                    warn!("Error reading {} RTCP from WebRTC: {}", media_type, err);
                    break Ok(());
                }
            }
        }
    }
}

fn resolve_target_address(server_addr: &Option<std::net::SocketAddr>, target_host: &str) -> String {
    if let Some(addr) = server_addr {
        addr.ip().to_string()
    } else if target_host.is_empty() || target_host == "0.0.0.0" || target_host == "::" {
        std::net::Ipv4Addr::LOCALHOST.to_string()
    } else {
        target_host.to_string()
    }
}

impl Default for UdpHandler {
    fn default() -> Self {
        Self::new()
    }
}
