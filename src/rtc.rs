use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc::{channel, Sender};
use tokio::sync::RwLock;
use webrtc::api::APIBuilder;
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_OPUS, MIME_TYPE_VP8};
use webrtc::ice_transport::ice_connection_state::RTCIceConnectionState;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication;
use webrtc::rtp::packet::Packet;
use webrtc::rtp_transceiver::PayloadType;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType,
};
use webrtc::track::track_local::{TrackLocal, TrackLocalWriter};
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_remote::TrackRemote;

pub const VIDEO_KIND: &str = "video";
pub const AUDIO_KIND: &str = "audio";
pub const VIDEO_PAYLOAD_TYPE: PayloadType = 96;
pub const AUDIO_PAYLOAD_TYPE: PayloadType = 111;

type ForwardData = Arc<Packet>;

struct Peer(Arc<RTCPeerConnection>);

struct Anchor(Arc<RTCPeerConnection>, bool, bool);

struct Subscribe(Option<Sender<ForwardData>>, Option<Sender<ForwardData>>);

impl From<Arc<RTCPeerConnection>> for Peer {
    fn from(value: Arc<RTCPeerConnection>) -> Self {
        Peer(value)
    }
}

impl Clone for Peer {
    fn clone(&self) -> Self {
        Peer(self.0.clone())
    }
}

impl Eq for Peer {}

impl PartialEq for Peer {
    fn eq(&self, other: &Self) -> bool {
        self.0.get_stats_id() == other.0.get_stats_id()
    }

    fn ne(&self, other: &Self) -> bool {
        self.0.get_stats_id() != other.0.get_stats_id()
    }
}

impl Hash for Peer {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.get_stats_id().hash(state);
    }
}

#[derive(Default)]
pub struct PeerForward {
    anchor: Arc<RwLock<Option<Anchor>>>,
    subscription_map: Arc<RwLock<HashMap<Peer, Subscribe>>>,
}

impl Clone for PeerForward {
    fn clone(&self) -> Self {
        PeerForward {
            anchor: self.anchor.clone(),
            subscription_map: self.subscription_map.clone(),
        }
    }
}

impl PeerForward {
    pub async fn set_anchor(&self, offer: RTCSessionDescription) -> Result<RTCSessionDescription> {
        let mut anchor = self.anchor.write().await;
        if anchor.is_some() {
            return Err(anyhow::anyhow!("anchor is set"));
        }
        let peer = PeerForward::new_peer().await?;
        peer.on_ice_connection_state_change(Box::new(
            move |connection_state: RTCIceConnectionState| {
                println!("Connection State has changed {connection_state}");
                Box::pin(async {})
            },
        ));
        let self_arc = Arc::new(self.clone());
        let pc = peer.clone();
        peer.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            let self_arc = self_arc.clone();
            let pc = pc.clone();
            tokio::spawn(async move {
                println!("Peer Connection State has changed: {s}");
                if s == RTCPeerConnectionState::Failed
                    || s == RTCPeerConnectionState::Closed
                    || s == RTCPeerConnectionState::Disconnected
                {
                    let _ = pc.close().await;
                    let anchor = self_arc.anchor.clone();
                    let mut anchor = anchor.write().await;
                    *anchor = None;
                };
            });
            Box::pin(async {})
        }));
        let self_arc = Arc::new(self.clone());
        let pc = peer.clone();
        peer.on_track(Box::new(move |track, _, _| {
            let self_arc = self_arc.clone();
            let pc = pc.clone();
            tokio::spawn(async move {
                let _ = self_arc.anchor_up_track(pc, track).await;
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
            .ok_or(anyhow::anyhow!("Failed to get local description"))?;
        *anchor = Some(Anchor(peer, false, false));
        Ok(description)
    }

    pub async fn add_subscribe(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<RTCSessionDescription> {
        let peer = PeerForward::new_peer().await?;
        peer.on_ice_connection_state_change(Box::new(
            move |connection_state: RTCIceConnectionState| {
                println!("Connection State has changed {connection_state}");
                Box::pin(async {})
            },
        ));
        let pc = peer.clone();
        let self_arc = Arc::new(self.clone());
        peer.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            let pc = pc.clone();
            let self_arc = self_arc.clone();
            let subscription_map = self_arc.subscription_map.clone();
            Box::pin(async move {
                println!("Peer Connection State has changed: {s}");
                match s {
                    RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed | RTCPeerConnectionState::Disconnected => {
                        let _ = pc.close().await;
                        let mut subscribe_peers = subscription_map.write().await;
                        subscribe_peers.remove(&pc.into());
                    }
                    RTCPeerConnectionState::Connected => {
                        let mut subscribe_peers = subscription_map.write().await;
                        subscribe_peers.insert(pc.into(), Subscribe(None, None));
                        drop(subscribe_peers);
                        let _ = self_arc.subscribe_refresh(VIDEO_KIND).await;
                        let _ = self_arc.subscribe_refresh(AUDIO_KIND).await;
                    }
                    _ => {}
                };
            })
        }));
        let _ = peer.set_remote_description(offer).await?;
        let answer = peer.create_answer(None).await?;
        let mut gather_complete = peer.gathering_complete_promise().await;
        let _ = peer.set_local_description(answer).await?;
        let _ = gather_complete.recv().await;
        let description = peer
            .local_description()
            .await
            .ok_or(anyhow::anyhow!("Failed to get local description"))?;
        Ok(description)
    }

    async fn anchor_up_track(
        &self,
        peer: Arc<RTCPeerConnection>,
        track: Arc<TrackRemote>,
    ) -> Result<()> {
        let kind = track.kind().to_string();
        let kind = kind.as_str();
        let mut anchor = self.anchor.write().await;
        if anchor.is_none() {
            return Err(anyhow::anyhow!("anchor is none"));
        }
        println!("anchor_up_track : {kind}");
        let anchor_set = anchor.as_mut().unwrap();
        match kind {
            VIDEO_KIND => {
                if anchor_set.1 {
                    return Err(anyhow::anyhow!("video already online"));
                }
                anchor_set.1 = true;
            }
            AUDIO_KIND => {
                if anchor_set.2 {
                    return Err(anyhow::anyhow!("audio already online"));
                }
                anchor_set.2 = true;
            }
            _ => return Err(anyhow::anyhow!("kind error")),
        };
        drop(anchor);
        tokio::spawn(PeerForward::publish_track_remote_pli(
            peer.clone(),
            track.clone(),
        ));
        let self_arc = Arc::new(self.clone());
        tokio::spawn(async move {
            self_arc.publish_track_remote(track.clone()).await;
        });
        let _ = self.subscribe_refresh(kind).await;
        Ok(())
    }

    async fn subscribe_refresh(&self, kind: &str) -> Result<()> {
        let anchor = self.anchor.read().await;
        if anchor.is_none() {
            return Ok(());
        }
        let anchor_up = anchor.as_ref().unwrap();
        if (kind == VIDEO_KIND && !anchor_up.1) || (kind == AUDIO_KIND && !anchor_up.2) {
            return Ok(());
        }
        drop(anchor);
        println!("subscribe refresh {kind}");
        let mut peers = self.subscription_map.write().await;
        let subscribe_refresh_peers: Vec<Peer> = peers
            .iter()
            .filter(|(_, subscription)| {
                match kind {
                    VIDEO_KIND => subscription.0.is_none(),
                    AUDIO_KIND => subscription.1.is_none(),
                    _ => false,
                }
            })
            .map(|(p, _)| p.clone())
            .collect();
        for peer in subscribe_refresh_peers {
            match PeerForward::peer_add_track(peer.0.clone(), kind).await {
                Ok(sender) => {
                    let subscription = peers.get_mut(&peer).unwrap();
                    match kind {
                        VIDEO_KIND => subscription.0 = Some(sender),
                        AUDIO_KIND => subscription.1 = Some(sender),
                        _ => {}
                    }
                }
                Err(err) => {
                    println!("peer_add_track err: {} peer id : {}", err, peer.0.get_stats_id());
                }
            }
        }
        Ok(())
    }


    async fn publish_track_remote(&self, track: Arc<TrackRemote>) {
        let kind = track.kind().to_string();
        let kind = kind.as_str();
        let mut b = vec![0u8; 1500];
        while let Ok((rtp_packet, _)) = track.read(&mut b).await {
            let subscription_map = self.subscription_map.read().await;
            let senders: Vec<Sender<ForwardData>> = subscription_map
                .iter()
                .map(|(_, subscription)| {
                    match kind {
                        VIDEO_KIND => subscription.0.clone(),
                        AUDIO_KIND => subscription.1.clone(),
                        _ => None,
                    }
                })
                .filter(|subscription| subscription.is_some())
                .map(|subscription| subscription.unwrap())
                .collect();
            drop(subscription_map);
            let packet = Arc::new(rtp_packet);
            for sender in senders.iter() {
                let _ = sender.send(packet.clone()).await;
            }
        }
    }

    async fn publish_track_remote_pli(peer: Arc<RTCPeerConnection>, track: Arc<TrackRemote>) {
        let pc = Arc::downgrade(&peer);
        // Send a PLI on an interval so that the publisher is pushing a keyframe every rtcpPLIInterval
        let media_ssrc = track.ssrc();
        let pc2 = pc.clone();
        tokio::spawn(async move {
            let mut result = Result::<usize>::Ok(0);
            while result.is_ok() {
                let timeout = tokio::time::sleep(Duration::from_secs(1));
                tokio::pin!(timeout);
                tokio::select! {
                    _ = timeout.as_mut() =>{
                        if let Some(pc) = pc2.upgrade(){
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
        });
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

    async fn peer_add_track(
        peer: Arc<RTCPeerConnection>,
        kind: &str,
    ) -> Result<Sender<ForwardData>> {
        let (mime_type, id, stream_id) = match kind {
            VIDEO_KIND => (
                MIME_TYPE_VP8.to_owned(),
                VIDEO_KIND.to_owned(),
                "webrtc-rs-video".to_owned(),
            ),
            AUDIO_KIND => (
                MIME_TYPE_OPUS.to_owned(),
                AUDIO_KIND.to_owned(),
                "webrtc-rs-audio".to_owned(),
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
        let _ = peer
            .add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;
        let (send, mut recv) = channel::<ForwardData>(32);
        tokio::spawn(async move {
            while let Some(data) = recv.recv().await {
                if let Err(err) = track.write_rtp(&data).await {
                    println!("video_track.write err: {}", err);
                }
            }
        });
        Ok(send)
    }
}
