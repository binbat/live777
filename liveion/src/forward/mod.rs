use std::io::Cursor;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, broadcast};
#[cfg(feature = "source")]
use tracing::{debug, trace, warn};
use tracing::error;
use webrtc::peer_connection::{
    PeerConnection, RTCIceCandidateInit, RTCIceServer,
    RTCSessionDescription,
};
use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;

use libwish::Client;

#[cfg(feature = "source")]
use crate::config::Channel;
use crate::forward::internal::PeerForwardInternal;
use crate::forward::message::{ForwardInfo, Layer};
use crate::result::Result;
use crate::{AppError, constant};
#[cfg(feature = "source")]
pub use bridge::SourceBridge;
#[cfg(feature = "source")]
use rtc::rtp::packet::Packet;
#[cfg(feature = "source")]
use rtc::shared::marshal::Unmarshal;

use self::media::MediaInfo;
use self::message::{CascadeInfo, ForwardEvent};

#[cfg(feature = "source")]
pub(crate) mod channel;
mod internal;
mod media;
pub mod message;
mod publish;
pub mod rtcp;
mod subscribe;

#[cfg(not(feature = "source"))]
mod track;

use md5::{Digest, Md5};

#[cfg(feature = "source")]
pub mod bridge;

#[cfg(feature = "source")]
pub mod track;

pub(crate) fn get_peer_id(peer: &Arc<dyn PeerConnection>) -> String {
    let mut hasher = Md5::new();
    hasher.update(format!("{:?}", Arc::as_ptr(peer)));
    let digest = hasher.finalize();
    format!("{digest:x}")
}

#[derive(Clone)]
pub struct PeerForward {
    pub(crate) stream: String,
    publish_lock: Arc<Mutex<()>>,
    pub(crate) internal: Arc<PeerForwardInternal>,
}

#[cfg(feature = "recorder")]
#[derive(Clone, Debug)]
pub struct AudioTrackInfo {
    pub clock_rate: u32,
    pub channels: u16,
    pub codec_mime: String,
    pub fmtp: String,
}

impl PeerForward {
    #[cfg(feature = "source")]
    pub fn new(stream: impl ToString, ice_server: Vec<RTCIceServer>, channel: Channel) -> Self {
        PeerForward {
            stream: stream.to_string(),
            publish_lock: Arc::new(Mutex::new(())),
            internal: Arc::new(PeerForwardInternal::new(stream, ice_server, channel)),
        }
    }

    #[cfg(not(feature = "source"))]
    pub fn new(stream: impl ToString, ice_server: Vec<RTCIceServer>) -> Self {
        PeerForward {
            stream: stream.to_string(),
            publish_lock: Arc::new(Mutex::new(())),
            internal: Arc::new(PeerForwardInternal::new(stream, ice_server)),
        }
    }
    pub fn subscribe_event(&self) -> broadcast::Receiver<ForwardEvent> {
        self.internal.subscribe_event()
    }

    pub async fn add_ice_candidate(&self, session: String, ice_candidates: String) -> Result<()> {
        let ice_candidates = parse_ice_candidate(ice_candidates)?;
        if ice_candidates.is_empty() {
            return Ok(());
        }
        self.internal
            .add_ice_candidate(session, ice_candidates)
            .await
    }

    pub async fn remove_peer(&self, session: String) -> Result<bool> {
        self.internal.remove_peer(session).await
    }

    pub async fn close(&self) -> Result<()> {
        self.internal.close().await?;
        Ok(())
    }

    pub async fn info(&self) -> ForwardInfo {
        self.internal.info().await
    }

    #[cfg(feature = "source")]
    pub async fn get_subscribe_peer(&self, session_id: &str) -> Option<Arc<dyn PeerConnection>> {
        let subscribe_group = self.internal.subscribe_group.read().await;
        for subscribe in subscribe_group.iter() {
            if subscribe.id == session_id {
                return Some(subscribe.peer.clone());
            }
        }

        None
    }
}

// publish
impl PeerForward {
    pub async fn set_publish(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<(RTCSessionDescription, String)> {
        if self.internal.publish_is_some().await {
            return Err(AppError::stream_already_exists(
                "A connection has already been established",
            ));
        }

        let _ = self.publish_lock.lock().await;

        if self.internal.publish_is_some().await {
            return Err(AppError::stream_already_exists(
                "A connection has already been established",
            ));
        }

        let media_info = MediaInfo::try_from(unmarshal_sdp(&offer.sdp)?)?;
        let (peer, gather_complete) = self.new_publish_peer(media_info).await?;

        let description = peer_complete(offer, peer.clone(), gather_complete).await?;

        self.internal.set_publish(peer.clone(), None).await?;

        let session = get_peer_id(&peer);

        Ok((description, session))
    }

    pub async fn publish_pull(&self, src: String, token: Option<String>) -> Result<()> {
        if self.internal.publish_is_some().await {
            return Err(AppError::stream_already_exists(
                "A connection has already been established",
            ));
        }

        let _ = self.publish_lock.lock().await;

        if self.internal.publish_is_some().await {
            return Err(AppError::stream_already_exists(
                "A connection has already been established",
            ));
        }

        let (peer, gather_complete) = self
            .new_publish_peer(MediaInfo {
                _codec: vec![],
                video_transceiver: (1, 0, false),
                audio_transceiver: (1, 0),
                has_data_channel: false,
            })
            .await?;

        let offer = peer.create_offer(None).await?;
        peer.set_local_description(offer).await?;
        gather_complete.notified().await;

        let description = peer
            .pending_local_description()
            .await
            .ok_or(AppError::throw("pending_local_description error"))?;

        let mut client = Client::new(
            src.clone(),
            Client::get_authorization_header_map(token.clone()),
        );

        match client.wish(description.sdp.clone()).await {
            Ok((target_sdp, _)) => {
                let _ = peer.set_remote_description(target_sdp).await;
                self.internal
                    .set_publish(
                        peer.clone(),
                        Some(CascadeInfo {
                            source_url: Some(src),
                            target_url: None,
                            token,
                            session_url: client.session_url,
                        }),
                    )
                    .await?;
                Ok(())
            }
            Err(err) => {
                peer.close().await?;
                Err(AppError::InternalServerError(err))
            }
        }
    }

    async fn new_publish_peer(&self, media_info: MediaInfo) -> Result<(Arc<dyn PeerConnection>, Arc<Notify>)> {
        self.internal.new_publish_peer(media_info, Arc::downgrade(&self.internal)).await
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

    #[cfg(feature = "recorder")]
    pub async fn first_video_codec(&self) -> Option<String> {
        self.internal.first_publish_video_codec().await
    }

    #[cfg(feature = "recorder")]
    pub async fn first_audio_track_info(&self) -> Option<AudioTrackInfo> {
        let tracks = self.internal.publish_tracks.read().await;
        for track in tracks.iter() {
            if let track::PublishTrackRemote::Real { track, .. } = track {
                let kind = track.kind().await;
                if kind == RtpCodecKind::Audio {
                    let ssrcs = track.ssrcs().await;
                    let first_ssrc = ssrcs.first().copied().unwrap_or(0);
                    if let Some(params) = track.codec(first_ssrc).await {
                        return Some(AudioTrackInfo {
                            clock_rate: params.clock_rate,
                            channels: params.channels,
                            codec_mime: params.mime_type.clone(),
                            fmtp: params.sdp_fmtp_line.clone(),
                        });
                    }
                }
            }
        }
        None
    }

    #[cfg(feature = "recorder")]
    pub fn subscribe_tracks_change(&self) -> tokio::sync::broadcast::Receiver<()> {
        self.internal.subscribe_publish_tracks_change()
    }

    #[cfg(feature = "recorder")]
    pub async fn first_video_track(&self) -> Option<Arc<dyn webrtc::media_stream::track_remote::TrackRemote>> {
        self.internal.first_video_track().await
    }

    #[cfg(feature = "recorder")]
    pub async fn send_rtcp_to_publish(&self, message: rtcp::RtcpMessage, ssrc: u32) -> Result<()> {
        self.internal.send_rtcp_to_publish(message, ssrc).await
    }

    #[cfg(feature = "recorder")]
    pub async fn subscribe_audio_rtp(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<track::ForwardData>> {
        let tracks = self.internal.publish_tracks.read().await;
        for t in tracks.iter() {
            if t.kind() == RtpCodecKind::Audio {
                return Some(t.subscribe());
            }
        }
        None
    }
}

// subscribe
impl PeerForward {
    pub async fn add_subscribe(
        &self,
        offer: RTCSessionDescription,
    ) -> Result<(RTCSessionDescription, String)> {
        let media_info = MediaInfo::try_from(unmarshal_sdp(&offer.sdp)?)?;
        let (peer, gather_complete) = self.new_subscription_peer(media_info.clone()).await?;

        let (sdp, session) = (
            peer_complete(offer, peer.clone(), gather_complete).await?,
            get_peer_id(&peer),
        );

        let _ = self
            .internal
            .add_subscribe(peer.clone(), None, media_info)
            .await;

        Ok((sdp, session))
    }

    pub async fn subscribe_push(&self, dst: String, token: Option<String>) -> Result<()> {
        let media_info = MediaInfo {
            _codec: vec![],
            video_transceiver: (0, 1, false),
            audio_transceiver: (0, 1),
            has_data_channel: false,
        };

        let (peer, gather_complete) = self.new_subscription_peer(media_info.clone()).await?;

        let offer: RTCSessionDescription = peer.create_offer(None).await?;
        peer.set_local_description(offer).await?;
        gather_complete.notified().await;

        let description = peer
            .pending_local_description()
            .await
            .ok_or(AppError::throw("pending_local_description error"))?;

        let mut client = Client::new(
            dst.clone(),
            Client::get_authorization_header_map(token.clone()),
        );

        match client.wish(description.sdp.clone()).await {
            Ok((target_sdp, _)) => {
                self.internal
                    .add_subscribe(
                        peer.clone(),
                        Some(CascadeInfo {
                            source_url: None,
                            target_url: Some(dst.clone()),
                            token: token.clone(),
                            session_url: client.session_url,
                        }),
                        media_info,
                    )
                    .await?;
                let _ = peer.set_remote_description(target_sdp).await;
                Ok(())
            }
            Err(err) => {
                peer.close().await?;
                error!("cascade push dst: {}, err: {}", dst, err);
                Err(AppError::InternalServerError(err))
            }
        }
    }

    async fn new_subscription_peer(&self, media_info: MediaInfo) -> Result<(Arc<dyn PeerConnection>, Arc<Notify>)> {
        self.internal.new_subscription_peer(media_info, Arc::downgrade(&self.internal)).await
    }

    pub async fn select_layer(&self, session: String, layer: Option<Layer>) -> Result<()> {
        let rid = if let Some(layer) = layer {
            layer.encoding_id
        } else {
            self.internal.publish_svc_rids().await?[0].clone()
        };

        self.internal
            .select_kind_rid(session, RtpCodecKind::Video, rid)
            .await
    }

    pub async fn change_resource(
        &self,
        session: String,
        (kind, enabled): (String, bool),
    ) -> Result<()> {
        let codec_type = RtpCodecKind::from(kind.as_str());
        if codec_type == RtpCodecKind::Unspecified {
            return Err(AppError::throw("kind unspecified"));
        }

        let rid = if enabled {
            constant::RID_ENABLE.to_string()
        } else {
            constant::RID_DISABLE.to_string()
        };

        self.internal
            .select_kind_rid(session, codec_type, rid)
            .await
    }

    #[cfg(feature = "recorder")]
    pub async fn subscribe_video_rtp(
        &self,
    ) -> Option<tokio::sync::broadcast::Receiver<track::ForwardData>> {
        let tracks = self.internal.publish_tracks.read().await;
        for t in tracks.iter() {
            if t.kind() == RtpCodecKind::Video {
                return Some(t.subscribe());
            }
        }
        None
    }
}

async fn peer_complete(
    offer: RTCSessionDescription,
    peer: Arc<dyn PeerConnection>,
    gather_complete: Arc<Notify>,
) -> Result<RTCSessionDescription> {
    peer.set_remote_description(offer).await?;
    let answer = peer.create_answer(None).await?;
    peer.set_local_description(answer).await?;

    // Wait for ICE gathering to complete with a timeout.
    // If gathering never completes (e.g. STUN servers unreachable),
    // fall back to the current partial description after timeout.
    if tokio::time::timeout(
        std::time::Duration::from_secs(3),
        gather_complete.notified(),
    )
    .await
    .is_err()
    {
        tracing::warn!("ICE gathering timed out after 3s, using partial description");
    }

    let description = peer
        .local_description()
        .await
        .ok_or(anyhow::anyhow!("failed to get local description"))?;

    Ok(description)
}

fn unmarshal_sdp(sdp_str: &str) -> Result<sdp::SessionDescription> {
    let mut reader = Cursor::new(sdp_str);
    Ok(sdp::SessionDescription::unmarshal(&mut reader)?)
}

fn parse_ice_candidate(content: String) -> Result<Vec<RTCIceCandidateInit>> {
    let content = format!("v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n{content}");
    let mut reader = Cursor::new(content);
    let session_desc = sdp::SessionDescription::unmarshal(&mut reader)?;

    let mut ice_candidates = Vec::new();

    for media_descriptions in session_desc.media_descriptions {
        let attributes = media_descriptions.attributes;

        let mid = attributes
            .iter()
            .filter(|attr| attr.key == "mid")
            .map(|attr| attr.value.clone())
            .next_back();

        let mid = mid
            .ok_or_else(|| anyhow::anyhow!("no mid"))?
            .ok_or_else(|| anyhow::anyhow!("no mid"))?;

        let mline_index = mid.parse::<u16>()?;

        for attr in attributes {
            if attr.is_ice_candidate()
                && let Some(value) = attr.value
            {
                ice_candidates.push(RTCIceCandidateInit {
                    candidate: value,
                    sdp_mid: Some(mid.clone()),
                    sdp_mline_index: Some(mline_index),
                    username_fragment: None,
                    url: None,
                });
            }
        }
    }

    Ok(ice_candidates)
}

// Source feature extensions
impl PeerForward {
    #[cfg(feature = "source")]
    pub async fn add_virtual_track(
        &self,
        kind: RtpCodecKind,
        codec_params: rtc::rtp_transceiver::rtp_sender::RTCRtpCodecParameters,
    ) -> Result<()> {
        use crate::forward::track::{PublishTrackRemote, VirtualPublishTrack};

        let track = Arc::new(VirtualPublishTrack::new(
            self.stream.clone(),
            kind,
            codec_params,
        ));

        let mut publish_tracks = self.internal.publish_tracks.write().await;
        publish_tracks.push(PublishTrackRemote::Virtual(track));

        let _ = self.internal.publish_tracks_change.send(());

        debug!("[{}] Added virtual {:?} track", self.stream, kind);

        Ok(())
    }
    #[cfg(feature = "source")]
    pub async fn inject_video_rtp(&self, mut data: &[u8]) -> Result<()> {
        let packet = match Packet::unmarshal(&mut data) {
            Ok(p) => p,
            Err(e) => {
                error!(
                    "[{}] Failed to unmarshal video RTP packet: {}",
                    self.stream, e
                );
                return Err(anyhow::anyhow!("Unmarshal error: {}", e).into());
            }
        };

        trace!(
            "[{}] Injecting video RTP: SSRC={}, seq={}, ts={}, size={}",
            self.stream,
            packet.header.ssrc,
            packet.header.sequence_number,
            packet.header.timestamp,
            packet.payload.len()
        );

        let tracks = self.internal.publish_tracks.read().await;

        let video_track = tracks.iter().find(|t| t.kind() == RtpCodecKind::Video);

        match video_track {
            Some(track) => match track.inject_rtp(Arc::new(packet)) {
                Ok(_) => {
                    trace!("[{}] Video RTP injected successfully", self.stream);
                    Ok(())
                }
                Err(e) => {
                    error!("[{}] Failed to inject video RTP: {}", self.stream, e);
                    Err(anyhow::anyhow!("Inject failed: {}", e).into())
                }
            },
            None => {
                warn!("[{}] No video track found for injection", self.stream);
                Err(anyhow::anyhow!("No video track").into())
            }
        }
    }

    #[cfg(feature = "source")]
    pub async fn inject_audio_rtp(&self, mut data: &[u8]) -> Result<()> {
        let packet = match Packet::unmarshal(&mut data) {
            Ok(p) => p,
            Err(e) => {
                error!(
                    "[{}] Failed to unmarshal audio RTP packet: {}",
                    self.stream, e
                );
                return Err(anyhow::anyhow!("Unmarshal error: {}", e).into());
            }
        };

        trace!(
            "[{}] Injecting audio RTP: SSRC={}, seq={}, ts={}, size={}",
            self.stream,
            packet.header.ssrc,
            packet.header.sequence_number,
            packet.header.timestamp,
            packet.payload.len()
        );

        let tracks = self.internal.publish_tracks.read().await;

        let audio_track = tracks.iter().find(|t| t.kind() == RtpCodecKind::Audio);

        match audio_track {
            Some(track) => match track.inject_rtp(Arc::new(packet)) {
                Ok(_) => {
                    trace!("[{}] Audio RTP injected successfully", self.stream);
                    Ok(())
                }
                Err(e) => {
                    error!("[{}] Failed to inject audio RTP: {}", self.stream, e);
                    Err(anyhow::anyhow!("Inject failed: {}", e).into())
                }
            },
            None => {
                warn!("[{}] No audio track found for injection", self.stream);
                Err(anyhow::anyhow!("No audio track").into())
            }
        }
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
