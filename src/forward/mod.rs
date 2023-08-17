use std::sync::{Arc, Mutex};

use anyhow::Result;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_OPUS, MIME_TYPE_VP8};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::PayloadType;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType,
};

use crate::forward::forward_internal::PeerForwardInternal;

mod forward_internal;

const VIDEO_PAYLOAD_TYPE: PayloadType = 96;
const AUDIO_PAYLOAD_TYPE: PayloadType = 111;

#[derive(Clone)]
pub struct PeerForward {
    anchor_lock: Arc<Mutex<()>>,
    internal: Arc<PeerForwardInternal>,
}

impl PeerForward {
    pub fn new(id: impl ToString, kind_many: bool) -> Self {
        PeerForward {
            anchor_lock: Arc::new(Mutex::new(())),
            internal: Arc::new(PeerForwardInternal::new(id, kind_many)),
        }
    }

    pub fn get_id(&self) -> String {
        self.internal.id.clone()
    }

    pub async fn set_anchor(&self, offer: RTCSessionDescription) -> Result<RTCSessionDescription> {
        if self.internal.anchor_is_some().await {
            return Err(anyhow::anyhow!("anchor is set"));
        }
        let _ = self.anchor_lock.lock();
        if self.internal.anchor_is_some().await {
            return Err(anyhow::anyhow!("anchor is set"));
        }
        let peer = new_peer().await?;
        let internal = Arc::downgrade(&self.internal);
        let pc = Arc::downgrade(&peer);
        peer.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            let internal = internal.upgrade();
            let pc = pc.upgrade();
            if internal.is_some() && pc.is_some() {
                let internal = internal.unwrap();
                let peer = pc.unwrap();
                tokio::spawn(async move {
                    println!(
                        "[{}] [anchor] [{}] connection state changed: {}",
                        internal.id,
                        peer.get_stats_id(),
                        s
                    );
                    match s {
                        RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                            let _ = peer.close().await;
                        }
                        RTCPeerConnectionState::Closed => {
                            let _ = internal.remove_anchor(peer).await;
                        }
                        _ => {}
                    };
                });
            }

            Box::pin(async {})
        }));
        let internal = Arc::downgrade(&self.internal);
        let pc = Arc::downgrade(&peer);
        peer.on_track(Box::new(move |track, _, _| {
            let internal = internal.upgrade();
            let peer = pc.upgrade();
            if internal.is_some() && peer.is_some() {
                let internal = internal.unwrap();
                let peer = peer.unwrap();
                tokio::spawn(async move {
                    let _ = internal.anchor_track_up(peer, track.clone()).await;
                    let _ = internal.anchor_track_forward(track).await;
                });
            };
            Box::pin(async {})
        }));
        let _ = peer.set_remote_description(offer).await?;
        let answer = peer.create_answer(None).await?;
        let mut gather_complete = peer.gathering_complete_promise().await;
        let _ = peer.set_local_description(answer).await?;
        let _ = gather_complete.recv().await;
        let description = peer
            .local_description()
            .await
            .ok_or(anyhow::anyhow!("failed to get local description"))?;
        self.internal.set_anchor(peer).await?;
        Ok(description)
    }

    pub async fn add_subscribe(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<RTCSessionDescription> {
        let peer = new_peer().await?;
        let internal = self.internal.clone();
        let pc = peer.clone();
        peer.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            let internal = Arc::downgrade(&internal);
            let pc = Arc::downgrade(&pc);
            tokio::spawn(async move {
                let internal = internal.upgrade();
                let pc = pc.upgrade();
                if internal.is_some() && pc.is_some() {
                    let internal = internal.unwrap();
                    let pc = pc.unwrap();
                    println!(
                        "[{}] [subscribe] [{}] connection state changed: {}",
                        internal.id,
                        pc.get_stats_id(),
                        s
                    );
                    match s {
                        RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                            let _ = pc.close().await;
                        }
                        RTCPeerConnectionState::Closed => {
                            let _ = internal.remove_subscribe(pc).await;
                        }
                        RTCPeerConnectionState::Connected => {
                            let _ = internal.add_subscribe(pc).await;
                        }
                        _ => {}
                    }
                }
            });
            Box::pin(async {})
        }));
        let _ = peer.set_remote_description(offer).await?;
        let answer = peer.create_answer(None).await?;
        let mut gather_complete = peer.gathering_complete_promise().await;
        let _ = peer.set_local_description(answer).await?;
        let _ = gather_complete.recv().await;
        let description = peer
            .local_description()
            .await
            .ok_or(anyhow::anyhow!("failed to get local description"))?;
        Ok(description)
    }
}

async fn new_peer() -> Result<Arc<RTCPeerConnection>> {
    let mut m = MediaEngine::default();
    m.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_VP8.to_owned(),
                clock_rate: 90000,
                channels: 0,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: VIDEO_PAYLOAD_TYPE,
            ..Default::default()
        },
        RTPCodecType::Video,
    )?;

    m.register_codec(
        RTCRtpCodecParameters {
            capability: RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48000,
                channels: 2,
                sdp_fmtp_line: "".to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: AUDIO_PAYLOAD_TYPE,
            ..Default::default()
        },
        RTPCodecType::Audio,
    )?;

    let mut registry = Registry::new();

    registry = register_default_interceptors(registry, &mut m)?;

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();

    let config = RTCConfiguration {
        ice_servers: vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }],
        ..Default::default()
    };
    return Ok(Arc::new(api.new_peer_connection(config).await?));
}
