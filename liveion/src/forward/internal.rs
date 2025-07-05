use std::borrow::ToOwned;
use std::sync::Arc;

use chrono::Utc;
use libwish::Client;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_OPUS, MIME_TYPE_VP8};
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::APIBuilder;
use webrtc::data::data_channel::DataChannel;
use webrtc::data_channel::RTCDataChannel;
use webrtc::ice::mdns::MulticastDnsMode;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::{
    RTCRtpCodecCapability, RTCRtpHeaderExtensionCapability, RTPCodecType,
};
use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::{RTCPFeedback, RTCRtpTransceiverInit};
use webrtc::sdp::extmap::{SDES_MID_URI, SDES_RTP_STREAM_ID_URI};
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_remote::TrackRemote;

use crate::forward::get_peer_id;
use crate::forward::message::ForwardInfo;
use crate::forward::rtcp::RtcpMessage;
use crate::result::Result;
use crate::AppError;
use crate::{metrics, new_broadcast_channel};

use super::media::MediaInfo;
use super::message::{CascadeInfo, ForwardEvent, ForwardEventType};
use super::publish::PublishRTCPeerConnection;
use super::subscribe::SubscribeRTCPeerConnection;
use super::track::PublishTrackRemote;

const MESSAGE_SIZE: usize = 1024 * 16;

#[derive(Clone)]
struct DataChannelForward {
    publish: broadcast::Sender<Vec<u8>>,
    subscribe: broadcast::Sender<Vec<u8>>,
}

pub(crate) struct PeerForwardInternal {
    pub(crate) stream: String,
    create_at: i64,
    publish_leave_at: RwLock<i64>,
    subscribe_leave_at: RwLock<i64>,
    publish: RwLock<Option<PublishRTCPeerConnection>>,
    pub(super) publish_tracks: Arc<RwLock<Vec<PublishTrackRemote>>>,
    publish_tracks_change: broadcast::Sender<()>,
    publish_rtcp_channel: broadcast::Sender<(RtcpMessage, u32)>,
    subscribe_group: RwLock<Vec<SubscribeRTCPeerConnection>>,
    data_channel_forward: DataChannelForward,
    ice_server: Vec<RTCIceServer>,
    event_sender: broadcast::Sender<ForwardEvent>,
}

impl PeerForwardInternal {
    pub(crate) fn new(stream: impl ToString, ice_server: Vec<RTCIceServer>) -> Self {
        PeerForwardInternal {
            stream: stream.to_string(),
            create_at: Utc::now().timestamp_millis(),
            publish_leave_at: RwLock::new(0),
            subscribe_leave_at: RwLock::new(Utc::now().timestamp_millis()),
            publish: RwLock::new(None),
            publish_tracks: Arc::new(RwLock::new(Vec::new())),
            publish_tracks_change: new_broadcast_channel!(16),
            publish_rtcp_channel: new_broadcast_channel!(48),
            subscribe_group: RwLock::new(Vec::new()),
            data_channel_forward: DataChannelForward {
                publish: new_broadcast_channel!(1024),
                subscribe: new_broadcast_channel!(1024),
            },
            ice_server,
            event_sender: new_broadcast_channel!(16),
        }
    }

    pub(crate) fn subscribe_event(&self) -> broadcast::Receiver<ForwardEvent> {
        self.event_sender.subscribe()
    }

    pub(crate) async fn info(&self) -> ForwardInfo {
        let mut subscribe_session_infos = vec![];
        let subscribe_group = self.subscribe_group.read().await;
        for subscribe in subscribe_group.iter() {
            subscribe_session_infos.push(subscribe.info().await);
        }
        ForwardInfo {
            id: self.stream.clone(),
            create_at: self.create_at,
            publish_leave_at: *self.publish_leave_at.read().await,
            subscribe_leave_at: *self.subscribe_leave_at.read().await,
            publish_session_info: self
                .publish
                .read()
                .await
                .as_ref()
                .map(|publish| publish.info()),
            subscribe_session_infos,
            codecs: self
                .publish_tracks
                .read()
                .await
                .iter()
                .map(|track| track.codec())
                .collect(),
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
        if publish.is_some() {
            publish.as_ref().unwrap().peer.close().await?;
        }
        for subscribe in subscribe_group.iter() {
            subscribe.peer.close().await?;
        }
        info!("{} close", self.stream);
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

    pub(crate) async fn set_publish(
        &self,
        peer: Arc<RTCPeerConnection>,
        cascade: Option<CascadeInfo>,
    ) -> Result<()> {
        {
            let mut publish = self.publish.write().await;
            if publish.is_some() {
                return Err(AppError::stream_already_exists(
                    "A connection has already been established",
                ));
            }
            let publish_peer = PublishRTCPeerConnection::new(
                self.stream.clone(),
                peer.clone(),
                self.publish_rtcp_channel.subscribe(),
                cascade,
            )
            .await?;
            info!("[{}] [publish] set {}", self.stream, publish_peer.id);
            *publish = Some(publish_peer);
        }
        {
            let mut publish_leave_at = self.publish_leave_at.write().await;
            *publish_leave_at = 0;
        }
        metrics::PUBLISH.inc();
        self.send_event(ForwardEventType::PublishUp, get_peer_id(&peer))
            .await;
        Ok(())
    }

    pub(crate) async fn remove_publish(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        {
            let mut publish = self.publish.write().await;
            if publish.is_none() {
                return Err(AppError::throw("publish is none"));
            }
            if publish.as_ref().unwrap().id != get_peer_id(&peer) {
                return Err(AppError::throw("publish not myself"));
            }
            *publish = None;
        }
        {
            let mut publish_tracks = self.publish_tracks.write().await;
            publish_tracks.clear();
            let _ = self.publish_tracks_change.send(());
        }
        {
            let mut publish_leave_at = self.publish_leave_at.write().await;
            *publish_leave_at = Utc::now().timestamp_millis();
        }
        info!("[{}] [publish] set none", self.stream);
        metrics::PUBLISH.dec();
        self.send_event(ForwardEventType::PublishDown, get_peer_id(&peer))
            .await;
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
            return Err(AppError::throw("sendonly is more than 1"));
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

        // NOTE: Disabled mDNS send
        // As a cloud server, we don't need this
        // But, as a local server, maybe we need this
        // https://github.com/binbat/live777/issues/155
        s.set_ice_multicast_dns_mode(MulticastDnsMode::Disabled);

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
            PublishTrackRemote::new(self.stream.clone(), get_peer_id(&peer), track).await;
        let mut publish_tracks = self.publish_tracks.write().await;
        publish_tracks.push(publish_track_remote);
        publish_tracks.sort_by(|a, b| a.rid.cmp(&b.rid));
        let _ = self.publish_tracks_change.send(());
        Ok(())
    }

    pub(crate) async fn publish_data_channel(
        &self,
        _peer: Arc<RTCPeerConnection>,
        dc: Arc<RTCDataChannel>,
    ) -> Result<()> {
        let sender = self.data_channel_forward.subscribe.clone();
        let receiver = self.data_channel_forward.publish.subscribe();
        Self::data_channel_forward(dc, sender, receiver).await;
        Ok(())
    }

    #[cfg(feature = "recorder")]
    pub(crate) async fn first_publish_video_codec(&self) -> Option<String> {
        let publish_tracks = self.publish_tracks.read().await;
        for t in publish_tracks.iter() {
            if t.kind == RTPCodecType::Video {
                let c = t.codec();
                return Some(format!(
                    "{}/{}",
                    c.kind.to_lowercase(),
                    c.codec.to_lowercase()
                ));
            }
        }
        None
    }

    /// Subscribe to notifications when publish tracks change (e.g., new tracks arrive).
    #[cfg(feature = "recorder")]
    pub(crate) fn subscribe_publish_tracks_change(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.publish_tracks_change.subscribe()
    }
}

// subscribe
impl PeerForwardInternal {
    pub(crate) async fn new_subscription_peer(
        &self,
        media_info: MediaInfo,
    ) -> Result<Arc<RTCPeerConnection>> {
        if media_info.video_transceiver.1 > 1 && media_info.audio_transceiver.1 > 1 {
            return Err(AppError::throw("recvonly is more than 1"));
        }
        let mut m = MediaEngine::default();
        m.register_default_codecs()?;
        let mut registry = Registry::new();
        registry = register_default_interceptors(registry, &mut m)?;
        let mut s = SettingEngine::default();
        s.detach_data_channels();

        // NOTE: Disabled mDNS send
        // As a cloud server, we don't need this
        // But, as a local server, maybe we need this
        // https://github.com/binbat/live777/issues/155
        s.set_ice_multicast_dns_mode(MulticastDnsMode::Disabled);

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
        Self::new_sender(&peer, RTPCodecType::Video, media_info.video_transceiver.1).await?;
        Self::new_sender(&peer, RTPCodecType::Audio, media_info.audio_transceiver.1).await?;
        Ok(peer)
    }

    async fn new_sender(
        peer: &Arc<RTCPeerConnection>,
        kind: RTPCodecType,
        recv_sender: u8,
    ) -> Result<Option<Arc<RTCRtpSender>>> {
        Ok(if recv_sender > 0 {
            let sender = peer
                .add_transceiver_from_kind(
                    kind,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Sendonly,
                        send_encodings: Vec::new(),
                    }),
                )
                .await?
                .sender()
                .await;
            let track = Arc::new(TrackLocalStaticRTP::new(
                if kind == RTPCodecType::Video {
                    RTCRtpCodecCapability {
                        mime_type: MIME_TYPE_VP8.to_owned(),
                        clock_rate: 90000,
                        channels: 0,
                        sdp_fmtp_line: "".to_owned(),
                        rtcp_feedback: vec![
                            RTCPFeedback {
                                typ: "goog-remb".to_owned(),
                                parameter: "".to_owned(),
                            },
                            RTCPFeedback {
                                typ: "ccm".to_owned(),
                                parameter: "fir".to_owned(),
                            },
                            RTCPFeedback {
                                typ: "nack".to_owned(),
                                parameter: "".to_owned(),
                            },
                            RTCPFeedback {
                                typ: "nack".to_owned(),
                                parameter: "pli".to_owned(),
                            },
                        ],
                    }
                } else {
                    RTCRtpCodecCapability {
                        mime_type: MIME_TYPE_OPUS.to_owned(),
                        clock_rate: 48000,
                        channels: 2,
                        sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                        rtcp_feedback: vec![],
                    }
                },
                "webrtc".to_string(),
                format!("{}-{}", "webrtc", kind),
            ));
            // ssrc for sdp
            let _ = sender.replace_track(Some(track)).await;
            info!(
                "[{}] new sender , kind : {}, ssrc : {}",
                get_peer_id(peer),
                kind,
                sender
                    .get_parameters()
                    .await
                    .encodings
                    .first()
                    .unwrap()
                    .ssrc
            );
            Some(sender)
        } else {
            None
        })
    }

    pub async fn add_subscribe(
        &self,
        peer: Arc<RTCPeerConnection>,
        cascade: Option<CascadeInfo>,
        media_info: MediaInfo,
    ) -> Result<()> {
        let transceivers = peer.get_transceivers().await;
        let mut video_sender = None;
        let mut audio_sender = None;
        for transceiver in transceivers {
            let sender = transceiver.sender().await;
            match transceiver.kind() {
                RTPCodecType::Video => video_sender = Some(sender),
                RTPCodecType::Audio => audio_sender = Some(sender),
                RTPCodecType::Unspecified => {}
            }
        }
        {
            let s = SubscribeRTCPeerConnection::new(
                cascade.clone(),
                self.stream.clone(),
                (peer.clone(), media_info),
                self.publish_rtcp_channel.clone(),
                (
                    self.publish_tracks.clone(),
                    self.publish_tracks_change.clone(),
                ),
                (video_sender, audio_sender),
            )
            .await;
            self.subscribe_group.write().await.push(s);
            *self.subscribe_leave_at.write().await = 0;
        }
        metrics::SUBSCRIBE.inc();
        self.send_event(ForwardEventType::SubscribeUp, get_peer_id(&peer))
            .await;
        if cascade.is_some() {
            metrics::REFORWARD.inc();
            self.send_event(ForwardEventType::ReforwardUp, get_peer_id(&peer))
                .await;
        }
        Ok(())
    }

    pub async fn remove_subscribe(&self, peer: Arc<RTCPeerConnection>) -> Result<()> {
        let mut flag = false;
        let mut reforward_flat = false;
        let session = get_peer_id(&peer);
        {
            let mut subscribe_peers = self.subscribe_group.write().await;
            for i in 0..subscribe_peers.len() {
                let subscribe = &mut subscribe_peers[i];
                if subscribe.id == session {
                    flag = true;
                    metrics::SUBSCRIBE.dec();
                    if let Some(cascade) = subscribe.cascade.clone() {
                        reforward_flat = true;
                        metrics::REFORWARD.dec();

                        let client = Client::build(
                            cascade.target_url.clone().unwrap(),
                            cascade.session_url.clone(),
                            Client::get_authorization_header_map(cascade.token.clone()),
                        );
                        tokio::spawn(async move {
                            let _ = client.remove_resource().await;
                        });
                    }
                    subscribe_peers.remove(i);
                    break;
                }
            }
            if subscribe_peers.is_empty() {
                *self.subscribe_leave_at.write().await = Utc::now().timestamp_millis();
            }
        }
        if flag {
            self.send_event(ForwardEventType::SubscribeDown, get_peer_id(&peer))
                .await;
            if reforward_flat {
                self.send_event(ForwardEventType::ReforwardDown, get_peer_id(&peer))
                    .await;
            }
            Ok(())
        } else {
            Err(AppError::throw("not found session"))
        }
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
        let sender = self.data_channel_forward.publish.clone();
        let receiver = self.data_channel_forward.subscribe.subscribe();
        Self::data_channel_forward(dc, sender, receiver).await;
        Ok(())
    }

    async fn send_event(&self, r#type: ForwardEventType, session: String) {
        let _ = self.event_sender.send(ForwardEvent {
            r#type,
            session,
            stream_info: self.info().await,
        });
    }
}
