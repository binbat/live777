use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{Mutex, RwLock};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use uuid::Uuid;
use webrtc::api::media_engine::{MIME_TYPE_OPUS, MIME_TYPE_VP8};
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp::packet::Packet;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::{TrackLocal, TrackLocalWriter};
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_remote::TrackRemote;

const VIDEO_KIND: &str = "video";
const AUDIO_KIND: &str = "audio";

type ForwardData = Arc<Packet>;

type SenderForwardData = UnboundedSender<ForwardData>;

struct PeerWrap(Arc<RTCPeerConnection>);

impl Clone for PeerWrap {
    fn clone(&self) -> Self {
        PeerWrap(self.0.clone())
    }
}

impl Eq for PeerWrap {}

impl PartialEq for PeerWrap {
    fn eq(&self, other: &Self) -> bool {
        self.0.get_stats_id() == other.0.get_stats_id()
    }

    fn ne(&self, other: &Self) -> bool {
        self.0.get_stats_id() != other.0.get_stats_id()
    }
}

impl Hash for PeerWrap {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.get_stats_id().hash(state);
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

    fn ne(&self, other: &Self) -> bool {
        self.0.id() != other.0.id()
    }
}

impl Hash for TrackRemoteWrap {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id().hash(state);
    }
}

pub struct PeerForwardInternal {
    pub(crate) id: String,
    pub(crate) kind_many: bool,
    anchor: Arc<RwLock<Option<Arc<RTCPeerConnection>>>>,
    subscribe_group: Arc<RwLock<Vec<PeerWrap>>>,
    anchor_track_forward_map:
    Arc<RwLock<HashMap<TrackRemoteWrap, Arc<RwLock<HashMap<PeerWrap, SenderForwardData>>>>>>,
    anchor_track_forward_map_retain:
    Arc<Mutex<HashMap<String, Arc<RwLock<HashMap<PeerWrap, SenderForwardData>>>>>>,
}

impl PeerForwardInternal {
    pub(crate) fn new(id: impl ToString, kind_many: bool) -> Self {
        PeerForwardInternal {
            id: id.to_string(),
            kind_many,
            anchor: Arc::new(Default::default()),
            subscribe_group: Arc::new(Default::default()),
            anchor_track_forward_map: Arc::new(Default::default()),
            anchor_track_forward_map_retain: Arc::new(Default::default()),
        }
    }

    fn get_anchor_track_retain_key(&self, track: Arc<TrackRemote>) -> String {
        match self.kind_many {
            true => track.id(),
            false => track.kind().to_string(),
        }
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
        let mut anchor_track_forward_map = self.anchor_track_forward_map.write().await;
        let mut anchor_track_forward_map_retain = self.anchor_track_forward_map_retain.lock().await;
        for (track_wrap, track_forward) in anchor_track_forward_map.iter() {
            anchor_track_forward_map_retain.insert(
                self.get_anchor_track_retain_key(track_wrap.0.clone()),
                track_forward.clone(),
            );
        }
        anchor_track_forward_map.clear();
        *anchor = None;
        println!("[{}] [anchor] set none", self.id);
        Ok(())
    }

    pub async fn add_subscribe(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut subscribe_peers = self.subscribe_group.write().await;
        subscribe_peers.push(PeerWrap(peer.clone()));
        drop(subscribe_peers);
        println!("[{}] [subscribe] [{}] up", self.id, peer.get_stats_id());
        let _ = self.refresh_subscribe_peers().await?;
        Ok(())
    }

    pub async fn remove_subscribe(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut subscribe_peers = self.subscribe_group.write().await;
        subscribe_peers.retain(|x| x != &PeerWrap(peer.clone()));
        drop(subscribe_peers);
        println!("[{}] [subscribe] [{}] down", self.id, peer.get_stats_id());
        let _ = self.refresh_subscribe_peers().await;
        Ok(())
    }

    pub(crate) async fn anchor_track_up(
        &self,
        peer: Arc<RTCPeerConnection>,
        track: Arc<TrackRemote>,
    ) -> Result<()> {
        println!(
            "[{}] [anchor] [{}] [track-{}] kind : {}",
            self.id,
            peer.get_stats_id(),
            track.id(),
            track.kind().to_string()
        );
        if !PeerForwardInternal::track_kind_support(track.clone()) {
            return Err(anyhow::anyhow!("track kind not support"));
        }
        let anchor = self.anchor.read().await;
        if anchor.is_none() {
            return Err(anyhow::anyhow!("anchor is none"));
        }
        if anchor.as_ref().unwrap().get_stats_id() != peer.get_stats_id() {
            return Err(anyhow::anyhow!("anchor is not self"));
        }
        let mut anchor_track_forward = self.anchor_track_forward_map.write().await;
        if !self.kind_many {
            for (track_wrap, _) in anchor_track_forward.iter() {
                if track_wrap.0.kind() == track.kind() {
                    return Err(anyhow::anyhow!("track kind exist"));
                }
            }
        }
        let mut anchor_track_forward_map_retain = self.anchor_track_forward_map_retain.lock().await;
        let track_forward = anchor_track_forward_map_retain
            .remove(&self.get_anchor_track_retain_key(track.clone()))
            .map_or_else(
                || Arc::new(RwLock::default()),
                |track_forward| track_forward,
            );
        anchor_track_forward.insert(TrackRemoteWrap(track.clone()), track_forward);
        drop(anchor_track_forward_map_retain);
        drop(anchor_track_forward);
        tokio::spawn(PeerForwardInternal::anchor_track_pli(
            Arc::downgrade(&peer),
            track.ssrc(),
        ));
        let _ = self.refresh_track_forward(track).await;
        Ok(())
    }

    async fn refresh_subscribe_peers(&self) -> Result<()> {
        let anchor_track_forward_map_retain = self.anchor_track_forward_map_retain.lock().await;
        let peers = self.subscribe_group.read().await;
        for (_, peers_forwards) in anchor_track_forward_map_retain.iter() {
            let mut peers_forwards = peers_forwards.write().await;
            let remove_peer_keys: Vec<PeerWrap> = peers_forwards
                .iter()
                .map(|(p, _)| p.clone())
                .filter(|p| !peers.contains(p))
                .collect();
            for peer_wrap in remove_peer_keys.iter() {
                peers_forwards.remove(peer_wrap);
            }
        }
        drop(peers);
        drop(anchor_track_forward_map_retain);
        let anchor = self.anchor.read().await;
        if anchor.is_none() {
            return Ok(());
        }
        drop(anchor);
        let anchor_track_forward_map = self.anchor_track_forward_map.read().await;
        let tracks: Vec<Arc<TrackRemote>> = anchor_track_forward_map
            .iter()
            .map(|(track, _)| track.0.clone())
            .collect();
        drop(anchor_track_forward_map);
        for track in tracks {
            let _ = self.refresh_track_forward(track.clone()).await;
        }
        Ok(())
    }

    async fn refresh_track_forward(&self, track: Arc<TrackRemote>) {
        println!(
            "[{}] [anchor] [track-{}] refresh forward",
            self.id,
            track.id()
        );
        let kind = track.kind().to_string();
        let kind = kind.as_str();
        let anchor_track_forward_map = self.anchor_track_forward_map.read().await;
        let anchor_track_forward = anchor_track_forward_map.get(&TrackRemoteWrap(track.clone()));
        if anchor_track_forward.is_none() {
            return;
        }
        let anchor_track_forward = anchor_track_forward.unwrap().clone();
        drop(anchor_track_forward_map);
        let mut anchor_track_forward = anchor_track_forward.write().await;
        let peers = self.subscribe_group.read().await;
        for peer in peers.iter() {
            if anchor_track_forward.get(peer).is_none() {
                if let Ok(sender) = self.peer_add_track(peer.0.clone(), kind).await {
                    anchor_track_forward.insert(peer.clone(), sender);
                }
            }
        }
        let sile: Vec<PeerWrap> = anchor_track_forward
            .iter()
            .filter(|(p, _)| !peers.contains(*p))
            .map(|(p, _)| (*p).clone())
            .collect();
        for x in sile {
            anchor_track_forward.remove(&x);
        }
    }

    pub(crate) async fn anchor_track_forward(&self, track: Arc<TrackRemote>) {
        let mut b = vec![0u8; 1500];
        println!("[{}] [anchor] [track-{}] forward up", self.id, track.id());
        while let Ok((rtp_packet, _)) = track.read(&mut b).await {
            let anchor_track_forward_map = self.anchor_track_forward_map.read().await;
            let anchor_track_forward =
                anchor_track_forward_map.get(&TrackRemoteWrap(track.clone()));
            if anchor_track_forward.is_none() {
                break;
            }
            let anchor_track_forward = anchor_track_forward.unwrap().read().await;
            let senders: Vec<SenderForwardData> = anchor_track_forward
                .iter()
                .map(|(_, sender)| sender.clone())
                .collect();
            drop(anchor_track_forward);
            let packet = Arc::new(rtp_packet);
            for sender in senders.iter() {
                let _ = sender.send(packet.clone());
            }
        }
        println!("[{}] [anchor] [track-{}] forward down", self.id, track.id());
    }

    async fn peer_add_track(
        &self,
        peer: Arc<RTCPeerConnection>,
        kind: &str,
    ) -> Result<SenderForwardData> {
        let uuid = Uuid::new_v4().to_string();
        let (mime_type, id, stream_id) = match kind {
            VIDEO_KIND => (
                MIME_TYPE_VP8.to_owned(),
                format!("{}-{}", VIDEO_KIND, uuid),
                format!("webrtc-rs-video-{}", uuid),
            ),
            AUDIO_KIND => (
                MIME_TYPE_OPUS.to_owned(),
                format!("{}-{}", AUDIO_KIND, uuid),
                format!("webrtc-rs-audio-{}", uuid),
            ),
            _ => return Err(anyhow::anyhow!("kind error")),
        };
        let track = Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type,
                ..Default::default()
            },
            id,
            stream_id,
        ));
        let sender = peer
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;
        let (send, mut recv) = unbounded_channel::<ForwardData>();
        let self_id = self.id.clone();
        tokio::spawn(async move {
            println!(
                "[{}] [subscribe] [{}] forward up",
                self_id,
                peer.get_stats_id()
            );
            let mut sequence_number: u16 = 0;
            while let Some(packet) = recv.recv().await {
                let mut packet = packet.as_ref().clone();
                packet.header.sequence_number = sequence_number;
                if let Err(err) = track.write_rtp(&packet).await {
                    println!("video_track.write err: {}", err);
                }
                sequence_number = (sequence_number + 1) % 65535;
            }
            let _ = peer.remove_track(&sender).await;
            println!(
                "[{}] [subscribe] [{}] forward down",
                self_id,
                peer.get_stats_id()
            );
        });
        Ok(send)
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

    fn track_kind_support(track: Arc<TrackRemote>) -> bool {
        let kind = track.kind().to_string();
        let kind = kind.as_str();
        return kind == VIDEO_KIND || kind == AUDIO_KIND;
    }
}
