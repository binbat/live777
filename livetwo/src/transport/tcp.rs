use std::io::Cursor;
use std::sync::Arc;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tracing::{debug, error, info, trace, warn};
use webrtc::peer_connection::PeerConnection;

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
        peer: Arc<dyn PeerConnection>,
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
        _peer: Arc<dyn PeerConnection>,
        _tx: UnboundedSender<(u8, Vec<u8>)>,
    ) {
        // In v0.20, RTCP feedback is handled internally by the peer connection.
        // sender.read_rtcp() is no longer available.
        // RTCP forwarding to output is not needed in the new architecture.
        debug!("RTCP to output forwarding is handled internally in v0.20");
    }

    pub fn spawn_output_rtcp_to_webrtc(
        &self,
        mut rx: UnboundedReceiver<(u8, Vec<u8>)>,
        peer: Arc<dyn PeerConnection>,
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

    async fn forward_rtcp_to_webrtc(data: &[u8], _peer: &Arc<dyn PeerConnection>) {
        let mut cursor = Cursor::new(data);
        match rtc_rtcp::packet::unmarshal(&mut cursor) {
            Ok(packets) => {
                for packet in packets {
                    crate::whip::log_rtcp_feedback_packet("TCP output RTCP", packet.as_ref());
                }
                // In v0.20, write_rtcp is on TrackLocal/TrackRemote, not PeerConnection.
                // RTCP from the output side would need to be sent via specific tracks.
                debug!("Parsed RTCP packet (forwarding not yet implemented for v0.20)");
            }
            Err(e) => {
                warn!("Failed to parse RTCP: {}", e);
            }
        }
    }
}
