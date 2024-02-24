use std::sync::Arc;

use tokio::sync::broadcast;
use tracing::{debug, info};
use webrtc::rtp::packet::Packet;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::track::track_remote::TrackRemote;

pub(crate) type ForwardData = Arc<Packet>;

#[derive(Clone)]
pub(crate) struct PublishTrackRemote {
    pub(crate) rid: String,
    pub(crate) kind: RTPCodecType,
    pub(crate) track: Arc<TrackRemote>,
    rtp_broadcast: Arc<broadcast::Sender<ForwardData>>,
}

impl PublishTrackRemote {
    pub async fn new(path: String, id: String, track: Arc<TrackRemote>) -> Self {
        let (rtp_sender, mut rtp_recv) = broadcast::channel(128);
        tokio::spawn(async move { while rtp_recv.recv().await.is_ok() {} });
        let rid = track.rid().to_owned();
        let kind = track.kind();
        tokio::spawn(Self::track_forward(
            path,
            id,
            track.clone(),
            rtp_sender.clone(),
        ));
        Self {
            rid,
            kind,
            track,
            rtp_broadcast: Arc::new(rtp_sender),
        }
    }

    async fn track_forward(
        path: String,
        id: String,
        track: Arc<TrackRemote>,
        rtp_sender: broadcast::Sender<ForwardData>,
    ) {
        info!(
            "[{}] [{}] track : {:?} rid :{} ssrc: {} start forward",
            path,
            id,
            track.kind(),
            track.rid(),
            track.ssrc()
        );
        let mut b = vec![0u8; 1500];
        loop {
            match track.read(&mut b).await {
                Ok((rtp_packet, _)) => {
                    if let Err(err) = rtp_sender.send(Arc::new(rtp_packet)) {
                        debug!(
                            "[{}] [{}] track : {:?} {} rtp broadcast error : {}",
                            path,
                            id,
                            track.kind(),
                            track.rid(),
                            err
                        );
                        break;
                    }
                }
                Err(err) => {
                    debug!(
                        "[{}] [{}] track : {:?} {} read error : {}",
                        path,
                        id,
                        track.kind(),
                        track.rid(),
                        err
                    );
                    break;
                }
            }
        }
        info!(
            "[{}] [{}] track : {:?} rid :{} ssrc: {} stop forward",
            path,
            id,
            track.kind(),
            track.rid(),
            track.ssrc()
        );
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<ForwardData> {
        self.rtp_broadcast.subscribe()
    }
}
