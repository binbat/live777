use std::io::Cursor;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, trace, warn};
use webrtc::{peer_connection::RTCPeerConnection, rtp_transceiver::rtp_codec::RTPCodecType};

pub struct TcpHandler {
    video_rtp_channel: Option<u8>,
    video_rtcp_channel: Option<u8>,
    audio_rtp_channel: Option<u8>,
    audio_rtcp_channel: Option<u8>,
}

impl TcpHandler {
    pub fn new(media_info: &rtsp::MediaInfo) -> Self {
        let mut normalized_info = media_info.clone();
        normalized_info.normalize_audio_only();

        let (video_rtp_channel, video_rtcp_channel) =
            Self::extract_tcp_channels(&normalized_info.video_transport);

        let (audio_rtp_channel, audio_rtcp_channel) =
            Self::extract_tcp_channels(&normalized_info.audio_transport);

        info!(
            "TCP handler initialized - Video: {:?}/{:?}, Audio: {:?}/{:?}",
            video_rtp_channel, video_rtcp_channel, audio_rtp_channel, audio_rtcp_channel
        );

        Self {
            video_rtp_channel,
            video_rtcp_channel,
            audio_rtp_channel,
            audio_rtcp_channel,
        }
    }

    fn extract_tcp_channels(transport: &Option<rtsp::TransportInfo>) -> (Option<u8>, Option<u8>) {
        if let Some(rtsp::TransportInfo::Tcp {
            rtp_channel,
            rtcp_channel,
        }) = transport
        {
            (Some(*rtp_channel), Some(*rtcp_channel))
        } else {
            (None, None)
        }
    }

    pub fn spawn_input_to_webrtc(
        &self,
        mut rx: UnboundedReceiver<(u8, Vec<u8>)>,
        video_sender: Option<UnboundedSender<Vec<u8>>>,
        audio_sender: Option<UnboundedSender<Vec<u8>>>,
        peer: Arc<RTCPeerConnection>,
    ) {
        let video_rtp_channel = self.video_rtp_channel;
        let video_rtcp_channel = self.video_rtcp_channel;
        let audio_rtp_channel = self.audio_rtp_channel;
        let audio_rtcp_channel = self.audio_rtcp_channel;

        tokio::spawn(async move {
            info!("TCP input to WebRTC forwarder started");

            while let Some((channel, data)) = rx.recv().await {
                trace!("Received data on channel {}: {} bytes", channel, data.len());

                if Some(channel) == video_rtp_channel {
                    if let Some(ref sender) = video_sender {
                        trace!("Forwarding video RTP to WebRTC");
                        if let Err(e) = sender.send(data) {
                            error!("Failed to forward video RTP: {}", e);
                            break;
                        }
                    }
                } else if Some(channel) == video_rtcp_channel {
                    trace!("Processing video RTCP from input");
                    Self::forward_rtcp_to_webrtc(&data, &peer).await;
                } else if Some(channel) == audio_rtp_channel {
                    if let Some(ref sender) = audio_sender {
                        trace!("Forwarding audio RTP to WebRTC");
                        if let Err(e) = sender.send(data) {
                            error!("Failed to forward audio RTP: {}", e);
                            break;
                        }
                    }
                } else if Some(channel) == audio_rtcp_channel {
                    trace!("Processing audio RTCP from input");
                    Self::forward_rtcp_to_webrtc(&data, &peer).await;
                }
            }

            warn!("TCP input to WebRTC forwarder stopped");
        });
    }

    pub fn spawn_webrtc_to_output(
        &self,
        mut video_recv: UnboundedReceiver<Vec<u8>>,
        mut audio_recv: UnboundedReceiver<Vec<u8>>,
        tx: UnboundedSender<(u8, Vec<u8>)>,
    ) {
        if let Some(channel) = self.video_rtp_channel {
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                info!("Starting video RTP sender on channel {}", channel);
                while let Some(data) = video_recv.recv().await {
                    trace!("Sending video RTP data ({} bytes)", data.len());
                    if let Err(e) = tx_clone.send((channel, data)) {
                        error!("Failed to send video RTP data: {}", e);
                        break;
                    }
                }
                warn!("Video RTP sender stopped");
            });
        }

        if let Some(channel) = self.audio_rtp_channel {
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                info!("Starting audio RTP sender on channel {}", channel);
                while let Some(data) = audio_recv.recv().await {
                    trace!("Sending audio RTP data ({} bytes)", data.len());
                    if let Err(e) = tx_clone.send((channel, data)) {
                        error!("Failed to send audio RTP data: {}", e);
                        break;
                    }
                }
                warn!("Audio RTP sender stopped");
            });
        }
    }

    pub fn spawn_webrtc_rtcp_to_output(
        &self,
        peer: Arc<RTCPeerConnection>,
        tx: UnboundedSender<(u8, Vec<u8>)>,
    ) {
        let video_rtcp_channel = self.video_rtcp_channel;
        let audio_rtcp_channel = self.audio_rtcp_channel;

        tokio::spawn(async move {
            let senders = peer.get_senders().await;

            for sender in senders {
                if let Some(track) = sender.track().await {
                    let tx_clone = tx.clone();
                    let channel = match track.kind() {
                        RTPCodecType::Video => video_rtcp_channel,
                        RTPCodecType::Audio => audio_rtcp_channel,
                        _ => continue,
                    };

                    if let Some(channel) = channel {
                        tokio::spawn(async move {
                            info!("Starting RTCP sender on channel {}", channel);
                            loop {
                                match sender.read_rtcp().await {
                                    Ok((packets, _)) => {
                                        for packet in packets {
                                            if let Ok(data) = packet.marshal() {
                                                debug!("Sending RTCP data ({} bytes)", data.len());
                                                if let Err(e) =
                                                    tx_clone.send((channel, data.to_vec()))
                                                {
                                                    error!("Failed to send RTCP: {}", e);
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!("Error reading RTCP: {}", e);
                                        break;
                                    }
                                }
                            }
                        });
                    }
                }
            }
        });
    }

    pub fn spawn_output_rtcp_to_webrtc(
        &self,
        mut rx: UnboundedReceiver<(u8, Vec<u8>)>,
        peer: Arc<RTCPeerConnection>,
    ) {
        tokio::spawn(async move {
            info!("Starting RTCP receiver from output");

            while let Some((channel, data)) = rx.recv().await {
                debug!(
                    "Received RTCP data from output on channel {}, {} bytes",
                    channel,
                    data.len()
                );
                Self::forward_rtcp_to_webrtc(&data, &peer).await;
            }

            warn!("RTCP receiver from output stopped");
        });
    }

    async fn forward_rtcp_to_webrtc(data: &[u8], peer: &Arc<RTCPeerConnection>) {
        let mut cursor = Cursor::new(data);
        match webrtc::rtcp::packet::unmarshal(&mut cursor) {
            Ok(packets) => {
                if let Err(e) = peer.write_rtcp(&packets).await {
                    error!("Failed to write RTCP to WebRTC: {}", e);
                }
            }
            Err(e) => {
                warn!("Failed to parse RTCP: {}", e);
            }
        }
    }
}
