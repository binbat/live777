use std::io::Cursor;
use std::sync::Arc;

use crate::forward::info::ForwardInfo;
use crate::result::Result;
use tokio::sync::Mutex;
use tracing::info;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;

use libwish::Client;
use webrtc::sdp::SessionDescription;

use crate::dto::req::ChangeResourceReq;
use crate::forward::forward_internal::PeerForwardInternal;
use crate::forward::info::Layer;
use crate::{constant, AppError};

use self::info::ReforwardInfo;
use self::media::MediaInfo;

mod forward_internal;
pub mod info;
mod media;
mod publish;
mod rtcp;
mod subscribe;
mod track;

pub(crate) fn get_peer_id(peer: &Arc<RTCPeerConnection>) -> String {
    let digest = md5::compute(peer.get_stats_id());
    format!("{:x}", digest)
}

#[derive(Clone)]
pub struct PeerForward {
    publish_lock: Arc<Mutex<()>>,
    internal: Arc<PeerForwardInternal>,
}

impl PeerForward {
    pub fn new(id: impl ToString, ice_server: Vec<RTCIceServer>) -> Self {
        PeerForward {
            publish_lock: Arc::new(Mutex::new(())),
            internal: Arc::new(PeerForwardInternal::new(id, ice_server)),
        }
    }

    pub async fn set_publish(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<(RTCSessionDescription, String)> {
        if self.internal.publish_is_some().await {
            return Err(AppError::resource_already_exists(
                "A connection has already been established",
            ));
        }
        let _ = self.publish_lock.lock().await;
        if self.internal.publish_is_some().await {
            return Err(AppError::resource_already_exists(
                "A connection has already been established",
            ));
        }
        let peer = self
            .internal
            .new_publish_peer(MediaInfo::try_from(offer.unmarshal()?)?)
            .await?;
        let internal = Arc::downgrade(&self.internal);
        let pc = Arc::downgrade(&peer);
        peer.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            if let (Some(internal), Some(pc)) = (internal.upgrade(), pc.upgrade()) {
                tokio::spawn(async move {
                    info!(
                        "[{}] [publish] [{}] connection state changed: {}",
                        internal.id,
                        get_peer_id(&pc),
                        s
                    );
                    match s {
                        RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                            let _ = pc.close().await;
                        }
                        RTCPeerConnectionState::Closed => {
                            let _ = internal.remove_publish(pc).await;
                        }
                        _ => {}
                    };
                });
            }
            Box::pin(async {})
        }));
        let internal = Arc::downgrade(&self.internal);
        let pc = Arc::downgrade(&peer);
        peer.on_track(Box::new(move |track, _, _| {
            if let (Some(internal), Some(pc)) = (internal.upgrade(), pc.upgrade()) {
                tokio::spawn(async move {
                    let _ = internal.publish_track_up(pc, track).await;
                });
            }
            Box::pin(async {})
        }));
        let internal = Arc::downgrade(&self.internal);
        let pc = Arc::downgrade(&peer);
        peer.on_data_channel(Box::new(move |dc| {
            if let (Some(internal), Some(pc)) = (internal.upgrade(), pc.upgrade()) {
                tokio::spawn(async move {
                    let _ = internal.publish_data_channel(pc, dc).await;
                });
            }
            Box::pin(async {})
        }));
        let description = peer_complete(offer, peer.clone()).await?;
        self.internal.set_publish(peer.clone()).await?;
        Ok((description, get_peer_id(&peer)))
    }

    pub async fn add_subscribe(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<(RTCSessionDescription, String)> {
        if !self.internal.publish_is_ok().await {
            return Err(AppError::throw("publish is not ok"));
        }
        let peer = self
            .internal
            .new_subscription_peer(MediaInfo::try_from(offer.unmarshal()?)?, None)
            .await?;
        let internal = Arc::downgrade(&self.internal);
        let pc = Arc::downgrade(&peer);
        peer.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            if let (Some(internal), Some(pc)) = (internal.upgrade(), pc.upgrade()) {
                tokio::spawn(async move {
                    info!(
                        "[{}] [subscribe] [{}] connection state changed: {}",
                        internal.id,
                        get_peer_id(&pc),
                        s
                    );
                    match s {
                        RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                            let _ = pc.close().await;
                        }
                        RTCPeerConnectionState::Closed => {
                            let _ = internal.remove_subscribe(pc).await;
                        }
                        _ => {}
                    }
                });
            }
            Box::pin(async {})
        }));
        let internal = Arc::downgrade(&self.internal);
        let pc = Arc::downgrade(&peer);
        peer.on_data_channel(Box::new(move |dc| {
            if let (Some(internal), Some(pc)) = (internal.upgrade(), pc.upgrade()) {
                tokio::spawn(async move {
                    let _ = internal.subscribe_data_channel(pc, dc).await;
                });
            }
            Box::pin(async {})
        }));
        let (sdp, key) = (
            peer_complete(offer, peer.clone()).await?,
            get_peer_id(&peer),
        );
        Ok((sdp, key))
    }

    pub async fn reforward(&self, reforward_info: ReforwardInfo) -> Result<()> {
        if !self.internal.publish_is_ok().await {
            return Err(AppError::throw("publish is not ok"));
        }
        let publish_peer = self
            .internal
            .get_publish_peer()
            .await
            .ok_or(AppError::throw("not found publish peer"))?;
        let mut media_info = MediaInfo::try_from(
            publish_peer
                .remote_description()
                .await
                .ok_or(AppError::throw("get publish peer sdp error"))?
                .unmarshal()?,
        )?;
        if media_info.video_transceiver.2 {
            return Err(AppError::throw("svc not support re forward"));
        }
        let video_publish = media_info.video_transceiver.0;
        let audio_publish = media_info.audio_transceiver.0;
        media_info.video_transceiver.0 = media_info.video_transceiver.1;
        media_info.video_transceiver.1 = video_publish;
        media_info.audio_transceiver.0 = media_info.audio_transceiver.1;
        media_info.audio_transceiver.1 = audio_publish;
        let mut reforward_info = reforward_info.clone();
        reforward_info.resource_url = None;
        let target_peer = self
            .internal
            .new_subscription_peer(media_info, Some(reforward_info.clone()))
            .await?;
        let internal = Arc::downgrade(&self.internal);
        let pc = Arc::downgrade(&target_peer);
        target_peer.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
            if let (Some(internal), Some(pc)) = (internal.upgrade(), pc.upgrade()) {
                tokio::spawn(async move {
                    info!(
                        "[{}] [reforward] [{}] connection state changed: {}",
                        internal.id,
                        get_peer_id(&pc),
                        s
                    );
                    match s {
                        RTCPeerConnectionState::Failed | RTCPeerConnectionState::Disconnected => {
                            let _ = pc.close().await;
                        }
                        RTCPeerConnectionState::Closed => {
                            let _ = internal.remove_subscribe(pc).await;
                        }
                        _ => {}
                    }
                });
            }
            Box::pin(async {})
        }));
        let offer = target_peer.create_offer(None).await?;
        let mut gather_complete = target_peer.gathering_complete_promise().await;
        target_peer.set_local_description(offer).await?;
        let _ = gather_complete.recv().await;
        let description = target_peer
            .pending_local_description()
            .await
            .ok_or(AppError::throw("pending_local_description error"))?;
        let mut client = Client::new(
            reforward_info.whip_url.clone(),
            Client::get_auth_header_map(
                reforward_info.basic.clone(),
                reforward_info.token.clone(),
            ),
        );
        match client.wish(description.sdp.clone()).await {
            Ok((target_sdp, _)) => {
                let _ = target_peer.set_remote_description(target_sdp).await;
                reforward_info.resource_url = client.resource_url;
                self.internal
                    .set_reforward_info(target_peer, reforward_info)
                    .await?;
                Ok(())
            }
            Err(err) => {
                target_peer.close().await?;
                Err(AppError::InternalServerError(err))
            }
        }
    }

    pub async fn add_ice_candidate(&self, key: String, ice_candidates: String) -> Result<()> {
        let ice_candidates = parse_ice_candidate(ice_candidates)?;
        if ice_candidates.is_empty() {
            return Ok(());
        }
        self.internal.add_ice_candidate(key, ice_candidates).await
    }

    pub async fn remove_peer(&self, key: String) -> Result<bool> {
        self.internal.remove_peer(key).await
    }

    pub async fn layers(&self) -> Result<Vec<Layer>> {
        if self.internal.publish_is_svc().await {
            let mut layers = vec![];
            for rid in self.internal.publish_svc_rids().await? {
                layers.push(Layer {
                    encoding_id: rid.to_owned(),
                });
            }
            Ok(layers)
        } else {
            Err(AppError::throw("not layers"))
        }
    }

    pub async fn select_layer(&self, key: String, layer: Option<Layer>) -> Result<()> {
        let rid = if let Some(layer) = layer {
            layer.encoding_id
        } else {
            self.internal.publish_svc_rids().await?[0].clone()
        };
        self.internal
            .select_kind_rid(key, RTPCodecType::Video, rid)
            .await
    }

    pub async fn change_resource(
        &self,
        key: String,
        change_resource: ChangeResourceReq,
    ) -> Result<()> {
        let codec_type = RTPCodecType::from(change_resource.kind.as_str());
        if codec_type == RTPCodecType::Unspecified {
            return Err(AppError::throw("kind unspecified"));
        }

        let rid = if change_resource.enabled {
            constant::RID_ENABLE.to_string()
        } else {
            constant::RID_DISABLE.to_string()
        };
        self.internal.select_kind_rid(key, codec_type, rid).await
    }

    pub async fn close(&self) -> Result<()> {
        self.internal.close().await?;
        Ok(())
    }

    pub async fn info(&self) -> ForwardInfo {
        self.internal.info().await
    }
}

async fn peer_complete(
    offer: RTCSessionDescription,
    peer: Arc<RTCPeerConnection>,
) -> Result<RTCSessionDescription> {
    peer.set_remote_description(offer).await?;
    let answer = peer.create_answer(None).await?;
    let mut gather_complete = peer.gathering_complete_promise().await;
    peer.set_local_description(answer).await?;
    let _ = gather_complete.recv().await;
    let description = peer
        .local_description()
        .await
        .ok_or(anyhow::anyhow!("failed to get local description"))?;
    Ok(description)
}

fn parse_ice_candidate(content: String) -> Result<Vec<RTCIceCandidateInit>> {
    let content = format!(
        "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n{}",
        content
    );
    let mut reader = Cursor::new(content);
    let session_desc = SessionDescription::unmarshal(&mut reader)?;
    let mut ice_candidates = Vec::new();
    for media_descriptions in session_desc.media_descriptions {
        let attributes = media_descriptions.attributes;
        let mid = attributes
            .iter()
            .filter(|attr| attr.key == "mid")
            .map(|attr| attr.value.clone())
            .last();
        let mid = mid
            .ok_or_else(|| anyhow::anyhow!("no mid"))?
            .ok_or_else(|| anyhow::anyhow!("no mid"))?;
        let mline_index = mid.parse::<u16>()?;
        for attr in attributes {
            if attr.is_ice_candidate() {
                if let Some(value) = attr.value {
                    ice_candidates.push(RTCIceCandidateInit {
                        candidate: value,
                        sdp_mid: Some(mid.clone()),
                        sdp_mline_index: Some(mline_index),
                        username_fragment: None,
                    });
                }
            }
        }
    }
    Ok(ice_candidates)
}

pub(crate) fn peer_connect_state(peer: &Arc<RTCPeerConnection>) -> u8 {
    match peer.connection_state() {
        RTCPeerConnectionState::Unspecified => 0,
        RTCPeerConnectionState::New => 1,
        RTCPeerConnectionState::Connecting => 2,
        RTCPeerConnectionState::Connected => 3,
        RTCPeerConnectionState::Disconnected => 4,
        RTCPeerConnectionState::Failed => 5,
        RTCPeerConnectionState::Closed => 6,
    }
}

#[cfg(test)]
mod test {
    use crate::forward::parse_ice_candidate;

    #[test]
    fn test_parse_ice_candidate() -> crate::result::Result<()> {
        let body = "a=ice-ufrag:EsAw
a=ice-pwd:P2uYro0UCOQ4zxjKXaWCBui1
m=audio 9 RTP/AVP 0
a=mid:0
a=candidate:1387637174 1 udp 2122260223 192.0.2.1 61764 typ host generation 0 ufrag EsAw network-id 1
a=candidate:3471623853 1 udp 2122194687 198.51.100.1 61765 typ host generation 0 ufrag EsAw network-id 2
a=candidate:473322822 1 tcp 1518280447 192.0.2.1 9 typ host tcptype active generation 0 ufrag EsAw network-id 1
a=candidate:2154773085 1 tcp 1518214911 198.51.100.2 9 typ host tcptype active generation 0 ufrag EsAw network-id 2
a=end-of-candidates";
        parse_ice_candidate(body.to_owned())?;
        Ok(())
    }
}
