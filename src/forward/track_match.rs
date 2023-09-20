use std::sync::Arc;

use rand::Rng;
use webrtc::{
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability, sdp::MediaDescription,
    track::track_remote::TrackRemote,
};

use crate::media;

pub fn track_match(
    md: &MediaDescription,
    tracks: &Vec<Arc<TrackRemote>>,
) -> Option<Arc<TrackRemote>> {
    if let Ok(codecs) = media::codecs_capability_from_media_description(md) {
        let mut tracks = track_match_codec(&codecs, tracks);
        if !tracks.is_empty() {
            // TODO The current strategy is just to randomly select a
            let mut rng = rand::thread_rng();
            return Some(tracks.remove(rng.gen_range(0..tracks.len())));
        }
    }
    None
}

fn track_match_codec(
    codecs: &Vec<RTCRtpCodecCapability>,
    tracks: &Vec<Arc<TrackRemote>>,
) -> Vec<Arc<TrackRemote>> {
    tracks
        .iter()
        .filter(|track| {
            let capability = track.codec().capability;
            for codec in codecs {
                if codec.mime_type.clone() == capability.mime_type
                    && codec.clock_rate == capability.clock_rate
                {
                    return true;
                }
            }
            false
        }).cloned()
        .collect()
}
