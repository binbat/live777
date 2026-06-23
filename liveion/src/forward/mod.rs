use rtc::rtp_transceiver::rtp_sender::RtpCodecKind;
use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, broadcast};
#[cfg(any(feature = "source", feature = "cascade"))]
use tracing::error;
#[cfg(feature = "source")]
use tracing::{debug, trace, warn};
use webrtc::peer_connection::{
    PeerConnection, RTCIceCandidateInit, RTCIceServer, RTCPeerConnectionState,
    RTCSessionDescription,
};

#[cfg(feature = "cascade")]
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
#[cfg(feature = "cascade")]
use self::message::CascadeInfo;
use self::message::ForwardEvent;

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

#[cfg(feature = "recorder")]
#[derive(Clone, Debug)]
pub struct VideoTrackInfo {
    pub codec_mime: String,
    pub fmtp: String,
    pub payload_type: Option<u8>,
    pub ssrc: Option<u32>,
}

impl PeerForward {
    #[cfg(feature = "source")]
    pub fn new(
        stream: impl ToString,
        ice_server: Vec<RTCIceServer>,
        ice_udp_addrs: Vec<SocketAddr>,
        channel: Channel,
    ) -> Self {
        PeerForward {
            stream: stream.to_string(),
            publish_lock: Arc::new(Mutex::new(())),
            internal: Arc::new(PeerForwardInternal::new(
                stream,
                ice_server,
                ice_udp_addrs,
                channel,
            )),
        }
    }

    #[cfg(not(feature = "source"))]
    pub fn new(
        stream: impl ToString,
        ice_server: Vec<RTCIceServer>,
        ice_udp_addrs: Vec<SocketAddr>,
    ) -> Self {
        PeerForward {
            stream: stream.to_string(),
            publish_lock: Arc::new(Mutex::new(())),
            internal: Arc::new(PeerForwardInternal::new(stream, ice_server, ice_udp_addrs)),
        }
    }
    #[cfg(feature = "source")]
    pub(crate) async fn try_init_udp_channel(&self) -> Result<()> {
        self.internal.try_init_udp_channel().await
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

/// Parse the transport-wide-cc RTP header extension ID from an SDP.
/// Returns 0 if not found.
const TWCC_URI: &str = "http://www.ietf.org/id/draft-holmer-rmcat-transport-wide-cc-extensions-01";

fn parse_twcc_ext_id_from_sdp(sdp: &str) -> u8 {
    for line in sdp.lines() {
        let line = line.trim();
        if line.starts_with("a=extmap:") && line.contains(TWCC_URI) {
            // Format: a=extmap:<id> <URI>
            if let Some(id_part) = line.strip_prefix("a=extmap:")
                && let Some(id_str) = id_part.split_whitespace().next()
                && let Ok(id) = id_str.parse::<u8>()
            {
                return id;
            }
        }
    }
    0
}

// publish
impl PeerForward {
    pub async fn set_publish(
        &self,
        mut offer: RTCSessionDescription,
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

        offer.sdp = strip_unusable_remote_ice_candidates(&offer.sdp);
        let media_info = MediaInfo::try_from(unmarshal_sdp(&offer.sdp)?)?;
        let media_profile = media_info.profile();
        let generation_decision = self
            .internal
            .decide_publish_generation(&media_profile)
            .await;

        // Parse negotiated TWCC extmap ID from the publisher's SDP offer.
        // This ID is used by the inbound RTP probe to correctly identify the
        // transport-wide-cc header extension (instead of guessing).
        let twcc_ext_id = parse_twcc_ext_id_from_sdp(&offer.sdp);
        self.internal.set_twcc_ext_id(twcc_ext_id);

        let (peer, gather_complete, connection_state, readiness) =
            self.new_publish_peer(media_info).await?;

        let description = peer_complete(offer, peer.clone(), gather_complete).await?;

        self.internal
            .apply_publish_generation(&generation_decision, media_profile)
            .await?;

        self.internal
            .set_publish(peer.clone(), None, connection_state, readiness)
            .await?;

        let session = get_peer_id(&peer);

        Ok((description, session))
    }

    #[cfg(feature = "cascade")]
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

        let media_info = MediaInfo {
            _codec: vec![],
            video_transceiver: (1, 0, false),
            audio_transceiver: (1, 0),
            has_data_channel: false,
        };
        let media_profile = media_info.profile();
        let generation_decision = self
            .internal
            .decide_publish_generation(&media_profile)
            .await;

        let (peer, gather_complete, connection_state, readiness) =
            self.new_publish_peer(media_info).await?;

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
                    .apply_publish_generation(&generation_decision, media_profile)
                    .await?;
                self.internal
                    .set_publish(
                        peer.clone(),
                        Some(CascadeInfo {
                            source_url: Some(src),
                            target_url: None,
                            token,
                            session_url: client.session_url,
                        }),
                        connection_state,
                        readiness,
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

    async fn new_publish_peer(
        &self,
        media_info: MediaInfo,
    ) -> Result<(
        Arc<dyn PeerConnection>,
        Arc<Notify>,
        Arc<std::sync::RwLock<RTCPeerConnectionState>>,
        Arc<internal::PeerConnectionReadiness>,
    )> {
        self.internal
            .new_publish_peer(media_info, Arc::downgrade(&self.internal))
            .await
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
            match track {
                track::PublishTrackRemote::Real { track, .. } => {
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
                #[cfg(feature = "source")]
                track::PublishTrackRemote::Virtual(track) => {
                    if track.kind == RtpCodecKind::Audio {
                        return Some(AudioTrackInfo {
                            clock_rate: track.codec_params.rtp_codec.clock_rate,
                            channels: track.codec_params.rtp_codec.channels,
                            codec_mime: track.codec_params.rtp_codec.mime_type.clone(),
                            fmtp: track.codec_params.rtp_codec.sdp_fmtp_line.clone(),
                        });
                    }
                }
            }
        }
        None
    }

    #[cfg(feature = "recorder")]
    pub async fn first_video_track_info(&self) -> Option<VideoTrackInfo> {
        let tracks = self.internal.publish_tracks.read().await;
        for track in tracks.iter() {
            match track {
                track::PublishTrackRemote::Real { track, .. } => {
                    let kind = track.kind().await;
                    if kind == RtpCodecKind::Video {
                        let ssrcs = track.ssrcs().await;
                        let first_ssrc = ssrcs.first().copied().unwrap_or(0);
                        if let Some(params) = track.codec(first_ssrc).await {
                            return Some(VideoTrackInfo {
                                codec_mime: params.mime_type.clone(),
                                fmtp: params.sdp_fmtp_line.clone(),
                                payload_type: None,
                                ssrc: ssrcs.first().copied(),
                            });
                        }
                    }
                }
                #[cfg(feature = "source")]
                track::PublishTrackRemote::Virtual(track) => {
                    if track.kind == RtpCodecKind::Video {
                        return Some(VideoTrackInfo {
                            codec_mime: track.codec_params.rtp_codec.mime_type.clone(),
                            fmtp: track.codec_params.rtp_codec.sdp_fmtp_line.clone(),
                            payload_type: Some(track.codec_params.payload_type),
                            ssrc: None,
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
    pub async fn first_video_track(
        &self,
    ) -> Option<Arc<dyn webrtc::media_stream::track_remote::TrackRemote>> {
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
        mut offer: RTCSessionDescription,
    ) -> Result<(RTCSessionDescription, String)> {
        offer.sdp = strip_unusable_remote_ice_candidates(&offer.sdp);
        let media_info = MediaInfo::try_from(unmarshal_sdp(&offer.sdp)?)?;
        let (peer, gather_complete, connection_state, _readiness) =
            self.new_subscription_peer(media_info.clone()).await?;

        let (sdp, session) = (
            peer_complete(offer, peer.clone(), gather_complete).await?,
            get_peer_id(&peer),
        );

        let _ = self
            .internal
            .add_subscribe(peer.clone(), None, media_info, connection_state)
            .await;

        Ok((sdp, session))
    }

    #[cfg(feature = "cascade")]
    pub async fn subscribe_push(&self, dst: String, token: Option<String>) -> Result<()> {
        let media_info = MediaInfo {
            _codec: vec![],
            video_transceiver: (0, 1, false),
            audio_transceiver: (0, 1),
            has_data_channel: false,
        };

        let (peer, gather_complete, connection_state, _readiness) =
            self.new_subscription_peer(media_info.clone()).await?;

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
                        connection_state,
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

    async fn new_subscription_peer(
        &self,
        media_info: MediaInfo,
    ) -> Result<(
        Arc<dyn PeerConnection>,
        Arc<Notify>,
        Arc<std::sync::RwLock<RTCPeerConnectionState>>,
        Arc<internal::PeerConnectionReadiness>,
    )> {
        self.internal
            .new_subscription_peer(media_info, Arc::downgrade(&self.internal))
            .await
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

fn strip_unusable_remote_ice_candidates(sdp: &str) -> String {
    sdp.lines()
        .filter(|line| {
            let unusable = is_unusable_remote_ice_candidate_line(line);
            if unusable {
                tracing::warn!("Skipping unusable remote ICE candidate in SDP offer: {line}");
            }
            !unusable
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_unusable_remote_ice_candidate_line(line: &str) -> bool {
    let Some(candidate) = line
        .trim()
        .strip_prefix("a=candidate:")
        .or_else(|| line.trim().strip_prefix("candidate:"))
    else {
        return false;
    };

    let Some(addr) = candidate.split_whitespace().nth(4) else {
        return false;
    };

    addr.parse::<IpAddr>()
        .map(is_unusable_remote_candidate_ip)
        .unwrap_or(false)
}

fn is_unusable_remote_candidate_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_benchmarking_ipv4(ip),
        IpAddr::V6(_) => false,
    }
}

fn is_benchmarking_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    octets[0] == 198 && (octets[1] == 18 || octets[1] == 19)
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
                if is_unusable_remote_ice_candidate_line(&format!("candidate:{value}")) {
                    tracing::warn!("Skipping unusable remote ICE candidate: {value}");
                    continue;
                }
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
    use crate::forward::PeerForward;
    use crate::forward::parse_ice_candidate;
    use crate::forward::strip_unusable_remote_ice_candidates;
    use rtc::media_stream::MediaStreamTrack;
    use rtc::peer_connection::configuration::interceptor_registry::{
        configure_nack, configure_rtcp_reports, configure_simulcast_extension_headers,
        configure_twcc_sender_only,
    };
    use rtc::peer_connection::configuration::media_engine::MIME_TYPE_VP8;
    use rtc::rtp_transceiver::rtp_sender::{
        RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
    };
    use sdp::extmap::TRANSPORT_CC_URI;
    use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
    use webrtc::peer_connection::{
        MediaEngine, PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler,
        RTCConfigurationBuilder, Registry,
    };

    #[derive(Clone)]
    struct TestPeerHandler;

    #[async_trait::async_trait]
    impl PeerConnectionEventHandler for TestPeerHandler {}

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

    #[test]
    fn parse_ice_candidate_skips_benchmarking_fake_ip_candidates() -> crate::result::Result<()> {
        let body = "a=ice-ufrag:EsAw
a=ice-pwd:P2uYro0UCOQ4zxjKXaWCBui1
m=audio 9 RTP/AVP 0
a=mid:0
a=candidate:1 1 udp 2122260223 198.18.0.1 55964 typ host generation 0 ufrag EsAw network-id 1
a=candidate:2 1 udp 2122260223 192.0.2.1 61764 typ host generation 0 ufrag EsAw network-id 1
a=end-of-candidates";

        let candidates = parse_ice_candidate(body.to_owned())?;

        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].candidate.contains("192.0.2.1"));
        Ok(())
    }

    #[test]
    fn sdp_sanitizer_removes_benchmarking_fake_ip_candidates() {
        let sdp = "v=0
o=- 0 0 IN IP4 127.0.0.1
s=-
t=0 0
m=video 9 UDP/TLS/RTP/SAVPF 96
a=mid:0
a=candidate:1 1 udp 2122260223 198.18.0.1 55964 typ host generation 0 ufrag abc network-id 1
a=candidate:2 1 udp 2122260223 192.0.2.1 61764 typ host generation 0 ufrag abc network-id 1
a=end-of-candidates";

        let sanitized = strip_unusable_remote_ice_candidates(sdp);

        assert!(!sanitized.contains("198.18.0.1"));
        assert!(sanitized.contains("192.0.2.1"));
        assert!(sanitized.contains("a=end-of-candidates"));
    }

    #[tokio::test]
    async fn whip_publish_answer_advertises_bwe_feedback_contract() -> crate::result::Result<()> {
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs()?;
        let registry = Registry::new();
        let registry = configure_nack(registry, &mut media_engine);
        let registry = configure_rtcp_reports(registry);
        configure_simulcast_extension_headers(&mut media_engine)?;
        let registry = configure_twcc_sender_only(registry, &mut media_engine)?;

        let offer_peer: std::sync::Arc<dyn PeerConnection> = std::sync::Arc::new(
            PeerConnectionBuilder::<std::net::SocketAddr>::new()
                .with_media_engine(media_engine)
                .with_interceptor_registry(registry)
                .with_handler(std::sync::Arc::new(TestPeerHandler))
                .with_configuration(RTCConfigurationBuilder::new().build())
                .with_udp_addrs(vec!["127.0.0.1:0".parse().unwrap()])
                .build()
                .await?,
        );
        let media_track = MediaStreamTrack::new(
            "bwe-contract-test".to_owned(),
            "video".to_owned(),
            "video".to_owned(),
            RtpCodecKind::Video,
            vec![RTCRtpEncodingParameters {
                rtp_coding_parameters: RTCRtpCodingParameters {
                    ssrc: Some(1),
                    ..Default::default()
                },
                codec: RTCRtpCodec {
                    mime_type: MIME_TYPE_VP8.to_owned(),
                    clock_rate: 90000,
                    channels: 0,
                    sdp_fmtp_line: "".to_owned(),
                    rtcp_feedback: vec![],
                },
                ..Default::default()
            }],
        );
        offer_peer
            .add_track(std::sync::Arc::new(TrackLocalStaticRTP::new(media_track)))
            .await?;

        let offer = offer_peer.create_offer(None).await?;
        #[cfg(feature = "source")]
        let forward = PeerForward::new(
            "bwe-contract-test",
            vec![],
            api::webrtc::resolve_webrtc_ice_udp_addrs(Some(vec!["127.0.0.1:0".to_owned()])),
            crate::config::Channel::default(),
        );
        #[cfg(not(feature = "source"))]
        let forward = PeerForward::new(
            "bwe-contract-test",
            vec![],
            api::webrtc::resolve_webrtc_ice_udp_addrs(Some(vec!["127.0.0.1:0".to_owned()])),
        );
        let (answer, session) = forward.set_publish(offer).await?;

        let has_transport_cc_feedback = answer
            .sdp
            .lines()
            .any(|line| line.starts_with("a=rtcp-fb:") && line.contains(" transport-cc"));
        let has_transport_cc_extmap = answer
            .sdp
            .lines()
            .any(|line| line.starts_with("a=extmap:") && line.contains(TRANSPORT_CC_URI));
        let has_remb_fallback = answer.sdp.contains(" goog-remb");

        assert!(
            answer.sdp.contains(" 127.0.0.1 "),
            "expected loopback ICE candidate in liveion answer SDP:\n{}",
            answer.sdp
        );
        assert!(
            !answer.sdp.contains(" 0.0.0.0 "),
            "unspecified ICE candidate leaked into liveion answer SDP:\n{}",
            answer.sdp
        );
        assert!(
            (has_transport_cc_feedback && has_transport_cc_extmap) || has_remb_fallback,
            "WHIP answer must advertise transport-cc with TWCC extmap, or goog-remb fallback:\n{}",
            answer.sdp
        );

        forward.remove_peer(session).await?;
        offer_peer.close().await?;
        Ok(())
    }
}
