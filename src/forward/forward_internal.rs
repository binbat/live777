use std::borrow::ToOwned;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::data::data_channel::DataChannel;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::{RTCRtpHeaderExtensionCapability, RTPCodecType};
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;
use webrtc::sdp::extmap::{SDES_MID_URI, SDES_RTP_STREAM_ID_URI};

use webrtc::track::track_remote::TrackRemote;

use crate::forward::get_peer_id;
use crate::forward::rtcp::RtcpMessage;
use crate::metrics;
use crate::AppError;

use super::media::MediaInfo;
use super::publish::PublishRTCPeerConnection;
use super::subscribe::SubscribeRTCPeerConnection;
use super::track::PublishTrackRemote;

const MESSAGE_SIZE: usize = 1024 * 16;

#[derive(Clone)]
struct DataChannelForward {
    publish: (
        broadcast::Sender<Vec<u8>>,
        Arc<broadcast::Receiver<Vec<u8>>>,
    ),
    subscribe: (
        broadcast::Sender<Vec<u8>>,
        Arc<broadcast::Receiver<Vec<u8>>>,
    ),
}

type PublishRtcpChannel = (
    broadcast::Sender<(RtcpMessage, u32)>,
    broadcast::Receiver<(RtcpMessage, u32)>,
);

pub(crate) struct PeerForwardInternal {
    pub(crate) id: String,
    publish: RwLock<Option<PublishRTCPeerConnection>>,
    publish_tracks: Arc<RwLock<Vec<PublishTrackRemote>>>,
    publish_tracks_change: (broadcast::Sender<()>, broadcast::Receiver<()>),
    publish_rtcp_channel: PublishRtcpChannel,
    subscribe_group: RwLock<Vec<SubscribeRTCPeerConnection>>,
    data_channel_forward: DataChannelForward,
    ice_server: Vec<RTCIceServer>,
}

impl PeerForwardInternal {
    pub(crate) fn new(id: impl ToString, ice_server: Vec<RTCIceServer>) -> Self {
        let publish_tracks_change = broadcast::channel(1024);
        let data_channel_forward_publish = broadcast::channel(1024);
        let data_channel_forward_subscribe = broadcast::channel(1024);
        let data_channel_forward = DataChannelForward {
            publish: (
                data_channel_forward_publish.0,
                Arc::new(data_channel_forward_publish.1),
            ),
            subscribe: (
                data_channel_forward_subscribe.0,
                Arc::new(data_channel_forward_subscribe.1),
            ),
        };
        PeerForwardInternal {
            id: id.to_string(),
            publish: RwLock::new(None),
            publish_tracks: Arc::new(RwLock::new(Vec::new())),
            publish_tracks_change,
            publish_rtcp_channel: broadcast::channel(48),
            subscribe_group: RwLock::new(Vec::new()),
            data_channel_forward,
            ice_server,
        }
    }

    pub(crate) async fn add_ice_candidate(
        &self,
        id: String,
        ice_candidates: Vec<RTCIceCandidateInit>,
    ) -> Result<()> {
        let publish = self.publish.read().await;
        if publish.is_some() && publish.as_ref().unwrap().id == id {
            let publish = publish.as_ref().unwrap();
            for ice_candidate in ice_candidates {
                publish.peer.add_ice_candidate(ice_candidate).await?;
            }
            return Ok(());
        }
        drop(publish);
        let subscribe_group = self.subscribe_group.read().await;
        for subscribe in subscribe_group.iter() {
            if subscribe.id == id {
                for ice_candidate in ice_candidates {
                    subscribe.peer.add_ice_candidate(ice_candidate).await?;
                }
                return Ok(());
            }
        }
        Ok(())
    }

    pub(crate) async fn remove_peer(&self, id: String) -> Result<bool> {
        let publish = self.publish.read().await;
        if publish.is_some() && publish.as_ref().unwrap().id == id {
            drop(publish);
            self.close().await?;
            return Ok(true);
        }

        let subscribe_group = self.subscribe_group.read().await;
        for subscribe in subscribe_group.iter() {
            if subscribe.id == id {
                subscribe.peer.close().await?;
                break;
            }
        }
        Ok(false)
    }

    pub(crate) async fn close(&self) -> Result<()> {
        let publish = self.publish.read().await;
        let subscribe_group = self.subscribe_group.read().await;
        let mut publish_tracks = self.publish_tracks.write().await;
        if publish.is_some() {
            publish.as_ref().unwrap().peer.close().await?;
        }
        for subscribe in subscribe_group.iter() {
            subscribe.peer.close().await?;
        }
        publish_tracks.clear();
        let _ = self.publish_tracks_change.0.send(());
        info!("{} close", self.id);
        Ok(())
    }

    async fn data_channel_forward(
        dc: Arc<RTCDataChannel>,
        sender: broadcast::Sender<Vec<u8>>,
        receiver: broadcast::Receiver<Vec<u8>>,
    ) {
        let dc2 = dc.clone();
        dc.on_open(Box::new(move || {
            tokio::spawn(async move {
                let raw = match dc2.detach().await {
                    Ok(raw) => raw,
                    Err(err) => {
                        debug!("detach err: {}", err);
                        return;
                    }
                };
                let r = Arc::clone(&raw);
                tokio::spawn(Self::data_channel_read_loop(r, sender));
                tokio::spawn(Self::data_channel_write_loop(raw, receiver));
            });

            Box::pin(async {})
        }));
    }

    async fn data_channel_read_loop(d: Arc<DataChannel>, sender: broadcast::Sender<Vec<u8>>) {
        let mut buffer = vec![0u8; MESSAGE_SIZE];
        loop {
            let n = match d.read(&mut buffer).await {
                Ok(n) => n,
                Err(err) => {
                    info!("Datachannel closed; Exit the read_loop: {err}");
                    return;
                }
            };
            if n == 0 {
                break;
            }
            if let Err(err) = sender.send(buffer[..n].to_vec()) {
                info!("send data channel err: {}", err);
                return;
            };
        }
    }

    async fn data_channel_write_loop(
        d: Arc<DataChannel>,
        mut receiver: broadcast::Receiver<Vec<u8>>,
    ) {
        while let Ok(msg) = receiver.recv().await {
            if let Err(err) = d.write(&msg.into()).await {
                info!("write data channel err: {}", err);
                return;
            };
        }
    }
}

// publish
impl PeerForwardInternal {
    pub(crate) async fn publish_is_some(&self) -> bool {
        let publish = self.publish.read().await;
        publish.is_some()
    }

    pub(crate) async fn publish_is_ok(&self) -> bool {
        let publish = self.publish.read().await;
        publish.is_some()
            && publish.as_ref().unwrap().peer.connection_state()
                == RTCPeerConnectionState::Connected
    }

    pub(crate) async fn set_publish(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut publish = self.publish.write().await;
        if publish.is_some() {
            return Err(AppError::ResourceAlreadyExists(
                "A connection has already been established".to_string(),
            )
            .into());
        }
        let publish_peer = PublishRTCPeerConnection::new(
            self.id.clone(),
            peer.clone(),
            self.publish_rtcp_channel.0.subscribe(),
        )
        .await?;
        info!("[{}] [publish] set {}", self.id, publish_peer.id);
        *publish = Some(publish_peer);
        metrics::PUBLISH.inc();
        Ok(())
    }

    pub(crate) async fn remove_publish(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut publish = self.publish.write().await;
        if publish.is_none() {
            return Ok(());
        }
        if publish.as_ref().unwrap().id != get_peer_id(&peer) {
            return Err(anyhow::anyhow!("publish not myself"));
        }
        let mut publish_tracks = self.publish_tracks.write().await;
        publish_tracks.clear();
        let _ = self.publish_tracks_change.0.send(());
        *publish = None;
        info!("[{}] [publish] set none", self.id);
        metrics::PUBLISH.dec();
        Ok(())
    }

    pub async fn publish_is_svc(&self) -> bool {
        let publish = self.publish.read().await;
        if publish.is_none() {
            return false;
        }
        publish.as_ref().unwrap().media_info.video_transceiver.2
    }

    pub async fn publish_svc_rids(&self) -> Result<Vec<String>> {
        let publish_tracks = self.publish_tracks.read().await;
        let rids = publish_tracks
            .iter()
            .filter(|t| t.kind == RTPCodecType::Video)
            .map(|t| t.rid.clone())
            .collect::<Vec<_>>();
        Ok(rids)
    }

    pub(crate) async fn new_publish_peer(
        &self,
        media_info: MediaInfo,
    ) -> Result<Arc<RTCPeerConnection>> {
        if media_info.video_transceiver.0 > 1 && media_info.audio_transceiver.0 > 1 {
            return Err(anyhow::anyhow!("sendonly is more than 1"));
        }
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
        let mut s = SettingEngine::default();
        s.detach_data_channels();
        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .with_setting_engine(s)
            .build();
        let config = RTCConfiguration {
            ice_servers: self.ice_server.clone(),
            ..Default::default()
        };
        let peer = Arc::new(api.new_peer_connection(config).await?);
        let mut transceiver_kinds = vec![];
        if media_info.video_transceiver.0 > 0 {
            transceiver_kinds.push(RTPCodecType::Video);
        }
        if media_info.audio_transceiver.0 > 0 {
            transceiver_kinds.push(RTPCodecType::Audio);
        }
        for kind in transceiver_kinds {
            let _ = peer
                .add_transceiver_from_kind(
                    kind,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Recvonly,
                        send_encodings: Vec::new(),
                    }),
                )
                .await?;
        }
        Ok(peer)
    }

    pub(crate) async fn publish_track_up(
        &self,
        peer: Arc<RTCPeerConnection>,
        track: Arc<TrackRemote>,
    ) -> Result<()> {
        let publish_track_remote =
            PublishTrackRemote::new(self.id.clone(), get_peer_id(&peer), track).await;
        let mut publish_tracks = self.publish_tracks.write().await;
        publish_tracks.push(publish_track_remote);
        publish_tracks.sort_by(|a, b| a.rid.cmp(&b.rid));
        let _ = self.publish_tracks_change.0.send(());
        Ok(())
    }

    pub(crate) async fn publish_data_channel(
        &self,
        _peer: Arc<RTCPeerConnection>,
        dc: Arc<RTCDataChannel>,
    ) -> Result<()> {
        let sender = self.data_channel_forward.subscribe.0.clone();
        let receiver = self.data_channel_forward.publish.0.subscribe();
        Self::data_channel_forward(dc, sender, receiver).await;
        Ok(())
    }
}

// subscribe
impl PeerForwardInternal {
    pub(crate) async fn new_subscription_peer(
        &self,
        media_info: MediaInfo,
    ) -> Result<Arc<RTCPeerConnection>> {
        if !self.publish_is_some().await {
            return Err(anyhow::anyhow!("publish is none"));
        }
        if media_info.video_transceiver.1 > 1 && media_info.audio_transceiver.1 > 1 {
            return Err(anyhow::anyhow!("sendonly is more than 1"));
        }
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let mut s = SettingEngine::default();
        s.detach_data_channels();
        let api = APIBuilder::new()
            .with_media_engine(m)
            .with_interceptor_registry(registry)
            .with_setting_engine(s)
            .build();
        let config = RTCConfiguration {
            ice_servers: self.ice_server.clone(),
            ..Default::default()
        };
        let peer = Arc::new(api.new_peer_connection(config).await?);
        let video_sender = match media_info.video_transceiver.1 {
            0 => None,
            _ => Some(
                peer.add_transceiver_from_kind(
                    RTPCodecType::Video,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Sendonly,
                        send_encodings: Vec::new(),
                    }),
                )
                .await?
                .sender()
                .await,
            ),
        };
        let audio_sender = match media_info.audio_transceiver.1 {
            0 => None,
            _ => Some(
                peer.add_transceiver_from_kind(
                    RTPCodecType::Audio,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Sendonly,
                        send_encodings: Vec::new(),
                    }),
                )
                .await?
                .sender()
                .await,
            ),
        };
        let s = SubscribeRTCPeerConnection::new(
            self.id.clone(),
            peer.clone(),
            self.publish_rtcp_channel.0.clone(),
            self.publish_tracks.clone(),
            self.publish_tracks_change.0.clone(),
            video_sender,
            audio_sender,
        )
        .await;
        self.subscribe_group.write().await.push(s);
        Ok(peer)
    }

    pub async fn remove_subscribe(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut subscribe_peers = self.subscribe_group.write().await;
        subscribe_peers.retain(|p| p.id != get_peer_id(&peer));
        Ok(())
    }

    pub async fn select_kind_rid(&self, id: String, kind: RTPCodecType, rid: String) -> Result<()> {
        let subscribe_group = self.subscribe_group.read().await;
        for subscribe in subscribe_group.iter() {
            if subscribe.id == id {
                subscribe.select_kind_rid(kind, rid)?;
                break;
            }
        }
        Ok(())
    }

    pub(crate) async fn subscribe_data_channel(
        &self,
        _peer: Arc<RTCPeerConnection>,
        dc: Arc<RTCDataChannel>,
    ) -> Result<()> {
        let sender = self.data_channel_forward.publish.0.clone();
        let receiver = self.data_channel_forward.subscribe.0.subscribe();
        Self::data_channel_forward(dc, sender, receiver).await;
        Ok(())
    }
}
