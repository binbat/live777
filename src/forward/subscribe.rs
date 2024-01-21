use std::collections::HashMap;
use std::sync::Arc;

use log::{debug, error};
use tokio::sync::{broadcast, mpsc, RwLock};
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::rtp_transceiver::rtp_sender::RTCRtpSender;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::{TrackLocal, TrackLocalWriter};

use crate::forward::rtcp::RtcpMessage;
use crate::forward::track::ForwardData;
use crate::AppResult;

use super::track::SubscribeTrackRemote;
use super::{get_peer_id, info};

pub(crate) struct SubscribeRTCPeerConnection {
    pub(crate) id: String,
    pub(crate) peer: Arc<RTCPeerConnection>,
    select_layer_sender: mpsc::Sender<String>,
}

#[derive(Clone)]
struct SubscribeTrackRemoteInfo {
    pub(crate) rid: String,
    pub(crate) kind: RTPCodecType,
    pub(crate) ssrc: u32,
}

impl SubscribeTrackRemoteInfo {
    pub(crate) fn new(subscribe_track: &SubscribeTrackRemote) -> Self {
        Self {
            rid: subscribe_track.rid.to_owned(),
            kind: subscribe_track.kind,
            ssrc: subscribe_track.track.ssrc(),
        }
    }
}

impl SubscribeRTCPeerConnection {
    pub(crate) async fn new(
        path: String,
        peer: Arc<RTCPeerConnection>,
        publish_rtcp_sender: mpsc::Sender<(RtcpMessage, u32)>,
        mut subscribe_tracks: Vec<SubscribeTrackRemote>,
        video_track: Option<Arc<TrackLocalStaticRTP>>,
        audio_track: Option<Arc<TrackLocalStaticRTP>>,
    ) -> AppResult<Self> {
        let (select_layer_sender, select_layer_recv) = mpsc::channel(1);
        let id = get_peer_id(&peer);
        let track_binding_publish_rid = Arc::new(RwLock::new(HashMap::new()));
        let subscribe_track_infos: Vec<SubscribeTrackRemoteInfo> = subscribe_tracks
            .iter()
            .map(SubscribeTrackRemoteInfo::new)
            .collect();
        let mut subscribe_track_audio = None;
        for (index, subscribe_track) in subscribe_tracks.iter().enumerate() {
            if subscribe_track.kind == RTPCodecType::Audio {
                subscribe_track_audio = Some(subscribe_tracks.remove(index));
                break;
            }
        }
        if let Some(track) = video_track {
            let sender = peer.add_track(track.clone()).await?;
            tokio::spawn(Self::track_read_rtcp(
                track.stream_id().to_owned(),
                RTPCodecType::Video,
                sender,
                subscribe_track_infos.clone(),
                track_binding_publish_rid.clone(),
                publish_rtcp_sender.clone(),
            ));
            tokio::spawn(Self::track_write_rtp_video(
                path.clone(),
                id.clone(),
                track,
                publish_rtcp_sender.clone(),
                select_layer_recv,
                track_binding_publish_rid.clone(),
                subscribe_tracks,
            ));
        }
        if let Some(track) = audio_track {
            if subscribe_track_audio.is_none() {
                return Err(anyhow::anyhow!("publish audio track is none").into());
            }
            let sender = peer.add_track(track.clone()).await?;
            tokio::spawn(Self::track_read_rtcp(
                track.stream_id().to_owned(),
                RTPCodecType::Video,
                sender,
                subscribe_track_infos.clone(),
                track_binding_publish_rid.clone(),
                publish_rtcp_sender.clone(),
            ));
            tokio::spawn(Self::track_write_rtp_audio(
                path.clone(),
                id.clone(),
                track,
                (subscribe_track_audio.unwrap().rtp_recv)()?,
            ));
        }

        Ok(Self {
            id,
            peer,
            select_layer_sender,
        })
    }

    async fn track_write_rtp_video(
        path: String,
        id: String,
        track: Arc<TrackLocalStaticRTP>,
        publish_rtcp_sender: mpsc::Sender<(RtcpMessage, u32)>,
        mut select_layer_recv: mpsc::Receiver<String>,
        track_binding_publish_rid: Arc<RwLock<HashMap<String, String>>>,
        mut subscribe_video_tracks: Vec<SubscribeTrackRemote>,
    ) {
        if subscribe_video_tracks.is_empty() {
            error!("[{}] [{}] subscribe video tracks is empty", path, id);
            return;
        }
        let subscribe_video_track = subscribe_video_tracks.first().unwrap();
        let mut recv = match (subscribe_video_track.rtp_recv)() {
            Ok(recv) => recv,
            Err(_err) => {
                return;
            }
        };
        info!("[{}] [{}] video up", path, id);
        let mut track_binding_publish_rid_one = track_binding_publish_rid.write().await;
        track_binding_publish_rid_one.insert(
            track.stream_id().to_owned(),
            subscribe_video_track.rid.clone(),
        );
        info!(
            "[{}] [{}] select layer to {}",
            path,
            track.stream_id(),
            subscribe_video_track.rid.clone()
        );
        drop(track_binding_publish_rid_one);
        let mut sequence_number: u16 = 0;
        loop {
            tokio::select! {
                rtp_result = recv.recv() => {
                    match rtp_result {
                        Ok(packet) => {
                            let mut packet = packet.as_ref().clone();
                            packet.header.sequence_number = sequence_number;
                            if let Err(err) = track.write_rtp(&packet).await {
                                debug!("[{}] [{}] video track write err: {}", path, id, err);
                                break;
                            }
                            sequence_number = sequence_number.wrapping_add(1);
                        }
                        Err(err) => {
                            debug!("[{}] [{}] video rtp receiver err: {}", path, id, err);
                            if err == broadcast::error::RecvError::Closed {
                              break;
                            }

                        }
                    }
                }
                select_layer_result = select_layer_recv.recv() => {
                    match select_layer_result {
                        Some(rid) => {
                            for  subscribe_track in subscribe_video_tracks.iter_mut() {
                                if subscribe_track.kind == RTPCodecType::Video && subscribe_track.rid == rid {
                                    recv= match (subscribe_track.rtp_recv)() {
                                        Ok(recv) => recv,
                                        Err(_err) => {
                                            return ;
                                        }
                                    };
                                    publish_rtcp_sender.send((RtcpMessage::PictureLossIndication, subscribe_track.track.ssrc())).await.unwrap();
                                    let mut track_binding_publish_rid = track_binding_publish_rid.write().await;
                                    track_binding_publish_rid.insert(track.stream_id().to_owned(), rid.clone());
                                    info!("[{}] [{}] select layer to {}", path, id, rid);
                                    break;
                                }
                            }
                        }
                        None => {
                            break ;
                        }
                    }
                }
            }
        }
        info!("[{}] [{}] video down", path, id);
    }

    async fn track_write_rtp_audio(
        path: String,
        id: String,
        track: Arc<TrackLocalStaticRTP>,
        mut rtp_receiver: broadcast::Receiver<ForwardData>,
    ) {
        info!("[{}] [{}] audio up", path, id);
        let mut sequence_number: u16 = 0;
        loop {
            match rtp_receiver.recv().await {
                Ok(packet) => {
                    let mut packet = packet.as_ref().clone();
                    packet.header.sequence_number = sequence_number;
                    if let Err(err) = track.write_rtp(&packet).await {
                        debug!("[{}] [{}] audio track write err: {}", path, id, err);
                        break;
                    }
                    sequence_number = sequence_number.wrapping_add(1);
                }
                Err(err) => {
                    debug!("[{}] [{}] audio rtp receiver err: {}", path, id, err);
                    break;
                }
            }
        }
        debug!("[{}] [{}] audio down", path, id);
    }

    pub(crate) fn select_layer(&self, rid: String) -> AppResult<()> {
        if let Err(err) = self.select_layer_sender.try_send(rid) {
            Err(anyhow::anyhow!("select layer send err: {}", err).into())
        } else {
            Ok(())
        }
    }

    async fn track_read_rtcp(
        stream_id: String,
        kind: RTPCodecType,
        sender: Arc<RTCRtpSender>,
        tracks: Vec<SubscribeTrackRemoteInfo>,
        track_binding_publish_rid: Arc<RwLock<HashMap<String, String>>>,
        publish_rtcp_sender: mpsc::Sender<(RtcpMessage, u32)>,
    ) {
        loop {
            match sender.read_rtcp().await {
                Ok((packets, _)) => {
                    let track_binding_publish_rid = track_binding_publish_rid.read().await;
                    let publish_rid = match track_binding_publish_rid.get(&stream_id) {
                        None => {
                            continue;
                        }
                        Some(rid) => rid,
                    };
                    for packet in packets {
                        if let Some(msg) = RtcpMessage::from_rtcp_packet(packet) {
                            for track in tracks.iter() {
                                if track.kind == kind && &track.rid == publish_rid {
                                    if let Err(_err) =
                                        publish_rtcp_sender.send((msg, track.ssrc)).await
                                    {
                                        return;
                                    }
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
