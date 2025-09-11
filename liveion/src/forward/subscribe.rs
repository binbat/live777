use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, info};
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
use webrtc::track::track_local::TrackLocalWriter;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;

use crate::error::AppError;
use crate::forward::message::SessionInfo;
use crate::forward::rtcp::RtcpMessage;
use crate::new_broadcast_channel;
use crate::{constant, result::Result};

use super::get_peer_id;
use super::media::MediaInfo;
use super::message::CascadeInfo;
use super::track::PublishTrackRemote;

type SelectLayerBody = (RTPCodecType, String);

struct SubscribeForwardChannel {
    publish_rtcp_sender: broadcast::Sender<(RtcpMessage, u32)>,
    select_layer_recv: broadcast::Receiver<SelectLayerBody>,
    publish_track_change: broadcast::Receiver<()>,
}

pub(crate) struct SubscribeRTCPeerConnection {
    pub(crate) id: String,
    pub(crate) cascade: Option<CascadeInfo>,
    pub(crate) peer: Arc<RTCPeerConnection>,
    pub(crate) create_at: i64,
    select_layer_sender: broadcast::Sender<SelectLayerBody>,
    pub(crate) media_info: MediaInfo,
}

impl SubscribeRTCPeerConnection {
    pub(crate) async fn new(
        cascade: Option<CascadeInfo>,
        stream: String,
        (peer, media_info): (Arc<RTCPeerConnection>, MediaInfo),
        publish_rtcp_sender: broadcast::Sender<(RtcpMessage, u32)>,
        (publish_tracks, publish_track_change): (
            Arc<RwLock<Vec<PublishTrackRemote>>>,
            broadcast::Sender<()>, // use subscribe
        ),
        (video_sender, audio_sender): (Option<Arc<RTCRtpSender>>, Option<Arc<RTCRtpSender>>),
    ) -> Self {
        let select_layer_sender = new_broadcast_channel!(1);
        let id = get_peer_id(&peer);
        let track_binding_publish_rid = Arc::new(RwLock::new(HashMap::new()));
        for (sender, kind) in [
            (video_sender, RTPCodecType::Video),
            (audio_sender, RTPCodecType::Audio),
        ] {
            if sender.is_none() {
                continue;
            }
            let sender = sender.unwrap();
            tokio::spawn(Self::sender_forward_rtcp(
                kind,
                sender.clone(),
                publish_tracks.clone(),
                track_binding_publish_rid.clone(),
                publish_rtcp_sender.clone(),
            ));
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
            state: self.peer.connection_state(),
            cascade: self.cascade.clone(),
            has_data_channel: self.media_info.has_data_channel,
        }
    }

    async fn sender_forward_rtp(
        stream: String,
        id: String,
        sender: Arc<RTCRtpSender>,
        kind: RTPCodecType,
        track_binding_publish_rid: Arc<RwLock<HashMap<String, String>>>,
        publish_tracks: Arc<RwLock<Vec<PublishTrackRemote>>>,
        mut forward_channel: SubscribeForwardChannel,
    ) {
        info!("[{}] [{}] {} up", stream, id, kind);
        let mut pre_rid: Option<String> = None;
        // empty broadcast channel
        let virtual_sender = new_broadcast_channel!(1);
        let mut recv = virtual_sender.subscribe();
        let mut track = None;
        let mut sequence_number: u16 = 0;
        loop {
            tokio::select! {
                publish_change = forward_channel.publish_track_change.recv() =>{
                    debug!("{} {} recv publish track_change",stream,id);
                    if publish_change.is_err() {
                        continue;
                    }
                      let mut track_binding_publish_rid = track_binding_publish_rid.write().await;
                        let publish_tracks = publish_tracks.read().await;
                        let current_rid = track_binding_publish_rid.get(&kind.clone().to_string());
                        if publish_tracks.is_empty() {
                            debug!("{} {} publish track len 0 , probably offline",stream,id);
                            recv = virtual_sender.subscribe();
                            let _ = sender.replace_track(None).await;
                            track = None;
                            pre_rid = None;
                            if current_rid.is_some() && current_rid.cloned().unwrap() != constant::RID_DISABLE {
                                track_binding_publish_rid.remove(&kind.clone().to_string());
                            };
                            continue;
                        }
                        if track.is_some(){
                            continue;
                        }
                        if current_rid.is_some() && current_rid.cloned().unwrap() == constant::RID_DISABLE {
                           continue;
                        }
                        for publish_track in publish_tracks.iter() {
                              if publish_track.kind != kind {
                                continue;
                            }
                                    let new_track= Arc::new(
                                        TrackLocalStaticRTP::new(publish_track.track.clone().codec().capability,"webrtc".to_string(),format!("{}-{}","webrtc",kind))
                                    );
                                    match sender.replace_track(Some(new_track.clone())).await {
                                     Ok(_) => {
                                        debug!("[{}] [{}] {} track replace ok", stream, id,kind);
                                        recv = publish_track.subscribe();
                                        track = Some(new_track);
                                        let _ = forward_channel.publish_rtcp_sender.send((RtcpMessage::PictureLossIndication, publish_track.track.ssrc()));
                                        track_binding_publish_rid.insert(kind.clone().to_string(), publish_track.rid.clone());
                                    }
                                     Err(e) => {
                                        debug!("[{}] [{}] {} track replace err: {}", stream, id,kind, e);
                                    }};
                                     break;
                       }
                }
                rtp_result = recv.recv() => {
                    match rtp_result {
                        Ok(packet) => {
                            match track {
                                None => {
                                    continue;
                                }
                                Some(ref track) => {
                                    let mut packet = packet.as_ref().clone();
                                    packet.header.sequence_number = sequence_number;
                                    if let Err(err) = track.write_rtp(&packet).await {
                                        debug!("[{}] [{}] {} track write err: {}", stream, id,kind, err);
                                        break;
                                    }
                                    sequence_number = sequence_number.wrapping_add(1);
                                }
                            }
                        }
                        Err(err) => {
                            debug!("[{}] [{}] {} rtp receiver err: {}", stream, id, kind,err);
                        }
                    }
                }
                select_layer_result = forward_channel.select_layer_recv.recv() => {
                    match select_layer_result {
                        Ok(select_layer_body) => {
                            if select_layer_body.0 != kind {
                                continue;
                            };
                             let select_rid = select_layer_body.1;
                             let mut track_binding_publish_rid = track_binding_publish_rid.write().await;
                             let publish_tracks =  publish_tracks.read().await;
                             let current_rid = track_binding_publish_rid.get(&kind.to_string()).cloned();
                             if current_rid == Some(select_rid.clone()){
                                continue;
                             }
                            let new_rid = match &current_rid{
                                None => {
                                    select_rid.clone()
                                }
                                Some(current_rid) => {
                                    if current_rid == constant::RID_DISABLE && select_rid == constant::RID_ENABLE{
                                        track_binding_publish_rid.remove(&kind.clone().to_string());
                                        match &pre_rid{
                                            None => {
                                                let next_rid = publish_tracks.iter().filter(|t|t.kind==kind).map(|t|t.rid.clone()).next();
                                                if next_rid.is_none(){
                                                    continue;
                                                }
                                                next_rid.unwrap()
                                            }
                                            Some(pre_rid) => {
                                                pre_rid.clone()
                                            }
                                        }
                                    }else{
                                        select_rid.clone()
                                    }
                                }
                            };
                            if new_rid == constant::RID_DISABLE {
                                if current_rid.is_some(){
                                    recv = virtual_sender.subscribe();
                                    let _ = sender.replace_track(None).await;
                                    track = None;
                                    pre_rid = Some(current_rid.unwrap());
                                }
                                track_binding_publish_rid.insert(kind.clone().to_string(), new_rid);
                                continue;
                            };
                            for  publish_track in publish_tracks.iter() {
                                if publish_track.kind == RTPCodecType::Video && (publish_track.rid == new_rid || new_rid == constant::RID_ENABLE) {
                                      let new_track= Arc::new(
                                        TrackLocalStaticRTP::new(publish_track.track.clone().codec().capability,"webrtc".to_string(),format!("{}-{}","webrtc",kind))
                                    );
                                    match sender.replace_track(Some(new_track.clone())).await {
                                     Ok(_) => {
                                        debug!("[{}] [{}] {} track replace ok", stream, id,kind);
                                        recv = publish_track.subscribe();
                                        track = Some(new_track);
                                        let _ = forward_channel.publish_rtcp_sender.send((RtcpMessage::PictureLossIndication, publish_track.track.ssrc())).unwrap();
                                        track_binding_publish_rid.insert(kind.clone().to_string(), new_rid.clone());
                                        info!("[{}] [{}] {} select layer to {}", stream, id, kind,new_rid);
                                    }
                                     Err(e) => {
                                        debug!("[{}] [{}] {} track replace err: {}", stream, id,kind, e);
                                    }};
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            debug!("select_layer_recv err : {:?}",e);
                            break ;
                        }
                    }
                }
            }
        }
        info!("[{}] [{}] {} down", stream, id, kind);
    }

    pub(crate) fn select_kind_rid(&self, kind: RTPCodecType, rid: String) -> Result<()> {
        if let Err(err) = self.select_layer_sender.send((kind, rid)) {
            Err(AppError::throw(format!("select layer send err: {err}")))
        } else {
            Ok(())
        }
    }

    async fn sender_forward_rtcp(
        kind: RTPCodecType,
        sender: Arc<RTCRtpSender>,
        publish_tracks: Arc<RwLock<Vec<PublishTrackRemote>>>,
        track_binding_publish_rid: Arc<RwLock<HashMap<String, String>>>,
        publish_rtcp_sender: broadcast::Sender<(RtcpMessage, u32)>,
    ) {
        loop {
            match sender.read_rtcp().await {
                Ok((packets, _)) => {
                    let track_binding_publish_rid = track_binding_publish_rid.read().await;
                    let publish_rid = match track_binding_publish_rid.get(&kind.clone().to_string())
                    {
                        None => {
                            continue;
                        }
                        Some(rid) => rid,
                    };
                    for packet in packets {
                        if let Some(msg) = RtcpMessage::from_rtcp_packet(packet) {
                            let publish_tracks = publish_tracks.read().await;
                            for publish_track in publish_tracks.iter() {
                                if publish_track.kind == kind
                                    && &publish_track.rid == publish_rid
                                    && let Err(_err) =
                                        publish_rtcp_sender.send((msg, publish_track.track.ssrc()))
                                {
                                    return;
                                }
                            }
                        }
                    }
                }
                Err(_err) => {
                    return;
                }
            }
        }
    }
}
