use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio::sync::RwLock;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp::packet::Packet;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecParameters, RTPCodecType};
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::track::track_local::{TrackLocal, TrackLocalWriter};
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_remote::TrackRemote;

use super::constant::*;

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

impl Clone for TrackRemoteWrap {
    fn clone(&self) -> Self {
        TrackRemoteWrap(self.0.clone())
    }
}

impl Eq for TrackRemoteWrap {}

impl PartialEq for TrackRemoteWrap {
    fn eq(&self, other: &Self) -> bool {
        self.0.id() == other.0.id()
    }
}

impl Hash for TrackRemoteWrap {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id().hash(state);
    }
}

pub struct PeerForwardInternal {
    pub(crate) id: String,
    anchor: RwLock<Option<Arc<RTCPeerConnection>>>,
    subscribe_group: RwLock<Vec<PeerWrap>>,
    anchor_track_codec_map: RwLock<HashMap<String, RTCRtpCodecParameters>>,
    anchor_track_forward_map: HashMap<String, RwLock<HashMap<PeerWrap, SenderForwardData>>>,
}

impl PeerForwardInternal {
    pub(crate) fn new(id: impl ToString) -> Self {
        let mut anchor_track_forward_map = HashMap::new();
        anchor_track_forward_map.insert(VIDEO_KIND.to_owned(), Default::default());
        anchor_track_forward_map.insert(AUDIO_KIND.to_owned(), Default::default());
        PeerForwardInternal {
            id: id.to_string(),
            anchor: Default::default(),
            subscribe_group: Default::default(),
            anchor_track_codec_map: Default::default(),
            anchor_track_forward_map,
        }
    }

    fn get_anchor_track_key(&self, track: Arc<TrackRemote>) -> String {
        track.kind().to_string()
    }

    pub(crate) async fn anchor_is_some(&self) -> bool {
        let anchor = self.anchor.read().await;
        anchor.is_some()
    }

    pub(crate) async fn set_anchor(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut anchor = self.anchor.write().await;
        if anchor.is_some() {
            return Err(anyhow::anyhow!("anchor is set"));
        }
        println!("[{}] [anchor] set {}", self.id, peer.get_stats_id());
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
        let subscribe_group = self.subscribe_group.read().await;
        if subscribe_group.is_empty() {
            let mut anchor_track_type_map = self.anchor_track_codec_map.write().await;
            anchor_track_type_map.clear();
        }
        *anchor = None;
        println!("[{}] [anchor] set none", self.id);
        Ok(())
    }

    pub async fn add_subscribe(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut subscribe_peers = self.subscribe_group.write().await;
        subscribe_peers.push(PeerWrap(peer.clone()));
        drop(subscribe_peers);
        println!("[{}] [subscribe] [{}] up", self.id, peer.get_stats_id());
        let _ = self.refresh_subscribe().await?;
        Ok(())
    }

    pub async fn remove_subscribe(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut subscribe_peers = self.subscribe_group.write().await;
        subscribe_peers.retain(|x| x != &PeerWrap(peer.clone()));
        drop(subscribe_peers);
        println!("[{}] [subscribe] [{}] down", self.id, peer.get_stats_id());
        let _ = self.refresh_subscribe().await?;
        Ok(())
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
        tokio::spawn(PeerForwardInternal::anchor_track_pli(
            Arc::downgrade(&peer),
            track.ssrc(),
        ));
        let track_key = self.get_anchor_track_key(track.clone());
        let mut anchor_track_type_map = self.anchor_track_codec_map.write().await;
        anchor_track_type_map.insert(track_key, track.codec());
        Ok(())
    }

    pub(crate) async fn refresh_subscribe(&self) -> Result<()> {
        let subscribe_group = self.subscribe_group.read().await;
        for (kind, track_forward_map) in self.anchor_track_forward_map.iter() {
            let mut track_forward_map = track_forward_map.write().await;
            track_forward_map.retain(|k, _| subscribe_group.contains(k));
            for peer_wrap in subscribe_group.iter() {
                if !track_forward_map.contains_key(peer_wrap) {
                    if let Ok(sender) = self
                        .peer_add_track(peer_wrap.0.clone(), kind.as_str())
                        .await
                    {
                        track_forward_map.insert(peer_wrap.clone(), sender);
                    }
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn anchor_track_forward(&self, track: Arc<TrackRemote>) {
        let mut b = vec![0u8; 1500];
        let track_key = self.get_anchor_track_key(track.clone());
        println!("[{}] [anchor] [track-{}] forward up", self.id, track_key);
        while let Ok((rtp_packet, _)) = track.read(&mut b).await {
            let anchor_track_forward = self.anchor_track_forward_map.get(&track_key).unwrap();
            let anchor_track_forward = anchor_track_forward.read().await;
            let packet = Arc::new(rtp_packet);
            for (peer_wrap, sender) in anchor_track_forward.iter() {
                if peer_wrap.0.connection_state() != RTCPeerConnectionState::Connected {
                    continue;
                }
                let _ = sender.send(packet.clone());
            }
        }
        println!("[{}] [anchor] [track-{}] forward down", self.id, track_key);
    }

    pub(crate) async fn new_peer(&self, publish: bool) -> Result<Arc<RTCPeerConnection>> {
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
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
        let peer = Arc::new(api.new_peer_connection(config).await?);
        if publish {
            let video_transceiver = peer
                .add_transceiver_from_kind(
                    RTPCodecType::Video,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Recvonly,
                        send_encodings: Vec::new(),
                    }),
                )
                .await?;
            let audio_transceiver = peer.add_transceiver_from_kind(
                RTPCodecType::Audio,
                Some(RTCRtpTransceiverInit {
                    direction: RTCRtpTransceiverDirection::Recvonly,
                    send_encodings: Vec::new(),
                }),
            ).await?;
            let track_codec_map = self.anchor_track_codec_map.read().await;
            if let Some(codec) = track_codec_map.get(VIDEO_KIND) {
                video_transceiver.set_codec_preferences(vec![codec.clone()]).await?;
            }
            if let Some(codec) = track_codec_map.get(AUDIO_KIND) {
                audio_transceiver.set_codec_preferences(vec![codec.clone()]).await?;
            }
        }
        Ok(peer)
    }
    async fn peer_add_track(
        &self,
        peer: Arc<RTCPeerConnection>,
        kind: &str,
    ) -> Result<SenderForwardData> {
        let anchor_track_codec_map = self.anchor_track_codec_map.read().await;
        let codec = anchor_track_codec_map.get(kind);
        if codec.is_none() {
            return Err(anyhow::anyhow!("kind codec not found"));
        }
        let codec = codec.unwrap().clone().capability;
        drop(anchor_track_codec_map);
        let track = Arc::new(TrackLocalStaticRTP::new(
            codec,
            kind.to_owned(),
            "webrtc-rs".to_owned(),
        ));
        let sender = peer
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;
        let (send, mut recv) = unbounded_channel::<ForwardData>();
        let self_id = self.id.clone();
        let kind = kind.to_owned();
        tokio::spawn(async move {
            println!(
                "[{}] [subscribe] [{}] {} forward up",
                self_id,
                peer.get_stats_id(),
                kind
            );
            let mut sequence_number: u16 = 0;
            while let Some(packet) = recv.recv().await {
                let mut packet = packet.as_ref().clone();
                packet.header.sequence_number = sequence_number;
                if let Err(err) = track.write_rtp(&packet).await {
                    println!("track write err: {}", err);
                }
                sequence_number = sequence_number.wrapping_add(1);
            }
            let _ = peer.remove_track(&sender).await;
            println!(
                "[{}] [subscribe] [{}] {} forward down",
                self_id,
                peer.get_stats_id(),
                kind
            );
        });
        Ok(send)
    }

    pub(crate) async fn add_ice_candidate(
        &self,
        key: String,
        ice_candidates: Vec<RTCIceCandidateInit>,
        whip: bool,
    ) -> Result<()> {
        let peer = match whip {
            true => {
                let anchor = self.anchor.read().await;
                if anchor.is_none() {
                    return Err(anyhow::anyhow!("anchor is none"));
                }
                let peer = anchor.as_ref().unwrap().clone();
                if PeerWrap(peer.clone()).get_key() != key.as_str() {
                    return Err(anyhow::anyhow!("key not match"));
                }
                peer
            }
            false => {
                let subscribe_peers = self.subscribe_group.read().await;
                let mut peers: Vec<Arc<RTCPeerConnection>> = subscribe_peers
                    .iter()
                    .filter(|peer_warap| peer_warap.get_key() == key.as_str())
                    .map(|peer_warap| peer_warap.0.clone())
                    .collect();
                if peers.len() != 1 {
                    return Err(anyhow::anyhow!("peer not found"));
                }
                peers.pop().unwrap()
            }
        };
        for ice_candidate in ice_candidates {
            peer.add_ice_candidate(ice_candidate).await?;
        }
        Ok(())
    }

    async fn anchor_track_pli(peer: Weak<RTCPeerConnection>, media_ssrc: u32) {
        // Send a PLI on an interval so that the publisher is pushing a keyframe every rtcpPLIInterval
        let mut result = Result::<usize>::Ok(0);
        while result.is_ok() {
            let timeout = tokio::time::sleep(Duration::from_secs(1));
            tokio::pin!(timeout);
            tokio::select! {
                _ = timeout.as_mut() =>{
                    if let Some(pc) = peer.upgrade(){
                        result = pc.write_rtcp(&[Box::new(PictureLossIndication{
                            sender_ssrc: 0,
                            media_ssrc,
                        })]).await.map_err(Into::into);
                    }else{
                        break;
                    }
                }
            }
        }
    }
}
