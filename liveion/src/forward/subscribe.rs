use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, info, warn};
use webrtc::peer_connection::{PeerConnection, RTCPeerConnectionState};
use webrtc::rtp_transceiver::RtpSender;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
use webrtc::media_stream::track_local::TrackLocal;
use rtc::rtp_transceiver::rtp_sender::{RtpCodecKind, RTCRtpEncodingParameters, RTCRtpCodingParameters};
use rtc::media_stream::MediaStreamTrack;

use crate::error::AppError;
use crate::forward::message::SessionInfo;
use crate::forward::rtcp::RtcpMessage;
use crate::new_broadcast_channel;
use crate::{constant, result::Result};

use super::get_peer_id;
use super::track::ForwardData;
use super::media::MediaInfo;
use super::message::CascadeInfo;
use super::track::PublishTrackRemote;

type SelectLayerBody = (RtpCodecKind, String);

struct SubscribeForwardChannel {
    publish_rtcp_sender: broadcast::Sender<(RtcpMessage, u32)>,
    select_layer_recv: broadcast::Receiver<SelectLayerBody>,
    publish_track_change: broadcast::Receiver<()>,
}

pub(crate) struct SubscribeRTCPeerConnection {
    pub(crate) id: String,
    pub(crate) cascade: Option<CascadeInfo>,
    pub(crate) peer: Arc<dyn PeerConnection>,
    pub(crate) create_at: i64,
    select_layer_sender: broadcast::Sender<SelectLayerBody>,
    pub(crate) media_info: MediaInfo,
}

impl SubscribeRTCPeerConnection {
    pub(crate) async fn new(
        cascade: Option<CascadeInfo>,
        stream: String,
        (peer, media_info): (Arc<dyn PeerConnection>, MediaInfo),
        publish_rtcp_sender: broadcast::Sender<(RtcpMessage, u32)>,
        (publish_tracks, publish_track_change): (
            Arc<RwLock<Vec<PublishTrackRemote>>>,
            broadcast::Sender<()>, // use subscribe
        ),
        (video_sender, audio_sender): (Option<Arc<dyn RtpSender>>, Option<Arc<dyn RtpSender>>),
    ) -> Self {
        let select_layer_sender = new_broadcast_channel!(1);
        let id = get_peer_id(&peer);
        let track_binding_publish_rid = Arc::new(RwLock::new(HashMap::new()));
        for (sender, kind) in [
            (video_sender, RtpCodecKind::Video),
            (audio_sender, RtpCodecKind::Audio),
        ] {
            if sender.is_none() {
                continue;
            }
            let sender = sender.unwrap();
            tokio::spawn(Self::sender_forward_rtp(
                stream.clone(),
                id.clone(),
                sender,
                kind,
                track_binding_publish_rid.clone(),
                publish_tracks.clone(),
                SubscribeForwardChannel {
                    publish_rtcp_sender: publish_rtcp_sender.clone(),
                    select_layer_recv: select_layer_sender.subscribe(),
                    publish_track_change: publish_track_change.subscribe(),
                },
            ));
        }
        let _ = publish_track_change.send(());
        Self {
            id,
            cascade,
            peer,
            create_at: Utc::now().timestamp_millis(),
            select_layer_sender,
            media_info,
        }
    }

    pub(crate) async fn info(&self) -> SessionInfo {
        SessionInfo {
            id: self.id.clone(),
            create_at: self.create_at,
            state: RTCPeerConnectionState::New, // Track state via handler
            cascade: self.cascade.clone(),
            has_data_channel: self.media_info.has_data_channel,
        }
    }

    /// Try to bind to an existing publish track. Returns (new_recv, new_track) if successful.
    async fn try_bind_publish_track(
        stream: &str,
        id: &str,
        sender: &Arc<dyn RtpSender>,
        kind: RtpCodecKind,
        sender_ssrc: u32,
        track_binding_publish_rid: &Arc<RwLock<HashMap<String, String>>>,
        publish_tracks: &Arc<RwLock<Vec<PublishTrackRemote>>>,
        forward_channel: &SubscribeForwardChannel,
        _virtual_sender: &broadcast::Sender<ForwardData>,
    ) -> Option<(broadcast::Receiver<ForwardData>, Arc<TrackLocalStaticRTP>)> {
        let mut track_binding_publish_rid = track_binding_publish_rid.write().await;
        let publish_tracks = publish_tracks.read().await;
        let current_rid = track_binding_publish_rid.get(&kind.to_string());

        if publish_tracks.is_empty() {
            return None;
        }

        if current_rid.is_some() && current_rid.cloned().unwrap() == constant::RID_DISABLE {
            return None;
        }

        for publish_track in publish_tracks.iter() {
            if publish_track.kind() != kind {
                continue;
            }

            let publisher_codec = match publish_track {
                PublishTrackRemote::Real { track, .. } => {
                    let ssrcs = track.ssrcs().await;
                    let first_ssrc = ssrcs.first().copied().unwrap_or(0);
                    track.codec(first_ssrc).await.unwrap_or_default()
                }
                #[cfg(feature = "source")]
                PublishTrackRemote::Virtual(v) => v.codec_params.rtp_codec.clone(),
            };

            let codec = if let Ok(params) = sender.get_parameters().await {
                if params.rtp_parameters.codecs.is_empty() {
                    publisher_codec
                } else {
                    let matched = params.rtp_parameters.codecs.iter()
                        .find(|c| c.rtp_codec.mime_type.to_lowercase() == publisher_codec.mime_type.to_lowercase())
                        .cloned();
                    match matched {
                        Some(c) => {
                            let mut updated_params = params.clone();
                            for encoding in updated_params.encodings.iter_mut() {
                                encoding.codec = c.rtp_codec.clone();
                            }
                            if let Err(e) = sender.set_parameters(updated_params, None).await {
                                debug!("[{}] [{}] {} failed to update encoding codec: {}", stream, id, kind, e);
                            }
                            c.rtp_codec
                        }
                        None => {
                            debug!("[{}] [{}] {} publisher codec {} not in send_codecs, using first available",
                                stream, id, kind, publisher_codec.mime_type);
                            params.rtp_parameters.codecs.iter()
                                .find(|c| {
                                    let mime = c.rtp_codec.mime_type.to_lowercase();
                                    match kind {
                                        RtpCodecKind::Video => mime.starts_with("video/"),
                                        RtpCodecKind::Audio => mime.starts_with("audio/"),
                                        _ => false,
                                    }
                                })
                                .map(|c| c.rtp_codec.clone())
                                .unwrap_or(publisher_codec)
                        }
                    }
                }
            } else {
                publisher_codec
            };

            let new_track = Arc::new(TrackLocalStaticRTP::new(
                MediaStreamTrack::new(
                    "webrtc".to_string(),
                    format!("{}-{}", "webrtc", kind),
                    "webrtc".to_string(),
                    kind,
                    vec![RTCRtpEncodingParameters {
                        rtp_coding_parameters: RTCRtpCodingParameters {
                            ssrc: Some(sender_ssrc),
                            ..Default::default()
                        },
                        codec,
                        ..Default::default()
                    }],
                ),
            ));

            match sender.replace_track(new_track.clone() as Arc<dyn webrtc::media_stream::track_local::TrackLocal>).await {
                Ok(_) => {
                    debug!("[{}] [{}] {} track replace ok", stream, id, kind);
                    let new_recv = publish_track.subscribe();

                    let ssrc = match publish_track {
                        PublishTrackRemote::Real { track, .. } => {
                            let ssrcs = track.ssrcs().await;
                            ssrcs.first().copied().unwrap_or(0)
                        }
                        #[cfg(feature = "source")]
                        PublishTrackRemote::Virtual(v) => v.ssrc(),
                    };

                    let _ = forward_channel.publish_rtcp_sender.send((
                        RtcpMessage::PictureLossIndication,
                        ssrc,
                    ));

                    track_binding_publish_rid.insert(kind.to_string(), publish_track.rid().to_string());
                    return Some((new_recv, new_track));
                }
                Err(e) => {
                    debug!("[{}] [{}] {} track replace err: {}", stream, id, kind, e);
                }
            }
            break;
        }
        None
    }

    async fn sender_forward_rtp(
        stream: String,
        id: String,
        sender: Arc<dyn RtpSender>,
        kind: RtpCodecKind,
        track_binding_publish_rid: Arc<RwLock<HashMap<String, String>>>,
        publish_tracks: Arc<RwLock<Vec<PublishTrackRemote>>>,
        mut forward_channel: SubscribeForwardChannel,
    ) {
        info!("[{}] [{}] {} up", stream, id, kind);

        let sender_ssrc = match sender.get_parameters().await {
            Ok(params) => params.encodings.first()
                .and_then(|e| e.rtp_coding_parameters.ssrc)
                .unwrap_or_else(|| rand::random::<u32>()),
            Err(_) => rand::random::<u32>(),
        };

        let mut pre_rid: Option<String> = None;
        let virtual_sender = new_broadcast_channel!(1);
        let mut recv = virtual_sender.subscribe();
        let mut track = None;
        let mut first_packet = true;

        // Check for existing publish tracks immediately at startup,
        // so we don't depend on a potentially-missed publish_track_change event.
        if let Some((new_recv, new_track)) = Self::try_bind_publish_track(
            &stream, &id, &sender, kind, sender_ssrc,
            &track_binding_publish_rid, &publish_tracks, &forward_channel, &virtual_sender,
        ).await {
            recv = new_recv;
            track = Some(new_track);
        }

        loop {
            tokio::select! {
                publish_change = forward_channel.publish_track_change.recv() => {
                    debug!("{} {} recv publish track_change", stream, id);

                    if publish_change.is_err() {
                        continue;
                    }

                    {
                        let mut rid_map = track_binding_publish_rid.write().await;
                        let pts = publish_tracks.read().await;
                        let current_rid = rid_map.get(&kind.to_string());

                        if pts.is_empty() {
                            debug!("{} {} publish track len 0 , probably offline", stream, id);
                            recv = virtual_sender.subscribe();
                            track = None;
                            pre_rid = None;

                            if current_rid.is_some() && current_rid.cloned().unwrap() != constant::RID_DISABLE {
                                rid_map.remove(&kind.to_string());
                            }
                            continue;
                        }

                        if track.is_some() {
                            continue;
                        }

                        if current_rid.is_some() && current_rid.cloned().unwrap() == constant::RID_DISABLE {
                            continue;
                        }
                    }

                    if let Some((new_recv, new_track)) = Self::try_bind_publish_track(
                        &stream, &id, &sender, kind, sender_ssrc,
                        &track_binding_publish_rid, &publish_tracks, &forward_channel, &virtual_sender,
                    ).await {
                        recv = new_recv;
                        track = Some(new_track);
                    }
                }

                rtp_result = recv.recv() => {
                    match rtp_result {
                        Ok(packet) => {
                            match track {
                                None => continue,
                                Some(ref track) => {
                                    let mut packet = packet.as_ref().clone();
                                    // Rewrite SSRC to match the sender's SSRC.
                                    // The rtc-layer write_rtp validates that packet.ssrc
                                    // is in sender.track().ssrcs(), so it must match.
                                    packet.header.ssrc = sender_ssrc;

                                    if let Err(err) = track.write_rtp(packet).await {
                                        warn!("[{}] [{}] {} track write err: {}", stream, id, kind, err);
                                        break;
                                    }
                                    if first_packet {
                                        info!("[{}] [{}] {} first RTP packet written successfully", stream, id, kind);
                                        first_packet = false;
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            debug!("[{}] [{}] {} rtp receiver err: {}", stream, id, kind, err);
                        }
                    }
                }

                select_layer_result = forward_channel.select_layer_recv.recv() => {
                    match select_layer_result {
                        Ok(select_layer_body) => {
                            if select_layer_body.0 != kind {
                                continue;
                            }

                            let select_rid = select_layer_body.1;
                            let mut track_binding_publish_rid = track_binding_publish_rid.write().await;
                            let publish_tracks = publish_tracks.read().await;
                            let current_rid = track_binding_publish_rid.get(&kind.to_string()).cloned();

                            if current_rid == Some(select_rid.clone()) {
                                continue;
                            }

                            let new_rid = match &current_rid {
                                None => select_rid.clone(),
                                Some(current_rid) => {
                                    if current_rid == constant::RID_DISABLE && select_rid == constant::RID_ENABLE {
                                        track_binding_publish_rid.remove(&kind.to_string());

                                        match &pre_rid {
                                            None => {
                                                let next_rid = publish_tracks
                                                    .iter()
                                                    .filter(|t| t.kind() == kind)
                                                    .map(|t| t.rid().to_string())
                                                    .next();

                                                if next_rid.is_none() {
                                                    continue;
                                                }
                                                next_rid.unwrap()
                                            }
                                            Some(pre_rid) => pre_rid.clone(),
                                        }
                                    } else {
                                        select_rid.clone()
                                    }
                                }
                            };

                            if new_rid == constant::RID_DISABLE {
                                if let Some(rid) = current_rid {
                                    recv = virtual_sender.subscribe();
                                    track = None;
                                    pre_rid = Some(rid);
                                }
                                track_binding_publish_rid.insert(kind.to_string(), new_rid);
                                continue;
                            }

                            for publish_track in publish_tracks.iter() {
                                if publish_track.kind() == kind
                                    && (publish_track.rid() == new_rid || new_rid == constant::RID_ENABLE)
                                {
                                    let publisher_codec = match publish_track {
                                        PublishTrackRemote::Real { track, .. } => {
                                            let ssrcs = track.ssrcs().await;
                                            let first_ssrc = ssrcs.first().copied().unwrap_or(0);
                                            track.codec(first_ssrc).await.unwrap_or_default()
                                        }
                                        #[cfg(feature = "source")]
                                        PublishTrackRemote::Virtual(v) => v.codec_params.rtp_codec.clone(),
                                    };
                                    let codec = if let Ok(params) = sender.get_parameters().await {
                                        if params.rtp_parameters.codecs.is_empty() {
                                            publisher_codec
                                        } else {
                                            let matched = params.rtp_parameters.codecs.iter()
                                                .find(|c| c.rtp_codec.mime_type.to_lowercase() == publisher_codec.mime_type.to_lowercase())
                                                .cloned();
                                            match matched {
                                                Some(c) => {
                                                    let mut updated_params = params.clone();
                                                    for encoding in updated_params.encodings.iter_mut() {
                                                        encoding.codec = c.rtp_codec.clone();
                                                    }
                                                    let _ = sender.set_parameters(updated_params, None).await;
                                                    c.rtp_codec
                                                }
                                                None => {
                                                    params.rtp_parameters.codecs.iter()
                                                        .find(|c| {
                                                            let mime = c.rtp_codec.mime_type.to_lowercase();
                                                            match kind {
                                                                RtpCodecKind::Video => mime.starts_with("video/"),
                                                                RtpCodecKind::Audio => mime.starts_with("audio/"),
                                                                _ => false,
                                                            }
                                                        })
                                                        .map(|c| c.rtp_codec.clone())
                                                        .unwrap_or(publisher_codec)
                                                }
                                            }
                                        }
                                    } else {
                                        publisher_codec
                                    };
                                    let new_track = Arc::new(TrackLocalStaticRTP::new(
                                        MediaStreamTrack::new(
                                            "webrtc".to_string(),
                                            format!("{}-{}", "webrtc", kind),
                                            "webrtc".to_string(),
                                            kind,
                                            vec![RTCRtpEncodingParameters {
                                                rtp_coding_parameters: RTCRtpCodingParameters {
                                                    ssrc: Some(sender_ssrc),
                                                    ..Default::default()
                                                },
                                                codec,
                                                ..Default::default()
                                            }],
                                        ),
                                    ));

                                    match sender.replace_track(new_track.clone() as Arc<dyn webrtc::media_stream::track_local::TrackLocal>).await {
                                        Ok(_) => {
                                            debug!("[{}] [{}] {} track replace ok", stream, id, kind);
                                            recv = publish_track.subscribe();
                                            track = Some(new_track);

                                            let ssrc = match publish_track {
                                                PublishTrackRemote::Real { track, .. } => {
                                                    let ssrcs = track.ssrcs().await;
                                                    ssrcs.first().copied().unwrap_or(0)
                                                }
                                                #[cfg(feature = "source")]
                                                PublishTrackRemote::Virtual(v) => v.ssrc(),
                                            };

                                            let _ = forward_channel
                                                .publish_rtcp_sender
                                                .send((RtcpMessage::PictureLossIndication, ssrc));

                                            track_binding_publish_rid.insert(kind.to_string(), new_rid.clone());
                                            info!("[{}] [{}] {} select layer to {}", stream, id, kind, new_rid);
                                        }
                                        Err(e) => {
                                            debug!("[{}] [{}] {} track replace err: {}", stream, id, kind, e);
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            debug!("select_layer_recv err : {:?}", e);
                            break;
                        }
                    }
                }
            }
        }

        info!("[{}] [{}] {} down", stream, id, kind);
    }

    pub(crate) fn select_kind_rid(&self, kind: RtpCodecKind, rid: String) -> Result<()> {
        if let Err(err) = self.select_layer_sender.send((kind, rid)) {
            Err(AppError::throw(format!("select layer send err: {err}")))
        } else {
            Ok(())
        }
    }

}
