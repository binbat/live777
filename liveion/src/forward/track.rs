use std::sync::Arc;

use tokio::sync::broadcast;
use tracing::{debug, info, trace};
use webrtc::rtp::packet::Packet;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::track::track_remote::TrackRemote;

use crate::new_broadcast_channel;

use super::message::Codec;

fn codec_string(params: webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecParameters) -> String {
    format!(
        "{}[{}],{}",
        params.capability.mime_type, params.payload_type, params.capability.sdp_fmtp_line,
    )
}

pub(crate) type ForwardData = Arc<Packet>;

#[derive(Clone)]
pub(crate) struct PublishTrackRemote {
    pub(crate) rid: String,
    pub(crate) kind: RTPCodecType,
    pub(crate) track: Arc<TrackRemote>,
    rtp_broadcast: Arc<broadcast::Sender<ForwardData>>,
}

impl PublishTrackRemote {
    pub async fn new(stream: String, id: String, track: Arc<TrackRemote>) -> Self {
        let rtp_sender = new_broadcast_channel!(128);
        let rid = track.rid().to_owned();
        let kind = track.kind();
        tokio::spawn(Self::track_forward(
            stream,
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
        stream: String,
        id: String,
        track: Arc<TrackRemote>,
        rtp_sender: broadcast::Sender<ForwardData>,
    ) {
        info!(
            "[{}] [{}] [track] kind: {:?}, rid: {}, ssrc: {}, codec: {} start forward",
            stream,
            id,
            track.kind(),
            track.rid(),
            track.ssrc(),
            codec_string(track.codec()),
        );
        trace!("codec: {:?}", track.codec());
        let mut b = vec![0u8; 1500];
        loop {
            match track.read(&mut b).await {
                Ok((rtp_packet, _)) => {
                    if let Err(err) = rtp_sender.send(Arc::new(rtp_packet)) {
                        debug!(
                            "[{}] [{}] [track] kind: {:?}, rid: {}, rtp broadcast error : {}",
                            stream,
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
                        "[{}] [{}] [track] kind: {:?}, {} read error : {}",
                        stream,
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
            "[{}] [{}] [track] kind: {:?}, rid :{}, ssrc: {} stop forward",
            stream,
            id,
            track.kind(),
            track.rid(),
            track.ssrc()
        );
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<ForwardData> {
        self.rtp_broadcast.subscribe()
    }

    pub(crate) fn codec(&self) -> Codec {
        let codec = self.track.codec();
        let media: Vec<String> = codec
            .capability
            .mime_type
            .clone()
            .to_lowercase()
            .split('/')
            .map(|s| s.to_string())
            .collect();
        Codec {
            kind: media.first().cloned().unwrap(),
            codec: media.get(1).cloned().unwrap(),
            fmtp: codec.capability.sdp_fmtp_line,
        }
    }
}
