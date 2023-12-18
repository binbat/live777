use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};

use anyhow::Result;
use log::{debug, info};
use tokio::sync::mpsc::{channel, unbounded_channel, Receiver, Sender, UnboundedSender};
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
use webrtc::rtcp::reception_report::ReceptionReport;
use webrtc::rtcp::sender_report::SenderReport;
use webrtc::rtp::packet::Packet;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpHeaderExtensionCapability, RTPCodecType,
};
use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
use webrtc::sdp::extmap::{SDES_MID_URI, SDES_RTP_STREAM_ID_URI};
use webrtc::sdp::MediaDescription;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::{TrackLocal, TrackLocalWriter};
use webrtc::track::track_remote::TrackRemote;

use crate::forward::info::Layer;
use crate::media;
use crate::AppError;

use super::rtcp::RtcpMessage;
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

type SubscriptionGroup = Arc<RwLock<HashMap<PeerWrap, SenderForwardData>>>;

#[derive(Clone)]
struct TrackForward {
    rtcp_send: Sender<RtcpMessage>,
    subscription_group: SubscriptionGroup,
}

pub(crate) struct PeerForwardInternal {
    pub(crate) id: String,
    anchor: RwLock<Option<Arc<RTCPeerConnection>>>,
    subscribe_group: RwLock<Vec<PeerWrap>>,
    anchor_track_forward_map: Arc<RwLock<HashMap<TrackRemoteWrap, TrackForward>>>,
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
            && anchor_track_forward_map.len()
                == media::count_sends(
                    &anchor
                        .as_ref()
                        .unwrap()
                        .remote_description()
                        .await
                        .unwrap()
                        .unmarshal()
                        .unwrap()
                        .media_descriptions,
                )
            && anchor.as_ref().unwrap().connection_state() == RTCPeerConnectionState::Connected
    }

    pub(crate) async fn set_anchor(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut anchor = self.anchor.write().await;
        if anchor.is_some() {
            return Err(AppError::ResourceAlreadyExists(
                "A connection has already been established".to_string(),
            )
            .into());
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

    pub async fn publish_is_svc(&self) -> bool {
        self.publish_track_remotes(RTPCodecType::Video).await.len() > 1
    }

    async fn publish_track_remotes(&self, code_type: RTPCodecType) -> Vec<Arc<TrackRemote>> {
        let anchor_track_forward_map = self.anchor_track_forward_map.read().await;
        let mut video_track_remotes = vec![];
        for (track_remote_wrap, _) in anchor_track_forward_map.iter() {
            if track_remote_wrap.0.kind() == code_type {
                video_track_remotes.push(track_remote_wrap.0.clone());
            }
        }
        video_track_remotes
    }

    pub async fn publish_svc_rids(&self) -> Result<Vec<String>> {
        let anchor = self.anchor.read().await.as_ref().cloned();
        if let Some(pc) = anchor {
            if let Some(rd) = pc.remote_description().await {
                let mds = rd.unmarshal()?.media_descriptions;
                for md in mds {
                    if RTPCodecType::from(md.media_name.media.as_str()) == RTPCodecType::Video {
                        return Ok(media::rids(&md));
                    }
                }
            }
        }
        Err(anyhow::anyhow!("anchor svc rids error"))
    }

    pub async fn select_layer(&self, key: String, layer: Option<Layer>) -> Result<()> {
        let rid = if let Some(layer) = layer {
            layer.encoding_id
        } else {
            self.publish_svc_rids().await?[0].clone()
        };
        let peer = self
            .subscribe_group
            .read()
            .await
            .iter()
            .find(|p| p.get_key() == key)
            .cloned();
        if let Some(peer) = peer {
            let anchor_track_forward_map = self.anchor_track_forward_map.write().await;
            for (track_remote, track_forward) in anchor_track_forward_map.iter() {
                if track_remote.0.rid() == rid && track_remote.0.kind() == RTPCodecType::Video {
                    for (track_remote_original, track_forward_original) in
                        anchor_track_forward_map.iter()
                    {
                        if track_remote_original.0.kind() != RTPCodecType::Video {
                            continue;
                        }
                        let mut subscription_group =
                            track_forward_original.subscription_group.write().await;
                        if subscription_group.contains_key(&peer) {
                            if track_remote_original.0.rid() == rid {
                                return Ok(());
                            }
                            let sender = subscription_group.remove(&peer).unwrap();
                            drop(subscription_group);
                            track_forward
                                .subscription_group
                                .write()
                                .await
                                .insert(peer.clone(), sender);
                            let _ = track_forward
                                .rtcp_send
                                .try_send(RtcpMessage::PictureLossIndication);
                            info!(
                                "[{}] [subscribe] [{}] select layer {} to {} ",
                                self.id,
                                peer.get_key(),
                                track_remote_original.0.rid(),
                                rid
                            );
                            return Ok(());
                        }
                    }
                }
            }
            Err(anyhow::anyhow!("not found layer"))
        } else {
            Err(anyhow::anyhow!("not found key"))
        }
    }

    pub async fn remove_subscribe(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let peer_wrap = PeerWrap(peer.clone());
        for (_, track_forward) in self.anchor_track_forward_map.write().await.iter() {
            let mut subscription_group = track_forward.subscription_group.write().await;
            subscription_group.remove(&peer_wrap);
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
            let rids = if RTPCodecType::from(media_description.media_name.media.as_str())
                == RTPCodecType::Video
                && self.publish_is_svc().await
            {
                Some(self.publish_svc_rids().await?)
            } else {
                None
            };
            if let Some(track) = track_match::track_match(&media_description, &tracks, rids) {
                if let Ok(sender) = self
                    .new_subscription_peer_track(
                        peer.clone(),
                        track.kind(),
                        track.codec().capability,
                    )
                    .await
                {
                    let mut subscription_group = anchor_track_forward_map
                        .get(&TrackRemoteWrap(track))
                        .unwrap()
                        .subscription_group
                        .write()
                        .await;
                    subscription_group.insert(PeerWrap(peer.clone()), sender);
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
        let ssrc = sender.get_parameters().await.encodings.pop().unwrap().ssrc;
        let _ = peer
            .write_rtcp(&[Box::new(SenderReport {
                ssrc,
                reports: vec![ReceptionReport {
                    ssrc,
                    ..Default::default()
                }],
                ..Default::default()
            })])
            .await;
        tokio::spawn(Self::subscribe_read_rtcp(
            Arc::downgrade(&peer),
            sender,
            self.anchor_track_forward_map.clone(),
        ));
        let (send, mut recv) = unbounded_channel::<ForwardData>();
        let self_id = self.id.clone();
        let peer_stats_id = peer.get_stats_id().to_string();
        tokio::spawn(async move {
            info!(
                "[{}] [subscribe] [{}] {} forward up",
                self_id,
                peer_stats_id,
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
            info!(
                "[{}] [subscribe] [{}] {} forward down",
                self_id,
                peer_stats_id,
                code_type.to_string()
            );
        });
        Ok(send)
    }

    async fn subscribe_read_rtcp(
        pc: Weak<RTCPeerConnection>,
        sender: Arc<RTCRtpSender>,
        track_forward_map: Arc<RwLock<HashMap<TrackRemoteWrap, TrackForward>>>,
    ) {
        while let (Ok((packets, _)), Some(pc)) = (sender.read_rtcp().await, pc.upgrade()) {
            for packet in packets {
                if let Some(msg) = RtcpMessage::from_rtcp_packet(packet) {
                    if let Some(track) = sender.track().await {
                        let kind = track.kind();
                        let track_forward_map = track_forward_map.read().await;
                        for (track_remote, track_forward) in track_forward_map.iter() {
                            if track_remote.0.kind() == kind {
                                let subscription_group =
                                    track_forward.subscription_group.read().await;
                                if subscription_group.contains_key(&PeerWrap(pc.clone())) {
                                    let _ = track_forward.rtcp_send.try_send(msg);
                                }
                            }
                        }
                    }
                }
            }
        }
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
        let anchor = self.anchor.read().await;
        if let Some(anchor) = anchor.as_ref() {
            if get_peer_key(anchor.clone()) == key {
                anchor.close().await?;
                return Ok(true);
            }
        }
        drop(anchor);
        let peers = self.subscribe_group.read().await;
        for peer in peers.iter() {
            if peer.get_key() == key {
                peer.0.close().await?;
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
        let (send, recv) = channel(1);
        tokio::spawn(Self::peer_send_rtcp(
            Arc::downgrade(&peer),
            track.ssrc(),
            recv,
        ));
        let mut anchor_track_forward_map = self.anchor_track_forward_map.write().await;
        let handle = TrackForward {
            rtcp_send: send,
            subscription_group: Default::default(),
        };
        anchor_track_forward_map.insert(TrackRemoteWrap(track.clone()), handle.clone());
        tokio::spawn(Self::anchor_track_forward(
            self.id.clone(),
            track,
            handle.subscription_group,
        ));
        Ok(())
    }

    async fn anchor_track_forward(
        id: String,
        track: Arc<TrackRemote>,
        subscription: SubscriptionGroup,
    ) {
        let mut b = vec![0u8; 1500];
        info!(
            "[{}] [anchor] [track-{}-{}-{}] forward up",
            id,
            track.kind(),
            track.ssrc(),
            track.rid()
        );
        while let Ok((rtp_packet, _)) = track.read(&mut b).await {
            if let Ok(anchor_track_forward) = subscription.try_read() {
                let packet = Arc::new(rtp_packet);
                for (peer_wrap, sender) in anchor_track_forward.iter() {
                    if peer_wrap.0.connection_state() == RTCPeerConnectionState::Connected {
                        let _ = sender.send(packet.clone());
                    }
                }
            }
        }
        info!(
            "[{}] [anchor] [track-{}-{}-{}] forward down",
            id,
            track.kind(),
            track.ssrc(),
            track.rid()
        );
    }

    async fn peer_send_rtcp(
        peer: Weak<RTCPeerConnection>,
        media_ssrc: u32,
        mut recv: Receiver<RtcpMessage>,
    ) {
        while let (Some(rtcp_message), Some(pc)) = (recv.recv().await, peer.upgrade()) {
            debug!("ssrc : {} ,send rtcp : {:?}", media_ssrc, rtcp_message);
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
