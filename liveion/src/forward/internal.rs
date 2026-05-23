use std::sync::Arc;

use chrono::Utc;
use libwish::Client;
use tokio::sync::{Mutex, Notify, RwLock, broadcast};
use tracing::trace;
use tracing::{debug, info};

use webrtc::peer_connection::{
    PeerConnectionBuilder, PeerConnection, PeerConnectionEventHandler,
    RTCIceCandidateInit, RTCIceServer,
    RTCConfigurationBuilder, RTCPeerConnectionState,
    RTCIceGatheringState, Registry, MediaEngine, SettingEngine,
};
use rtc::peer_connection::configuration::media_engine::{MIME_TYPE_OPUS, MIME_TYPE_VP8};
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use webrtc::data_channel::DataChannel;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
use webrtc::media_stream::track_remote::TrackRemote;
use webrtc::rtp_transceiver::{
    RTCRtpTransceiverDirection, RTCRtpTransceiverInit, RtpSender,
};
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpEncodingParameters, RTCRtpCodingParameters, RtpCodecKind, RTCPFeedback,
    RTCRtpHeaderExtensionCapability,
};
use rtc::sdp::extmap::{SDES_MID_URI, SDES_RTP_STREAM_ID_URI};
use rtc::media_stream::MediaStreamTrack;
use crate::AppError;
#[cfg(feature = "source")]
use crate::config::Channel;
use crate::forward::get_peer_id;
use crate::forward::message::{ForwardInfo, SessionInfo};
use crate::forward::rtcp::RtcpMessage;
use crate::result::Result;
use crate::{metrics, new_broadcast_channel};

use super::media::MediaInfo;
use super::message::{CascadeInfo, ForwardEvent, ForwardEventType};
use super::publish::PublishRTCPeerConnection;
use super::subscribe::SubscribeRTCPeerConnection;
use super::track::PublishTrackRemote;

#[derive(Clone)]
struct DataChannelForward {
    publish: broadcast::Sender<Vec<u8>>,
    subscribe: broadcast::Sender<Vec<u8>>,
}

#[derive(Clone)]
struct PublishPeerHandler {
    internal: std::sync::Weak<PeerForwardInternal>,
    gather_complete: Arc<Notify>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for PublishPeerHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if let Some(internal) = self.internal.upgrade() {
            let pc = internal.publish_peer_ref.lock().await.clone().and_then(|w| w.upgrade());
            if let Some(pc) = pc {
                info!(
                    "[{}] [publish] connection state changed: {}",
                    internal.stream, state
                );
                if let Some(publish) = internal.publish.read().await.as_ref() {
                    publish.set_connection_state(state);
                }
                match state {
                    RTCPeerConnectionState::Failed => {
                        let _ = pc.close().await;
                    }
                    RTCPeerConnectionState::Closed => {
                        let _ = internal.remove_publish(pc).await;
                    }
                    _ => {}
                }
            }
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        if let Some(internal) = self.internal.upgrade() {
            let pc = internal.publish_peer_ref.lock().await.clone().and_then(|w| w.upgrade());
            if let Some(pc) = pc {
                let _ = internal.publish_track_up(pc, track).await;
            }
        }
    }

    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        if let Some(internal) = self.internal.upgrade() {
            let pc = internal.publish_peer_ref.lock().await.clone().and_then(|w| w.upgrade());
            if let Some(pc) = pc {
                let _ = internal.publish_data_channel(pc, dc).await;
            }
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            info!("publish ICE gathering complete");
            self.gather_complete.notify_one();
        }
    }
}

#[derive(Clone)]
struct SubscribePeerHandler {
    internal: std::sync::Weak<PeerForwardInternal>,
    peer: Arc<Mutex<Option<std::sync::Weak<dyn PeerConnection>>>>,
    gather_complete: Arc<Notify>,
}

impl SubscribePeerHandler {
    fn new(internal: std::sync::Weak<PeerForwardInternal>, gather_complete: Arc<Notify>) -> Self {
        Self {
            internal,
            peer: Arc::new(Mutex::new(None)),
            gather_complete,
        }
    }

    async fn set_peer(&self, peer: std::sync::Weak<dyn PeerConnection>) {
        *self.peer.lock().await = Some(peer);
    }
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for SubscribePeerHandler {
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let pc = self.peer.lock().await.clone().and_then(|w| w.upgrade());
        if let (Some(internal), Some(pc)) = (self.internal.upgrade(), pc) {
            info!(
                "[{}] [subscribe] connection state changed: {}",
                internal.stream, state
            );
            match state {
                RTCPeerConnectionState::Failed => {
                    let _ = pc.close().await;
                }
                RTCPeerConnectionState::Closed => {
                    let _ = internal.remove_subscribe(pc).await;
                }
                _ => {}
            }
        }
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let pc = self.peer.lock().await.clone().and_then(|w| w.upgrade());
        if let (Some(internal), Some(_pc)) = (self.internal.upgrade(), pc) {
            let kind = track.kind().await;
            let ssrcs = track.ssrcs().await;
            info!(
                "[{}] [subscribe] on_track: kind={}, ssrcs={:?}, id={}",
                internal.stream, kind, ssrcs, track.track_id().await
            );
            // Subscribe peer is sendonly — incoming tracks from the remote
            // are unexpected but logged for debugging.
        }
    }

    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        let pc = self.peer.lock().await.clone().and_then(|w| w.upgrade());
        if let (Some(internal), Some(pc)) = (self.internal.upgrade(), pc) {
            let _ = internal.subscribe_data_channel(pc, dc).await;
        }
    }

    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            info!("subscribe ICE gathering complete");
            self.gather_complete.notify_one();
        }
    }
}

pub(crate) struct PeerForwardInternal {
    pub(crate) stream: String,
    create_at: i64,
    publish_leave_at: RwLock<i64>,
    subscribe_leave_at: RwLock<i64>,
    publish: RwLock<Option<PublishRTCPeerConnection>>,
    pub(crate) publish_tracks: Arc<RwLock<Vec<PublishTrackRemote>>>,
    pub(crate) publish_tracks_change: broadcast::Sender<()>,
    pub(crate) publish_rtcp_channel: broadcast::Sender<(RtcpMessage, u32)>,
    pub(crate) subscribe_group: RwLock<Vec<SubscribeRTCPeerConnection>>,
    data_channel_forward: DataChannelForward,
    ice_server: Vec<RTCIceServer>,
    event_sender: broadcast::Sender<ForwardEvent>,
    /// Weak reference to the publish peer, set before signaling via `set_publish_peer_ref`
    publish_peer_ref: Mutex<Option<std::sync::Weak<dyn PeerConnection>>>,
    #[cfg(feature = "source")]
    channel: Channel,
}

impl PeerForwardInternal {
    #[cfg(feature = "source")]
    pub(crate) fn new(
        stream: impl ToString,
        ice_server: Vec<RTCIceServer>,
        channel: Channel,
    ) -> Self {
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
            publish_peer_ref: Mutex::new(None),
            channel,
        }
    }

    #[cfg(not(feature = "source"))]
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
            publish_peer_ref: Mutex::new(None),
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

        let publish_tracks = self.publish_tracks.read().await;

        #[cfg(feature = "source")]
        let has_virtual_publisher = publish_tracks
            .iter()
            .any(|track| matches!(track, PublishTrackRemote::Virtual(_)));

        #[cfg(not(feature = "source"))]
        let has_virtual_publisher = false;

        let publish_session_info = match self.publish.read().await.as_ref() {
            Some(publish) => Some(publish.info().await),
            None => None,
        };

        let effective_publish_session_info = if publish_session_info.is_none()
            && has_virtual_publisher
        {
            Some(SessionInfo {
            id: "virtual-source".to_string(),
            create_at: self.create_at,
            state: RTCPeerConnectionState::Connected,
            cascade: None,
            has_data_channel: false,
        })
        } else {
            publish_session_info
        };

        ForwardInfo {
            id: self.stream.clone(),
            create_at: self.create_at,
            publish_leave_at: *self.publish_leave_at.read().await,
            subscribe_leave_at: *self.subscribe_leave_at.read().await,
            publish_session_info: effective_publish_session_info,
            subscribe_session_infos,
            codecs: publish_tracks.iter().map(|track| track.codec()).collect(),
            has_virtual_publisher,
        }
    }

    pub(crate) async fn add_ice_candidate(
        &self,
        id: String,
        ice_candidates: Vec<RTCIceCandidateInit>,
    ) -> Result<()> {
        trace!(
            "Adding {} ICE candidates for session {}",
            ice_candidates.len(),
            id
        );

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
        dc: Arc<dyn DataChannel>,
        sender: broadcast::Sender<Vec<u8>>,
        receiver: broadcast::Receiver<Vec<u8>>,
    ) {
        let dc_rx = dc.clone();
        let dc_tx = dc.clone();

        tokio::spawn(async move {
            loop {
                match dc_rx.poll().await {
                    Some(webrtc::data_channel::DataChannelEvent::OnMessage(data)) => {
                        if let Err(err) = sender.send(data.data.to_vec()) {
                            info!("send data channel err: {}", err);
                            return;
                        }
                    }
                    Some(webrtc::data_channel::DataChannelEvent::OnOpen) => {
                        debug!("Data channel opened");
                    }
                    Some(webrtc::data_channel::DataChannelEvent::OnClose) | None => {
                        info!("Datachannel closed; Exit the read_loop");
                        return;
                    }
                    _ => {}
                }
            }
        });

        tokio::spawn(async move {
            let mut receiver = receiver;
            while let Ok(msg) = receiver.recv().await {
                if let Err(err) = dc_tx.send(bytes::BytesMut::from(&msg[..])).await {
                    info!("write data channel err: {}", err);
                    return;
                }
            }
        });
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
        peer: Arc<dyn PeerConnection>,
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

    pub(crate) async fn remove_publish(&self, peer: Arc<dyn PeerConnection>) -> Result<()> {
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
            .filter(|t| t.kind() == RtpCodecKind::Video)
            .map(|t| t.rid().to_string())
            .collect::<Vec<_>>();
        Ok(rids)
    }

    pub(crate) async fn publisher_codec(&self, kind: RtpCodecKind) -> Option<RTCRtpCodec> {
        let publish_tracks = self.publish_tracks.read().await;
        for t in publish_tracks.iter() {
            if t.kind() == kind {
                return Some(match t {
                    PublishTrackRemote::Real { track, .. } => {
                        let ssrcs = track.ssrcs().await;
                        let first_ssrc = ssrcs.first().copied().unwrap_or(0);
                        track.codec(first_ssrc).await.unwrap_or_default()
                    }
                    #[cfg(feature = "source")]
                    PublishTrackRemote::Virtual(v) => v.codec_params.rtp_codec.clone(),
                });
            }
        }
        None
    }

    pub(crate) async fn new_publish_peer(
        &self,
        media_info: MediaInfo,
        internal_weak: std::sync::Weak<PeerForwardInternal>,
    ) -> Result<(Arc<dyn PeerConnection>, Arc<Notify>)> {
        if media_info.video_transceiver.0 > 1 && media_info.audio_transceiver.0 > 1 {
            return Err(AppError::throw("sendonly is more than 1"));
        }

        let mut m = MediaEngine::default();
        m.register_default_codecs()?;

        m.register_header_extension(
            RTCRtpHeaderExtensionCapability {
                uri: SDES_MID_URI.to_owned(),
            },
            RtpCodecKind::Video,
            Some(RTCRtpTransceiverDirection::Recvonly),
        )?;

        m.register_header_extension(
            RTCRtpHeaderExtensionCapability {
                uri: SDES_RTP_STREAM_ID_URI.to_owned(),
            },
            RtpCodecKind::Video,
            Some(RTCRtpTransceiverDirection::Recvonly),
        )?;

        let registry = Registry::new();
        let registry = register_default_interceptors(registry, &mut m)?;

        let s = SettingEngine::default();

        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(self.ice_server.clone())
            .build();

        let gather_complete = Arc::new(Notify::new());
        let handler = PublishPeerHandler { internal: internal_weak, gather_complete: gather_complete.clone() };
        let peer: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::<std::net::SocketAddr>::new()
                .with_media_engine(m)
                .with_interceptor_registry(registry)
                .with_setting_engine(s)
                .with_handler(Arc::new(handler))
                .with_udp_addrs(vec!["0.0.0.0:0".parse().unwrap()])
                .with_configuration(config)
                .build()
                .await?,
        );
        // Store weak ref so the handler can find the peer during events
        *self.publish_peer_ref.lock().await = Some(Arc::downgrade(&peer));

        let mut transceiver_kinds = vec![];
        if media_info.video_transceiver.0 > 0 {
            transceiver_kinds.push(RtpCodecKind::Video);
        }
        if media_info.audio_transceiver.0 > 0 {
            transceiver_kinds.push(RtpCodecKind::Audio);
        }

        for kind in transceiver_kinds {
            let _ = peer
                .add_transceiver_from_kind(
                    kind,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Recvonly,
                        streams: vec![],
                        send_encodings: Vec::new(),
                    }),
                )
                .await?;
        }

        Ok((peer, gather_complete))
    }

    pub(crate) async fn publish_track_up(
        &self,
        peer: Arc<dyn PeerConnection>,
        track: Arc<dyn TrackRemote>,
    ) -> Result<()> {
        let publish_track_remote =
            PublishTrackRemote::new(self.stream.clone(), get_peer_id(&peer), track).await;

        let mut publish_tracks = self.publish_tracks.write().await;
        publish_tracks.push(publish_track_remote);
        publish_tracks.sort_by(|a, b| a.rid().cmp(b.rid()));

        let _ = self.publish_tracks_change.send(());

        Ok(())
    }

    pub(crate) async fn publish_data_channel(
        &self,
        _peer: Arc<dyn PeerConnection>,
        dc: Arc<dyn DataChannel>,
    ) -> Result<()> {
        let sender = self.data_channel_forward.subscribe.clone();
        let receiver = self.data_channel_forward.publish.subscribe();
        // DataChannel ↔ UDP bidirectional forwarding (feature=source only).
        // Messages from the WHIP publisher arrive on the subscribe channel.
        #[cfg(feature = "source")]
        if let Some(stream_cfg) = self.channel.streams.get(&self.stream).cloned() {
            // UDP acts as a member of the WHIP group:
            // - dc_rx: receive messages from WHEP group (publish channel)
            // - dc_tx: send messages to WHEP group (subscribe channel)
            let dc_rx = self.data_channel_forward.publish.subscribe();
            let dc_tx = self.data_channel_forward.subscribe.clone();
            super::channel::spawn_channel(self.stream.clone(), dc_rx, dc_tx, stream_cfg).await?;
        }
        Self::data_channel_forward(dc, sender, receiver).await;
        Ok(())
    }

    #[cfg(feature = "recorder")]
    pub(crate) async fn first_publish_video_codec(&self) -> Option<String> {
        let publish_tracks = self.publish_tracks.read().await;
        for t in publish_tracks.iter() {
            if t.kind() == RtpCodecKind::Video {
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

    #[cfg(feature = "recorder")]
    pub(crate) fn subscribe_publish_tracks_change(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.publish_tracks_change.subscribe()
    }

    #[cfg(feature = "recorder")]
    pub(crate) async fn first_video_track(
        &self,
    ) -> Option<Arc<dyn TrackRemote>> {
        let publish_tracks = self.publish_tracks.read().await;
        publish_tracks.iter().find_map(|track| match track {
            PublishTrackRemote::Real { track, kind, .. }
                if *kind == RtpCodecKind::Video =>
            {
                Some(track.clone())
            }
            _ => None,
        })
    }

    #[cfg(feature = "recorder")]
    pub(crate) async fn send_rtcp_to_publish(
        &self,
        message: crate::forward::rtcp::RtcpMessage,
        ssrc: u32,
    ) -> Result<()> {
        if self.publish_rtcp_channel.send((message, ssrc)).is_err() {
            return Err(crate::error::AppError::throw("Failed to send RTCP message"));
        }
        Ok(())
    }
}

// subscribe
impl PeerForwardInternal {
    pub(crate) async fn new_subscription_peer(
        &self,
        media_info: MediaInfo,
        internal_weak: std::sync::Weak<PeerForwardInternal>,
    ) -> Result<(Arc<dyn PeerConnection>, Arc<Notify>)> {
        if media_info.video_transceiver.1 > 1 && media_info.audio_transceiver.1 > 1 {
            return Err(AppError::throw("recvonly is more than 1"));
        }

        let mut m = MediaEngine::default();
        m.register_default_codecs()?;

        let registry = Registry::new();
        let registry = register_default_interceptors(registry, &mut m)?;

        let s = SettingEngine::default();

        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(self.ice_server.clone())
            .build();

        let gather_complete = Arc::new(Notify::new());
        let handler = SubscribePeerHandler::new(internal_weak, gather_complete.clone());
        let peer: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::<std::net::SocketAddr>::new()
                .with_media_engine(m)
                .with_interceptor_registry(registry)
                .with_setting_engine(s)
                .with_handler(Arc::new(handler.clone()))
                .with_udp_addrs(vec!["0.0.0.0:0".parse().unwrap()])
                .with_configuration(config)
                .build()
                .await?,
        );
        handler.set_peer(Arc::downgrade(&peer)).await;

        // Use the publisher's negotiated codec for the subscriber's sender encoding.
        // This ensures the encoding codec matches what the publisher is actually sending,
        // so the rtc-layer write_rtp uses the correct payload type.
        let video_codec = self.publisher_codec(RtpCodecKind::Video).await;
        let audio_codec = self.publisher_codec(RtpCodecKind::Audio).await;

        Self::new_sender(&peer, RtpCodecKind::Video, media_info.video_transceiver.1, video_codec).await?;
        Self::new_sender(&peer, RtpCodecKind::Audio, media_info.audio_transceiver.1, audio_codec).await?;

        Ok((peer, gather_complete))
    }

    async fn new_sender(
        peer: &Arc<dyn PeerConnection>,
        kind: RtpCodecKind,
        recv_sender: u8,
        publisher_codec: Option<RTCRtpCodec>,
    ) -> Result<Option<Arc<dyn RtpSender>>> {
        Ok(if recv_sender > 0 {
            let default_codec = if kind == RtpCodecKind::Video {
                RTCRtpCodec {
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
                RTCRtpCodec {
                    mime_type: MIME_TYPE_OPUS.to_owned(),
                    clock_rate: 48000,
                    channels: 2,
                    sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                    rtcp_feedback: vec![],
                }
            };
            // Use the publisher's negotiated codec if available, otherwise fall back to default.
            // This ensures the subscriber's sender encoding codec matches the publisher's codec,
            // so the rtc-layer write_rtp uses the correct payload type.
            let codec = publisher_codec.unwrap_or(default_codec);

            // Use a single SSRC for both the encoding and the track.
            // The rtc-layer write_rtp validates that packet.ssrc matches sender.track().ssrcs(),
            // so the track's SSRC must match the encoding's SSRC.
            let ssrc = rand::random::<u32>();

            let transceiver = peer
                .add_transceiver_from_kind(
                    kind,
                    Some(RTCRtpTransceiverInit {
                        direction: RTCRtpTransceiverDirection::Sendonly,
                        streams: vec![],
                        send_encodings: vec![RTCRtpEncodingParameters {
                            rtp_coding_parameters: RTCRtpCodingParameters {
                                ssrc: Some(ssrc),
                                ..Default::default()
                            },
                            codec: codec.clone(),
                            ..Default::default()
                        }],
                    }),
                )
                .await?;

            let sender = transceiver.sender().await
                .map_err(|e| anyhow::anyhow!("Failed to get sender: {}", e))?
                .ok_or_else(|| anyhow::anyhow!("No sender found"))?;

            // Create a replacement track with the SAME SSRC as the encoding.
            // add_transceiver_from_kind already created a track, but we replace it
            // to ensure the track object is under our control for future replace_track calls.
            let media_track = MediaStreamTrack::new(
                "webrtc".to_string(),
                format!("{}-{}", "webrtc", kind),
                "webrtc".to_string(),
                kind,
                vec![RTCRtpEncodingParameters {
                    rtp_coding_parameters: RTCRtpCodingParameters {
                        ssrc: Some(ssrc),
                        ..Default::default()
                    },
                    codec,
                    ..Default::default()
                }],
            );
            let track = Arc::new(TrackLocalStaticRTP::new(media_track));

            let _ = sender.replace_track(track).await;

            let params = sender.get_parameters().await
                .map_err(|e| anyhow::anyhow!("Failed to get parameters: {}", e))?;
            info!(
                "[{}] new sender , kind : {}, ssrc : {}",
                get_peer_id(peer),
                kind,
                params.encodings.first().map(|e| e.rtp_coding_parameters.ssrc.unwrap_or(0)).unwrap_or(0),
            );

            Some(sender)
        } else {
            None
        })
    }

    pub async fn add_subscribe(
        &self,
        peer: Arc<dyn PeerConnection>,
        cascade: Option<CascadeInfo>,
        media_info: MediaInfo,
    ) -> Result<()> {
        let transceivers = peer.get_transceivers().await;

        let mut video_sender = None;
        let mut audio_sender = None;

        for transceiver in transceivers {
            let sender = transceiver.sender().await
                .map_err(|e| anyhow::anyhow!("Failed to get sender: {}", e))?;
            let _kind = transceiver.current_direction().await
                .map_err(|e| anyhow::anyhow!("Failed to get direction: {}", e))?;
            // Determine kind from the sender's track
            if let Some(ref s) = sender {
                let track_kind = s.track().kind().await;
                match track_kind {
                    RtpCodecKind::Video => video_sender = sender,
                    RtpCodecKind::Audio => audio_sender = sender,
                    RtpCodecKind::Unspecified => {}
                }
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

    pub async fn remove_subscribe(&self, peer: Arc<dyn PeerConnection>) -> Result<()> {
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

    pub async fn select_kind_rid(&self, id: String, kind: RtpCodecKind, rid: String) -> Result<()> {
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
        _peer: Arc<dyn PeerConnection>,
        dc: Arc<dyn DataChannel>,
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
