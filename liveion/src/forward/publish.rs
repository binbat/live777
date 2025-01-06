use std::sync::{Arc, Weak};

use anyhow::{anyhow, Result};
use chrono::Utc;
use tokio::sync::broadcast;
use tracing::debug;
use webrtc::peer_connection::RTCPeerConnection;

use crate::forward::message::SessionInfo;
use crate::forward::rtcp::RtcpMessage;

use super::get_peer_id;
use super::media::MediaInfo;
use super::message::CascadeInfo;

pub(crate) struct PublishRTCPeerConnection {
    pub(crate) id: String,
    pub(crate) peer: Arc<RTCPeerConnection>,
    pub(crate) media_info: MediaInfo,
    pub(crate) create_at: i64,
    pub(crate) cascade: Option<CascadeInfo>,
}

impl PublishRTCPeerConnection {
    pub(crate) async fn new(
        path: String,
        peer: Arc<RTCPeerConnection>,
        rtcp_recv: broadcast::Receiver<(RtcpMessage, u32)>,
        cascade: Option<CascadeInfo>,
    ) -> Result<Self> {
        let id = get_peer_id(&peer);
        let peer_weak = Arc::downgrade(&peer);
        let media_info = MediaInfo::try_from(
            peer.remote_description()
                .await
                .ok_or(anyhow!("not set remote_description"))?
                .unmarshal()?,
        )?;
        tokio::spawn(Self::peer_send_rtcp(path, id.clone(), peer_weak, rtcp_recv));
        Ok(Self {
            id,
            peer,
            media_info,
            create_at: Utc::now().timestamp_millis(),
            cascade,
        })
    }

    pub(crate) fn info(&self) -> SessionInfo {
        SessionInfo {
            id: self.id.clone(),
            create_at: self.create_at,
            state: self.peer.connection_state(),
            cascade: self.cascade.clone(),
            has_data_channel: self.media_info.has_data_channel,
        }
    }

    async fn peer_send_rtcp(
        path: String,
        id: String,
        peer: Weak<RTCPeerConnection>,
        mut recv: broadcast::Receiver<(RtcpMessage, u32)>,
    ) {
        while let (Ok((rtcp_message, media_ssrc)), Some(pc)) = (recv.recv().await, peer.upgrade()) {
            debug!(
                "[{}] [{}] ssrc : {} ,send rtcp : {:?}",
                path, id, media_ssrc, rtcp_message
            );
            if pc
                .write_rtcp(&[rtcp_message.to_rtcp_packet(media_ssrc)])
                .await
                .is_err()
            {
                break;
            }
        }
    }
}
