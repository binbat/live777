use std::io::Cursor;
use std::sync::{Arc, Weak};

use anyhow::{Result, anyhow};
use chrono::Utc;
use sdp::SessionDescription;
use tokio::sync::broadcast;
use tracing::debug;
use webrtc::peer_connection::PeerConnection;

use crate::forward::message::SessionInfo;
use crate::forward::rtcp::RtcpMessage;

use super::get_peer_id;
use super::media::MediaInfo;
use super::message::CascadeInfo;

pub(crate) struct PublishRTCPeerConnection {
    pub(crate) id: String,
    pub(crate) peer: Arc<dyn PeerConnection>,
    pub(crate) media_info: MediaInfo,
    pub(crate) create_at: i64,
    pub(crate) cascade: Option<CascadeInfo>,
    connection_state: std::sync::RwLock<webrtc::peer_connection::RTCPeerConnectionState>,
}

impl PublishRTCPeerConnection {
    pub(crate) async fn new(
        path: String,
        peer: Arc<dyn PeerConnection>,
        rtcp_recv: broadcast::Receiver<(RtcpMessage, u32)>,
        cascade: Option<CascadeInfo>,
    ) -> Result<Self> {
        let id = get_peer_id(&peer);
        let peer_weak = Arc::downgrade(&peer);
        let remote_desc = peer.remote_description()
            .await
            .ok_or(anyhow!("not set remote_description"))?;
        let mut reader = Cursor::new(remote_desc.sdp.as_bytes());
        let sdp = SessionDescription::unmarshal(&mut reader)?;
        let media_info = MediaInfo::try_from(sdp)?;
        tokio::spawn(Self::peer_send_rtcp(path, id.clone(), peer_weak, rtcp_recv));
        Ok(Self {
            id,
            peer,
            media_info,
            create_at: Utc::now().timestamp_millis(),
            cascade,
            connection_state: std::sync::RwLock::new(webrtc::peer_connection::RTCPeerConnectionState::New),
        })
    }

    pub(crate) fn set_connection_state(&self, state: webrtc::peer_connection::RTCPeerConnectionState) {
        if let Ok(mut s) = self.connection_state.write() {
            *s = state;
        }
    }

    pub(crate) async fn info(&self) -> SessionInfo {
        let state = self.connection_state.read().map(|s| *s).unwrap_or(webrtc::peer_connection::RTCPeerConnectionState::New);
        SessionInfo {
            id: self.id.clone(),
            create_at: self.create_at,
            state,
            cascade: self.cascade.clone(),
            has_data_channel: self.media_info.has_data_channel,
        }
    }

    async fn peer_send_rtcp(
        path: String,
        id: String,
        peer: Weak<dyn PeerConnection>,
        mut recv: broadcast::Receiver<(RtcpMessage, u32)>,
    ) {
        while let (Ok((rtcp_message, media_ssrc)), Some(pc)) = (recv.recv().await, peer.upgrade()) {
            debug!(
                "[{}] [{}] ssrc : {} ,send rtcp : {:?}",
                path, id, media_ssrc, rtcp_message
            );
            // In v0.20, write_rtcp is on TrackLocal/TrackRemote, not PeerConnection.
            // Find the receiver track matching the SSRC and send RTCP via it.
            let receivers = pc.get_receivers().await;
            if receivers.is_empty() {
                continue;
            }
            let mut found = false;
            for receiver in &receivers {
                let track = receiver.track();
                let ssrcs = track.ssrcs().await;
                if ssrcs.contains(&media_ssrc) {
                    found = true;
                    if track.write_rtcp(vec![rtcp_message.to_rtcp_packet(media_ssrc)]).await.is_err() {
                        debug!(
                            "[{}] [{}] Failed to write RTCP for ssrc {}",
                            path, id, media_ssrc
                        );
                    }
                    break;
                }
            }
            if !found {
                debug!(
                    "[{}] [{}] No receiver found for ssrc {}",
                    path, id, media_ssrc
                );
            }
        }
    }
}
