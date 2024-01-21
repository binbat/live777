use std::sync::Arc;

use anyhow::Result;
use log::{debug, info, warn};
use tokio::sync::{broadcast, RwLock};
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
use webrtc::sdp::MediaDescription;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_remote::TrackRemote;

use crate::forward::get_peer_id;
use crate::forward::info::Layer;
use crate::AppError;
use crate::{media, metrics};

use super::publish::PublishRTCPeerConnection;
use super::subscribe::SubscribeRTCPeerConnection;
use super::track::PublishTrackRemote;
use super::track_match;
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

pub(crate) struct PeerForwardInternal {
    pub(crate) id: String,
    publish: RwLock<Option<PublishRTCPeerConnection>>,
    publish_tracks: RwLock<Vec<PublishTrackRemote>>,
    subscribe_group: RwLock<Vec<SubscribeRTCPeerConnection>>,
    data_channel_forward: RwLock<Option<DataChannelForward>>,
    ice_server: Vec<RTCIceServer>,
}

impl PeerForwardInternal {
    pub(crate) fn new(id: impl ToString, ice_server: Vec<RTCIceServer>) -> Self {
        PeerForwardInternal {
            id: id.to_string(),
            publish: RwLock::new(None),
            publish_tracks: RwLock::new(Vec::new()),
            subscribe_group: RwLock::new(Vec::new()),
            data_channel_forward: RwLock::new(None),
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
            publish.as_ref().unwrap().peer.close().await?;
            return Ok(true);
        }
        drop(publish);
        let subscribe_group = self.subscribe_group.read().await;
        for subscribe in subscribe_group.iter() {
            if subscribe.id == id {
                subscribe.peer.close().await?;
                return Ok(true);
            }
        }
        Ok(false)
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
        let publish_tracks = self.publish_tracks.read().await;
        publish.is_some()
            && publish.as_ref().unwrap().peer.connection_state()
                == RTCPeerConnectionState::Connected
            && publish_tracks.len()
                == media::count_sends(
                    &publish
                        .as_ref()
                        .unwrap()
                        .peer
                        .remote_description()
                        .await
                        .unwrap()
                        .unmarshal()
                        .unwrap()
                        .media_descriptions,
                )
    }

    pub(crate) async fn set_publish(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut publish = self.publish.write().await;
        if publish.is_some() {
            return Err(AppError::ResourceAlreadyExists(
                "A connection has already been established".to_string(),
            )
            .into());
        }
        let mut data_channel_forward = self.data_channel_forward.write().await;
        let data_channel_forward_publish = broadcast::channel(1024);
        let data_channel_forward_subscribe = broadcast::channel(1024);
        *data_channel_forward = Some(DataChannelForward {
            publish: (
                data_channel_forward_publish.0,
                Arc::new(data_channel_forward_publish.1),
            ),
            subscribe: (
                data_channel_forward_subscribe.0,
                Arc::new(data_channel_forward_subscribe.1),
            ),
        });
        let publish_peer = PublishRTCPeerConnection::new(self.id.clone(), peer.clone()).await;
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
        let mut subscribe_group = self.subscribe_group.write().await;
        for peer_wrap in subscribe_group.iter() {
            let _ = peer_wrap.peer.close().await;
        }
        subscribe_group.clear();
        let mut data_channel_forward = self.data_channel_forward.write().await;
        *data_channel_forward = None;
        *publish = None;
        info!("[{}] [publish] set none", self.id);
        metrics::PUBLISH.dec();
        Ok(())
    }

    pub async fn publish_is_svc(&self) -> bool {
        self.publish_track_remotes(RTPCodecType::Video).await.len() > 1
    }

    async fn publish_track_remotes(&self, code_type: RTPCodecType) -> Vec<PublishTrackRemote> {
        let publish_tracks = self.publish_tracks.read().await;
        let mut video_track_remotes = vec![];
        for publish_track in publish_tracks.iter() {
            if publish_track.kind == code_type {
                video_track_remotes.push(publish_track.clone());
            }
        }
        video_track_remotes
    }

    pub async fn publish_svc_rids(&self) -> Result<Vec<String>> {
        let publish = self.publish.read().await;
        match publish.as_ref() {
            Some(publish) => {
                let rd = publish.peer.remote_description().await;
                if rd.is_none() {
                    return Err(anyhow::anyhow!("publish svc rids error"));
                }
                let mds = rd.unwrap().unmarshal()?.media_descriptions;
                for md in mds {
                    if RTPCodecType::from(md.media_name.media.as_str()) == RTPCodecType::Video {
                        return Ok(media::rids(&md));
                    }
                }
                Err(anyhow::anyhow!("publish svc rids error"))
            }
            None => Err(anyhow::anyhow!("publish svc rids error")),
        }
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
        Ok(())
    }

    pub(crate) async fn publish_data_channel(
        &self,
        _peer: Arc<RTCPeerConnection>,
        dc: Arc<RTCDataChannel>,
    ) -> Result<()> {
        let data_channel_forward = self.data_channel_forward.read().await;
        if data_channel_forward.is_none() {
            warn!("data channel forward is none");
            return Err(anyhow::anyhow!("data channel forward is none"));
        }
        let data_channel_forward = data_channel_forward.as_ref().unwrap();
        let sender = data_channel_forward.subscribe.0.clone();
        let receiver = data_channel_forward.publish.0.subscribe();
        Self::data_channel_forward(dc, sender, receiver).await;
        Ok(())
    }
}

// subscribe
impl PeerForwardInternal {
    pub(crate) async fn new_subscription_peer(
        &self,
        media_descriptions: Vec<MediaDescription>,
    ) -> Result<Arc<RTCPeerConnection>> {
        if !self.publish_is_some().await {
            return Err(anyhow::anyhow!("publish is none"));
        }
        let publish_rtcp_sender = self
            .publish
            .read()
            .await
            .as_ref()
            .unwrap()
            .rtcp_sender
            .clone();
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
        let mut video_track = None;
        let mut audio_track = None;
        let tracks = self
            .publish_tracks
            .read()
            .await
            .iter()
            .map(|t| t.track.clone())
            .collect::<Vec<_>>();
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
                let track = Arc::new(TrackLocalStaticRTP::new(
                    track.codec().capability,
                    track.kind().to_string(),
                    "webrtc-rs".to_owned(),
                ));
                if track.kind() == RTPCodecType::Video {
                    video_track = Some(track);
                } else {
                    audio_track = Some(track);
                }
            }
        }
        let subscribe_tracks = self
            .publish_tracks
            .read()
            .await
            .iter()
            .map(|t| t.subscribe())
            .collect::<Vec<_>>();
        let s = SubscribeRTCPeerConnection::new(
            self.id.clone(),
            peer.clone(),
            publish_rtcp_sender,
            subscribe_tracks,
            video_track,
            audio_track,
        )
        .await?;
        self.subscribe_group.write().await.push(s);
        Ok(peer)
    }

    pub async fn remove_subscribe(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut subscribe_peers = self.subscribe_group.write().await;
        subscribe_peers.retain(|p| p.id != get_peer_id(&peer));
        Ok(())
    }

    pub async fn select_layer(&self, id: String, layer: Option<Layer>) -> Result<()> {
        let rid = if let Some(layer) = layer {
            layer.encoding_id
        } else {
            self.publish_svc_rids().await?[0].clone()
        };
        let subscribe_group = self.subscribe_group.read().await;
        for subscribe in subscribe_group.iter() {
            if subscribe.id == id {
                subscribe.select_layer(rid)?;
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
        let data_channel_forward = self.data_channel_forward.read().await;
        if data_channel_forward.is_none() {
            warn!("data channel forward is none");
            return Err(anyhow::anyhow!("data channel forward is none"));
        }
        let data_channel_forward = data_channel_forward.as_ref().unwrap();
        let sender = data_channel_forward.publish.0.clone();
        let receiver = data_channel_forward.subscribe.0.subscribe();
        Self::data_channel_forward(dc, sender, receiver).await;
        Ok(())
    }
}
