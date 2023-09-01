use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::Result;
use log::info;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::RwLock;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp::packet::Packet;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpHeaderExtensionCapability, RTPCodecType,
};
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
use webrtc::sdp::extmap::{SDES_MID_URI, SDES_RTP_STREAM_ID_URI};
use webrtc::sdp::MediaDescription;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::{TrackLocal, TrackLocalWriter};
use webrtc::track::track_remote::TrackRemote;

use super::track_match;

type ForwardData = Arc<Packet>;

type SenderForwardData = UnboundedSender<ForwardData>;

struct PeerWrap(Arc<RTCPeerConnection>);

pub(crate) fn get_peer_key(peer: Arc<RTCPeerConnection>) -> String {
    PeerWrap(peer).get_key().to_string()
}

impl PeerWrap {
    fn get_key(&self) -> &str {
        self.0.get_stats_id()
    }
}

impl Clone for PeerWrap {
    fn clone(&self) -> Self {
        PeerWrap(self.0.clone())
    }
}

impl Eq for PeerWrap {}

impl PartialEq for PeerWrap {
    fn eq(&self, other: &Self) -> bool {
        self.get_key() == other.get_key()
    }
}

impl Hash for PeerWrap {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.get_key().hash(state);
    }
}

struct TrackRemoteWrap(Arc<TrackRemote>);

impl TrackRemoteWrap {
    fn get_key(&self) -> String {
        self.0.ssrc().to_string()
    }
}

impl Clone for TrackRemoteWrap {
    fn clone(&self) -> Self {
        TrackRemoteWrap(self.0.clone())
    }
}

impl Eq for TrackRemoteWrap {}

impl PartialEq for TrackRemoteWrap {
    fn eq(&self, other: &Self) -> bool {
        self.get_key() == other.get_key()
    }
}

impl Hash for TrackRemoteWrap {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.get_key().hash(state);
    }
}

pub(crate) struct PeerForwardInternal {
    pub(crate) id: String,
    anchor: RwLock<Option<Arc<RTCPeerConnection>>>,
    subscribe_group: RwLock<Vec<PeerWrap>>,
    anchor_track_forward_map:
        RwLock<HashMap<TrackRemoteWrap, Arc<RwLock<HashMap<PeerWrap, SenderForwardData>>>>>,
    ice_server: Vec<RTCIceServer>,
}

impl PeerForwardInternal {
    pub(crate) fn new(id: impl ToString, ice_server: Vec<RTCIceServer>) -> Self {
        PeerForwardInternal {
            id: id.to_string(),
            anchor: Default::default(),
            subscribe_group: Default::default(),
            anchor_track_forward_map: Default::default(),
            ice_server,
        }
    }

    pub(crate) async fn anchor_is_some(&self) -> bool {
        let anchor = self.anchor.read().await;
        anchor.is_some()
    }

    pub(crate) async fn anchor_is_ok(&self) -> bool {
        let anchor = self.anchor.read().await;
        let anchor_track_forward_map = self.anchor_track_forward_map.read().await;
        anchor.is_some()
            && !anchor_track_forward_map.is_empty()
            && anchor.as_ref().unwrap().connection_state() == RTCPeerConnectionState::Connected
    }

    pub(crate) async fn set_anchor(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut anchor = self.anchor.write().await;
        if anchor.is_some() {
            return Err(anyhow::anyhow!("anchor is set"));
        }
        info!("[{}] [anchor] set {}", self.id, peer.get_stats_id());
        *anchor = Some(peer);
        Ok(())
    }

    pub(crate) async fn remove_anchor(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut anchor = self.anchor.write().await;
        if anchor.is_none() {
            return Ok(());
        }
        if anchor.as_ref().unwrap().get_stats_id() != peer.get_stats_id() {
            return Err(anyhow::anyhow!("anchor not myself"));
        }
        let mut anchor_track_forward_map = self.anchor_track_forward_map.write().await;
        anchor_track_forward_map.clear();
        let mut subscribe_group = self.subscribe_group.write().await;
        for peer_wrap in subscribe_group.iter() {
            let _ = peer_wrap.0.close().await;
        }
        subscribe_group.clear();
        *anchor = None;
        info!("[{}] [anchor] set none", self.id);
        Ok(())
    }

    pub async fn add_subscribe(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut subscribe_peers = self.subscribe_group.write().await;
        subscribe_peers.push(PeerWrap(peer.clone()));
        drop(subscribe_peers);
        info!("[{}] [subscribe] [{}] up", self.id, peer.get_stats_id());
        Ok(())
    }

    pub async fn remove_subscribe(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let peer_wrap = PeerWrap(peer.clone());
        for (_, track_forward_map) in self.anchor_track_forward_map.write().await.iter() {
            let mut track_forward_map = track_forward_map.write().await;
            track_forward_map.remove(&peer_wrap);
        }
        let mut subscribe_peers = self.subscribe_group.write().await;
        subscribe_peers.retain(|x| x != &peer_wrap);
        drop(subscribe_peers);
        info!("[{}] [subscribe] [{}] down", self.id, peer.get_stats_id());
        Ok(())
    }

    pub(crate) async fn new_publish_peer(
        &self,
        media_descriptions: Vec<MediaDescription>,
    ) -> Result<Arc<RTCPeerConnection>> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        m.register_header_extension(
            RTCRtpHeaderExtensionCapability {
                uri: SDES_MID_URI.to_owned(),
            },
            RTPCodecType::Video,
            Some(RTCRtpTransceiverDirection::Recvonly),
        )?;
        m.register_header_extension(
            RTCRtpHeaderExtensionCapability {
                uri: SDES_RTP_STREAM_ID_URI.to_owned(),
            },
            RTPCodecType::Video,
            Some(RTCRtpTransceiverDirection::Recvonly),
        )?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();
        let config = RTCConfiguration {
            ice_servers: self.ice_server.clone(),
            ..Default::default()
        };
        let peer = Arc::new(api.new_peer_connection(config).await?);
        for media_description in &media_descriptions {
            let _ = peer
                .add_transceiver_from_kind(
                    RTPCodecType::from(media_description.media_name.media.as_str()),
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Recvonly,
                        send_encodings: Vec::new(),
                    }),
                )
                .await?;
        }
        Ok(peer)
    }

    pub(crate) async fn new_subscription_peer(
        &self,
        media_descriptions: Vec<MediaDescription>,
    ) -> Result<Arc<RTCPeerConnection>> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .build();
        let config = RTCConfiguration {
            ice_servers: self.ice_server.clone(),
            ..Default::default()
        };
        let peer = Arc::new(api.new_peer_connection(config).await?);
        let anchor_track_forward_map = self.anchor_track_forward_map.read().await;
        let tracks: Vec<Arc<TrackRemote>> = anchor_track_forward_map
            .iter()
            .map(|(t, _)| t.0.clone())
            .collect();
        for media_description in media_descriptions {
            if let Some(track) = track_match::track_match(&media_description, &tracks) {
                if let Ok(sender) = self
                    .new_subscription_peer_track(
                        peer.clone(),
                        track.kind(),
                        track.codec().capability,
                    )
                    .await
                {
                    let mut subscription_map = anchor_track_forward_map
                        .get(&TrackRemoteWrap(track))
                        .unwrap()
                        .write()
                        .await;
                    subscription_map.insert(PeerWrap(peer.clone()), sender);
                }
            }
        }
        Ok(peer)
    }

    async fn new_subscription_peer_track(
        &self,
        peer: Arc<RTCPeerConnection>,
        code_type: RTPCodecType,
        codec: RTCRtpCodecCapability,
    ) -> Result<SenderForwardData> {
        let track = Arc::new(TrackLocalStaticRTP::new(
            codec,
            code_type.to_string(),
            "webrtc-rs".to_owned(),
        ));
        let sender = peer
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;
        let (send, mut recv) = unbounded_channel::<ForwardData>();
        let self_id = self.id.clone();
        tokio::spawn(async move {
            info!(
                "[{}] [subscribe] [{}] {} forward up",
                self_id,
                peer.get_stats_id(),
                code_type.to_string()
            );
            let mut sequence_number: u16 = 0;
            while let Some(packet) = recv.recv().await {
                let mut packet = packet.as_ref().clone();
                packet.header.sequence_number = sequence_number;
                if let Err(err) = track.write_rtp(&packet).await {
                    info!("track write err: {}", err);
                }
                sequence_number = sequence_number.wrapping_add(1);
            }
            let _ = peer.remove_track(&sender).await;
            info!(
                "[{}] [subscribe] [{}] {} forward down",
                self_id,
                peer.get_stats_id(),
                code_type.to_string()
            );
        });
        Ok(send)
    }

    pub(crate) async fn add_ice_candidate(
        &self,
        key: String,
        ice_candidates: Vec<RTCIceCandidateInit>,
    ) -> Result<()> {
        let mut peers = self.subscribe_group.read().await.clone();
        let anchor = self.anchor.read().await.as_ref().cloned();
        if let Some(anchor) = anchor {
            peers.push(PeerWrap(anchor))
        }
        let mut peers: Vec<PeerWrap> = peers.into_iter().filter(|p| p.get_key() == key).collect();
        if peers.len() != 1 {
            return Err(anyhow::anyhow!("find key peers size : {}", peers.len()));
        }
        let peer = peers.pop().unwrap();
        for ice_candidate in ice_candidates {
            peer.0.add_ice_candidate(ice_candidate).await?;
        }
        Ok(())
    }

    pub(crate) async fn remove_peer(&self, key: String) -> Result<bool> {
        let anchor = self.anchor.write().await;
        if let Some(anchor) = anchor.as_ref() {
            if get_peer_key(anchor.clone()) == key {
                let _ = anchor.close().await?;
                return Ok(true);
            }
        }
        drop(anchor);
        let peers = self.subscribe_group.write().await.clone();
        for peer in peers {
            if peer.get_key() == key {
                let _ = peer.0.close().await?;
                break;
            }
        }
        Ok(false)
    }

    pub(crate) async fn anchor_track_up(
        &self,
        peer: Arc<RTCPeerConnection>,
        track: Arc<TrackRemote>,
    ) -> Result<()> {
        let anchor = self.anchor.read().await;
        if anchor.is_none() {
            return Err(anyhow::anyhow!("anchor is none"));
        }
        if anchor.as_ref().unwrap().get_stats_id() != peer.get_stats_id() {
            return Err(anyhow::anyhow!("anchor is not self"));
        }
        tokio::spawn(Self::anchor_track_pli(Arc::downgrade(&peer), track.ssrc()));
        let mut anchor_track_forward_map = self.anchor_track_forward_map.write().await;
        let subscription: Arc<RwLock<HashMap<PeerWrap, SenderForwardData>>> = Default::default();
        anchor_track_forward_map.insert(TrackRemoteWrap(track.clone()), subscription.clone());
        tokio::spawn(Self::anchor_track_forward(
            self.id.clone(),
            track,
            subscription,
        ));
        Ok(())
    }

    async fn anchor_track_forward(
        id: String,
        track: Arc<TrackRemote>,
        subscription: Arc<RwLock<HashMap<PeerWrap, SenderForwardData>>>,
    ) {
        let mut b = vec![0u8; 1500];
        info!(
            "[{}] [anchor] [track-{}-{}] forward up",
            id,
            track.kind(),
            track.ssrc()
        );
        while let Ok((rtp_packet, _)) = track.read(&mut b).await {
            let anchor_track_forward = subscription.read().await;
            let packet = Arc::new(rtp_packet);
            for (peer_wrap, sender) in anchor_track_forward.iter() {
                if peer_wrap.0.connection_state() != RTCPeerConnectionState::Connected {
                    continue;
                }
                let _ = sender.send(packet.clone());
            }
        }
        info!(
            "[{}] [anchor] [track-{}-{}] forward down",
            id,
            track.kind(),
            track.ssrc()
        );
    }

    async fn anchor_track_pli(peer: Weak<RTCPeerConnection>, media_ssrc: u32) {
        loop {
            let timeout = tokio::time::sleep(Duration::from_secs(1));
            tokio::pin!(timeout);
            let _ = timeout.as_mut().await;
            if let Some(pc) = peer.upgrade() {
                if pc
                    .write_rtcp(&[Box::new(PictureLossIndication {
                        sender_ssrc: 0,
                        media_ssrc,
                    })])
                    .await
                    .is_err()
                {
                    break;
                }
            } else {
                break;
            }
        }
    }
}
