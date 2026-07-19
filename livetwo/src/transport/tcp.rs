use rtc::rtp::packet::Packet;
use rtc_shared::marshal::Unmarshal;
use std::io::Cursor;
use std::sync::Arc;
use tokio::sync::mpsc::{Receiver, Sender, UnboundedSender};
use tokio_util::sync::CancellationToken;
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
        mut rx: Receiver<(u8, Vec<u8>)>,
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
            let mut first_video_rtp = true;
            let mut first_audio_rtp = true;
            let mut video_pkt_count: u64 = 0;
            let mut audio_pkt_count: u64 = 0;
            let mut last_error: Option<String> = None;

            while let Some((channel, data)) = rx.recv().await {
                trace!("Received data on channel {}: {} bytes", channel, data.len());

                if Some(channel) == video_rtp_channel {
                    if let Some(ref sender) = video_sender {
                        if first_video_rtp {
                            log_first_rtp_packet("video", channel, &data);
                            first_video_rtp = false;
                        }
                        trace!("Forwarding video RTP to WebRTC");
                        if let Err(e) = sender.send(data) {
                            last_error = Some(format!("video RTP send: {e}"));
                            error!("Failed to forward video RTP: {}", e);
                            break;
                        }
                        video_pkt_count += 1;
                    }
                } else if Some(channel) == video_rtcp_channel {
                    trace!("Processing video RTCP from input");
                    Self::forward_rtcp_to_webrtc(&data, &peer).await;
                } else if Some(channel) == audio_rtp_channel {
                    if let Some(ref sender) = audio_sender {
                        if first_audio_rtp {
                            log_first_rtp_packet("audio", channel, &data);
                            first_audio_rtp = false;
                        }
                        trace!("Forwarding audio RTP to WebRTC");
                        if let Err(e) = sender.send(data) {
                            last_error = Some(format!("audio RTP send: {e}"));
                            error!("Failed to forward audio RTP: {}", e);
                            break;
                        }
                        audio_pkt_count += 1;
                    }
                } else if Some(channel) == audio_rtcp_channel {
                    trace!("Processing audio RTCP from input");
                    Self::forward_rtcp_to_webrtc(&data, &peer).await;
                }
            }

            warn!(
                video_rtp_ch = ?video_rtp_channel,
                video_rtcp_ch = ?video_rtcp_channel,
                audio_rtp_ch = ?audio_rtp_channel,
                audio_rtcp_ch = ?audio_rtcp_channel,
                video_pkts = video_pkt_count,
                audio_pkts = audio_pkt_count,
                last_error = ?last_error,
                "TCP input to WebRTC forwarder stopped (sender dropped or error)"
            );
        });
    }

    pub fn spawn_webrtc_to_output(
        &self,
        mut video_recv: Receiver<Vec<u8>>,
        mut audio_recv: Receiver<Vec<u8>>,
        tx: Sender<(u8, Vec<u8>)>,
    ) {
        if let Some(channel) = self.video_rtp_channel {
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                info!("Starting video RTP sender on channel {}", channel);
                while let Some(data) = video_recv.recv().await {
                    trace!("Sending video RTP data ({} bytes)", data.len());
                    if let Err(e) = tx_clone.send((channel, data)).await {
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
                    if let Err(e) = tx_clone.send((channel, data)).await {
                        error!("Failed to send audio RTP data: {}", e);
                        break;
                    }
                }
                warn!("Audio RTP sender stopped");
            });
        }
    }

    /// Spawn a task that holds the TCP sender open until cancellation.
    ///
    /// In v0.20, RTCP feedback is handled internally by the peer connection
    /// (`sender.read_rtcp()` is no longer available). The endpoint must still be
    /// kept alive: the RTSP client handler tears down the whole interleaved
    /// connection when the mpsc channel closes, and dropping `tx` is exactly that
    /// signal. Once the token is cancelled, the sender is dropped and the RTSP
    /// connection tears down cleanly.
    ///
    /// The task is spawned rather than storing `tx` in `TcpHandler` because
    /// `TcpHandler` is `Clone` (shared across video/audio forwarders), while the
    /// connection-wide sender must be dropped exactly once.
    pub fn spawn_webrtc_rtcp_to_output(
        &self,
        ct: CancellationToken,
        _peer: Arc<dyn PeerConnection>,
        tx: Sender<(u8, Vec<u8>)>,
    ) {
        // In v0.20, RTCP feedback is handled internally by the peer connection.
        // sender.read_rtcp() is no longer available.
        // RTCP forwarding to output is not needed in the new architecture.
        //
        // The endpoint must still be kept alive: the RTSP client handler
        // tears down the whole interleaved connection when the channel
        // closes, and dropping `tx` is exactly that signal. Hold it until the
        // session is cancelled — a `pending()` task would hold it forever.
        debug!("RTCP to output forwarding is handled internally in v0.20");
        tokio::spawn(async move {
            let _tx = tx;
            ct.cancelled().await;
        });
    }

    pub fn spawn_output_rtcp_to_webrtc(
        &self,
        mut rx: Receiver<(u8, Vec<u8>)>,
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

    async fn forward_rtcp_to_webrtc(data: &[u8], peer: &Arc<dyn PeerConnection>) {
        let mut cursor = Cursor::new(data);
        match rtc_rtcp::packet::unmarshal(&mut cursor) {
            Ok(packets) => {
                for packet in packets {
                    crate::whip::log_rtcp_feedback_packet("TCP output RTCP", packet.as_ref());
                    Self::forward_rtcp_packet_to_webrtc(packet, peer).await;
                }
            }
            Err(e) => {
                warn!("Failed to parse RTCP: {}", e);
            }
        }
    }

    async fn forward_rtcp_packet_to_webrtc(
        packet: Box<dyn rtc_rtcp::packet::Packet + Send + Sync>,
        peer: &Arc<dyn PeerConnection>,
    ) {
        let destination_ssrcs = packet.destination_ssrc();
        if destination_ssrcs.is_empty() {
            debug!("Dropping RTCP packet without destination SSRC");
            return;
        }

        let receivers = peer.get_receivers().await;
        let mut target_track = None;
        for receiver in receivers {
            let track = receiver.track().clone();
            let track_ssrcs = track.ssrcs().await;
            if destination_ssrcs
                .iter()
                .any(|destination| track_ssrcs.contains(destination))
            {
                target_track = Some(track);
                break;
            }
        }

        let Some(track) = target_track else {
            warn!(
                "Dropping RTCP packet for unknown WHEP destination SSRC(s): {:?}",
                destination_ssrcs
            );
            return;
        };

        if let Err(error) = track.write_rtcp(vec![packet]).await {
            warn!("Failed to forward RTCP packet to WHEP track: {}", error);
        }
    }
}

fn log_first_rtp_packet(kind: &str, channel: u8, data: &[u8]) {
    let mut cursor = data;
    match Packet::unmarshal(&mut cursor) {
        Ok(packet) => info!(
            "First RTSP TCP {kind} RTP packet received: channel={}, payload_type={}, sequence_number={}, timestamp={}, ssrc={}, payload_len={}",
            channel,
            packet.header.payload_type,
            packet.header.sequence_number,
            packet.header.timestamp,
            packet.header.ssrc,
            packet.payload.len()
        ),
        Err(error) => warn!(
            "First RTSP TCP {kind} RTP packet on channel {} failed to parse: {} bytes, error={}",
            channel,
            data.len(),
            error
        ),
    }
}
