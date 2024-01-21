use std::sync::{Arc, Weak};

use log::debug;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;
use webrtc::peer_connection::RTCPeerConnection;

use crate::forward::rtcp::RtcpMessage;

use super::get_peer_id;

pub(crate) struct PublishRTCPeerConnection {
    pub(crate) id: String,
    pub(crate) peer: Arc<RTCPeerConnection>,
    pub(crate) rtcp_sender: mpsc::Sender<(RtcpMessage, u32)>,
}

impl PublishRTCPeerConnection {
    pub(crate) async fn new(path: String, peer: Arc<RTCPeerConnection>) -> Self {
        let (rtcp_sender, rtcp_recv) = mpsc::channel(100);
        let id = get_peer_id(&peer);
        let peer_weak = Arc::downgrade(&peer);
        tokio::spawn(Self::peer_send_rtcp(path, id.clone(), peer_weak, rtcp_recv));
        Self {
            id,
            peer,
            rtcp_sender,
        }
    }

    async fn peer_send_rtcp(
        path: String,
        id: String,
        peer: Weak<RTCPeerConnection>,
        mut recv: Receiver<(RtcpMessage, u32)>,
    ) {
        while let (Some((rtcp_message, media_ssrc)), Some(pc)) = (recv.recv().await, peer.upgrade())
        {
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
