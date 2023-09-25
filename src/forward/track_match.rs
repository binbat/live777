use std::sync::Arc;

use webrtc::{
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability, sdp::MediaDescription,
    track::track_remote::TrackRemote,
};

use crate::media;

pub fn track_sort(tracks: &mut [Arc<TrackRemote>]) {
    tracks.sort_by(|t1, t2| t1.rid().cmp(t2.rid()))
}

pub fn track_match(md: &MediaDescription, tracks: &[Arc<TrackRemote>]) -> Option<Arc<TrackRemote>> {
    if let Ok(codecs) = media::codecs_capability_from_media_description(md) {
        let mut tracks = track_match_codec(&codecs, tracks);
        track_sort(&mut tracks);
        tracks.first().cloned()
    } else {
        None
    }
}

pub fn track_match_codec(
    codecs: &[RTCRtpCodecCapability],
    tracks: &[Arc<TrackRemote>],
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
        })
        .cloned()
        .collect()
}
