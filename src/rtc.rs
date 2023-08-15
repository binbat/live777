use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc::{channel, Sender};
use tokio::sync::RwLock;
use uuid::Uuid;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_OPUS, MIME_TYPE_VP8};
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp::packet::Packet;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType,
};
use webrtc::rtp_transceiver::PayloadType;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::{TrackLocal, TrackLocalWriter};
use webrtc::track::track_remote::TrackRemote;

pub const VIDEO_KIND: &str = "video";
pub const AUDIO_KIND: &str = "audio";
pub const VIDEO_PAYLOAD_TYPE: PayloadType = 96;
pub const AUDIO_PAYLOAD_TYPE: PayloadType = 111;

type ForwardData = Arc<Packet>;

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

pub struct PeerForward {
    id: String,
    kind_many: bool,
    anchor: Arc<RwLock<Option<Arc<RTCPeerConnection>>>>,
    subscribe_group: Arc<RwLock<Vec<PeerWrap>>>,
    anchor_track_forward_map_old:
        Arc<RwLock<HashMap<String, Arc<RwLock<HashMap<PeerWrap, Sender<ForwardData>>>>>>>,
    anchor_track_forward_map:
        Arc<RwLock<HashMap<TrackRemoteWrap, Arc<RwLock<HashMap<PeerWrap, Sender<ForwardData>>>>>>>,
}

impl Clone for PeerForward {
    fn clone(&self) -> Self {
        PeerForward {
            id: self.id.clone(),
            kind_many: self.kind_many,
            anchor: self.anchor.clone(),
            subscribe_group: self.subscribe_group.clone(),
            anchor_track_forward_map_old: self.anchor_track_forward_map_old.clone(),
            anchor_track_forward_map: self.anchor_track_forward_map.clone(),
        }
    }
}

impl PeerForward {
    pub fn new(id: impl ToString, kind_many: bool) -> Self {
        PeerForward {
            id: id.to_string(),
            kind_many,
            anchor: Arc::new(Default::default()),
            subscribe_group: Arc::new(Default::default()),
            anchor_track_forward_map_old: Arc::new(Default::default()),
            anchor_track_forward_map: Arc::new(Default::default()),
        }
    }

    pub fn get_id(&self) -> String {
        self.id.clone()
    }

    pub async fn set_anchor(&self, offer: RTCSessionDescription) -> Result<RTCSessionDescription> {
        let mut anchor = self.anchor.write().await;
        if anchor.is_some() {
            return Err(anyhow::anyhow!("anchor is set"));
        }
        let peer = PeerForward::new_peer().await?;
        let self_arc = Arc::new(self.clone());
        let pc = peer.clone();
        peer.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            let self_arc = self_arc.clone();
            let pc = pc.clone();
            tokio::spawn(async move {
                println!(
                    "[{}] [anchor] [{}] connection state changed: {}",
                    self_arc.get_id(),
                    pc.get_stats_id(),
                    s
                );
                match s {
                    RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                        let _ = pc.close().await;
                    }
                    RTCPeerConnectionState::Closed => {
                        let mut anchor = self_arc.anchor.write().await;
                        if anchor.is_some()
                            && anchor.as_ref().unwrap().get_stats_id() == pc.get_stats_id()
                        {
                            let mut anchor_track_forward_map =
                                self_arc.anchor_track_forward_map.write().await;
                            let mut anchor_track_forward_map_old =
                                self_arc.anchor_track_forward_map_old.write().await;
                            for (track, track_forward) in anchor_track_forward_map.iter() {
                                anchor_track_forward_map_old
                                    .insert(track.0.id(), track_forward.clone());
                            }
                            anchor_track_forward_map.clear();
                            *anchor = None;
                            println!("[{}] [anchor] set none", self_arc.get_id())
                        }
                    }
                    _ => {}
                }
            });
            Box::pin(async {})
        }));
        let self_arc = Arc::new(self.clone());
        let pc = peer.clone();
        peer.on_track(Box::new(move |track, _, _| {
            println!(
                "[{}] [anchor] [{}] [track-{}] kind : {}",
                self_arc.get_id(),
                pc.get_stats_id(),
                track.id(),
                track.kind().to_string()
            );
            let self_arc = self_arc.clone();
            let pc = pc.clone();
            tokio::spawn(async move {
                let anchor_up_track_result =
                    self_arc.anchor_track_up(pc.clone(), track.clone()).await;
                println!(
                    "[{}] [anchor] [{}] [track-{}] result : {:?}",
                    self_arc.get_id(),
                    pc.get_stats_id(),
                    track.id(),
                    anchor_up_track_result
                );
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
        println!("[{}] [anchor] set {}", self.get_id(), peer.get_stats_id());
        *anchor = Some(peer);
        Ok(description)
    }

    pub async fn add_subscribe(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<RTCSessionDescription> {
        let peer = PeerForward::new_peer().await?;
        let pc = peer.clone();
        let self_arc = Arc::new(self.clone());
        peer.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            let pc = pc.clone();
            let self_arc = self_arc.clone();
            let subscribe_group = self_arc.subscribe_group.clone();
            tokio::spawn(async move {
                println!(
                    "[{}] [subscribe] [{}] connection state changed: {}",
                    self_arc.get_id(),
                    pc.get_stats_id(),
                    s
                );
                match s {
                    RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                        let _ = pc.close().await;
                    }
                    RTCPeerConnectionState::Closed => {
                        println!(
                            "[{}] [subscribe] [{}] down",
                            self_arc.get_id(),
                            pc.get_stats_id()
                        );
                        let mut subscribe_peers = subscribe_group.write().await;
                        subscribe_peers.retain(|x| x != &PeerWrap(pc.clone()));
                        drop(subscribe_peers);
                        let _ = self_arc.refresh().await;
                    }
                    RTCPeerConnectionState::Connected => {
                        let mut subscribe_peers = subscribe_group.write().await;
                        subscribe_peers.push(PeerWrap(pc.clone()));
                        println!(
                            "[{}] [subscribe] [{}] up",
                            self_arc.get_id(),
                            pc.get_stats_id()
                        );
                        drop(subscribe_peers);
                        let _ = self_arc.refresh().await;
                    }
                    _ => {}
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

    async fn anchor_track_up(
        &self,
        peer: Arc<RTCPeerConnection>,
        track: Arc<TrackRemote>,
    ) -> Result<()> {
        if !PeerForward::track_kind_support(track.clone()) {
            return Err(anyhow::anyhow!("track kind not support"));
        }
        let anchor = self.anchor.read().await;
        if anchor.is_none() {
            return Err(anyhow::anyhow!("anchor is none"));
        }
        if anchor.as_ref().unwrap().get_stats_id() != peer.get_stats_id() {
            return Err(anyhow::anyhow!("anchor is not self"));
        }
        drop(anchor);
        if !self.kind_many {
            let anchor_track_forward = self.anchor_track_forward_map.read().await;
            for (track_wrap, _) in anchor_track_forward.iter() {
                if track_wrap.0.kind() == track.kind() {
                    return Err(anyhow::anyhow!("track kind exist"));
                }
            }
        }
        let mut anchor_track_forward = self.anchor_track_forward_map.write().await;
        let mut anchor_track_forward_map_old = self.anchor_track_forward_map_old.write().await;
        let track_forward = anchor_track_forward_map_old
            .remove(track.id().as_str())
            .map_or_else(
                || Arc::new(RwLock::default()),
                |track_forward| track_forward,
            );
        anchor_track_forward.insert(TrackRemoteWrap(track.clone()), track_forward);
        drop(anchor_track_forward);
        tokio::spawn(PeerForward::anchor_track_pli(peer.clone(), track.clone()));
        let self_arc = Arc::new(self.clone());
        let track_arc = track.clone();
        tokio::spawn(async move {
            self_arc.anchor_track_forward(track_arc).await;
        });
        let _ = self.refresh_track_forward(track.clone()).await;
        Ok(())
    }

    async fn refresh(&self) -> Result<()> {
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
            self.get_id(),
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

    async fn anchor_track_forward(&self, track: Arc<TrackRemote>) {
        let mut b = vec![0u8; 1500];
        println!(
            "[{}] [anchor] [track-{}] forward up",
            self.get_id(),
            track.id()
        );
        while let Ok((rtp_packet, _)) = track.read(&mut b).await {
            let anchor_track_forward_map = self.anchor_track_forward_map.read().await;
            let anchor_track_forward =
                anchor_track_forward_map.get(&TrackRemoteWrap(track.clone()));
            if anchor_track_forward.is_none() {
                break;
            }
            let anchor_track_forward = anchor_track_forward.unwrap().read().await;
            let senders: Vec<Sender<ForwardData>> = anchor_track_forward
                .iter()
                .map(|(_, sender)| sender.clone())
                .collect();
            drop(anchor_track_forward);
            let packet = Arc::new(rtp_packet);
            for sender in senders.iter() {
                let _ = sender.send(packet.clone()).await;
            }
        }
        let mut anchor_track_forward_map = self.anchor_track_forward_map.write().await;
        anchor_track_forward_map.remove(&TrackRemoteWrap(track.clone()));
        println!(
            "[{}] [anchor] [track-{}] forward down",
            self.get_id(),
            track.id()
        );
    }

    async fn peer_add_track(
        &self,
        peer: Arc<RTCPeerConnection>,
        kind: &str,
    ) -> Result<Sender<ForwardData>> {
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
        let (send, mut recv) = channel::<ForwardData>(32);
        let self_id = self.get_id();
        tokio::spawn(async move {
            println!(
                "[{}] [subscribe] [{}] forward up",
                self_id,
                peer.get_stats_id()
            );
            while let Some(data) = recv.recv().await {
                if let Err(err) = track.write_rtp(&data).await {
                    println!("video_track.write err: {}", err);
                }
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

    async fn anchor_track_pli(peer: Arc<RTCPeerConnection>, track: Arc<TrackRemote>) {
        let pc = Arc::downgrade(&peer);
        // Send a PLI on an interval so that the publisher is pushing a keyframe every rtcpPLIInterval
        let media_ssrc = track.ssrc();
        let mut result = Result::<usize>::Ok(0);
        while result.is_ok() {
            let timeout = tokio::time::sleep(Duration::from_secs(1));
            tokio::pin!(timeout);
            tokio::select! {
                _ = timeout.as_mut() =>{
                    if let Some(pc) = pc.upgrade(){
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

    fn track_kind_support(track: Arc<TrackRemote>) -> bool {
        let kind = track.kind().to_string();
        let kind = kind.as_str();
        return kind == VIDEO_KIND || kind == AUDIO_KIND;
    }
}
