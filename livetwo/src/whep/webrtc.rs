use std::sync::Arc;

use anyhow::{Result, anyhow};
use libwish::Client;
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use rtc::shared::marshal::{Marshal, MarshalSize};
use tokio::sync::{Mutex, Notify, mpsc::UnboundedSender};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState,
    RTCSessionDescription,
};
use webrtc::rtp_transceiver::{RTCRtpTransceiverDirection, RTCRtpTransceiverInit};

use crate::utils;
use crate::utils::stats::RtcpStats;

pub async fn setup_whep_peer(
    ct: CancellationToken,
    client: &mut Client,
    video_send: UnboundedSender<Vec<u8>>,
    audio_send: UnboundedSender<Vec<u8>>,
    codec_info: Arc<Mutex<rtsp::CodecInfo>>,
) -> Result<(
    Arc<dyn PeerConnection>,
    RTCSessionDescription,
    Arc<RtcpStats>,
)> {
    let gather_complete = Arc::new(Notify::new());
    let peer = create_peer(
        ct,
        video_send,
        audio_send,
        codec_info.clone(),
        gather_complete.clone(),
    )
    .await?;

    utils::webrtc::setup_connection(peer.clone(), client, gather_complete).await?;

    let answer = peer
        .remote_description()
        .await
        .ok_or_else(|| anyhow!("No remote description"))?;

    let stats = Arc::new(RtcpStats::new());

    Ok((peer, answer, stats))
}

#[derive(Clone)]
struct WhepTrackHandler {
    ct: CancellationToken,
    gather_complete: Arc<Notify>,
    video_send: UnboundedSender<Vec<u8>>,
    audio_send: UnboundedSender<Vec<u8>>,
    codec_info: Arc<Mutex<rtsp::CodecInfo>>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for WhepTrackHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        info!("WHEP connection state changed: {}", state);
        match state {
            RTCPeerConnectionState::Failed => {
                self.ct.cancel();
                warn!("WHEP connection closed due to failure");
            }
            RTCPeerConnectionState::Closed => {
                self.ct.cancel();
                info!("WHEP connection closed normally");
            }
            _ => debug!("WHEP connection state: {}", state),
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let kind = track.kind().await;
        let ssrcs = track.ssrcs().await;
        let track_id = track.track_id().await;
        info!(
            "WHEP on_track: kind={}, ssrcs={:?}, id={}",
            kind, ssrcs, track_id
        );

        // Extract codec info from the track
        let first_ssrc = ssrcs.first().copied().unwrap_or(0);
        if let Some(codec) = track.codec(first_ssrc).await {
            let codec_params = rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters {
                rtp_codec: codec.clone(),
                payload_type: 0, // Will be negotiated
            };
            let mut info = self.codec_info.lock().await;
            match kind {
                RtpCodecKind::Video => {
                    debug!("WHEP updating video codec: {:?}", codec);
                    info.video_codec = Some(codec_params);
                }
                RtpCodecKind::Audio => {
                    debug!("WHEP updating audio codec: {:?}", codec);
                    info.audio_codec = Some(codec_params);
                }
                _ => {}
            }
        }

        // Select the appropriate sender channel
        let sender = match kind {
            RtpCodecKind::Video => Some(self.video_send.clone()),
            RtpCodecKind::Audio => Some(self.audio_send.clone()),
            _ => None,
        };

        if let Some(sender) = sender {
            let track_clone = track.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 1500];
                loop {
                    match track_clone.poll().await {
                        Some(TrackRemoteEvent::OnRtpPacket(rtp_packet)) => {
                            let size = rtp_packet.marshal_size();
                            if size > buf.len() {
                                warn!("WHEP: RTP packet too large ({} bytes)", size);
                                continue;
                            }
                            if let Err(e) = rtp_packet.marshal_to(&mut buf[..size]) {
                                warn!("WHEP: Failed to marshal RTP packet: {}", e);
                                continue;
                            }
                            if sender.send(buf[..size].to_vec()).is_err() {
                                debug!("WHEP: {} channel receiver dropped, stopping", kind);
                                break;
                            }
                        }
                        Some(TrackRemoteEvent::OnEnded) => {
                            info!("WHEP: {} track ended", kind);
                            break;
                        }
                        Some(TrackRemoteEvent::OnRtcpPacket(packets)) => {
                            // Forward RTCP packets for stats tracking
                            debug!("WHEP: Received {} RTCP packets for {}", packets.len(), kind);
                        }
                        None => {
                            debug!("WHEP: {} track poll returned None", kind);
                            break;
                        }
                        _ => {}
                    }
                }
                info!("WHEP: {} RTP reader stopped", kind);
            });
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            info!("WHEP ICE gathering complete");
            self.gather_complete.notify_one();
        }
    }
}

async fn create_peer(
    ct: CancellationToken,
    video_send: UnboundedSender<Vec<u8>>,
    audio_send: UnboundedSender<Vec<u8>>,
    codec_info: Arc<Mutex<rtsp::CodecInfo>>,
    gather_complete: Arc<Notify>,
) -> Result<Arc<dyn PeerConnection>> {
    let (builder, config) = utils::webrtc::create_peer_connection_builder()?;
    let handler = Arc::new(WhepTrackHandler {
        ct,
        gather_complete,
        video_send,
        audio_send,
        codec_info,
    });

    let peer: Arc<dyn PeerConnection> = Arc::new(
        builder
            .with_configuration(config)
            .with_handler(handler)
            .with_udp_addrs(vec!["0.0.0.0:0".parse().unwrap()])
            .build()
            .await
            .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?,
    );

    peer.add_transceiver_from_kind(
        RtpCodecKind::Video,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            streams: vec![],
            send_encodings: vec![],
        }),
    )
    .await
    .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    peer.add_transceiver_from_kind(
        RtpCodecKind::Audio,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            streams: vec![],
            send_encodings: vec![],
        }),
    )
    .await
    .map_err(|error| anyhow!(format!("{:?}: {}", error, error)))?;

    Ok(peer)
}
