use std::{collections::HashMap, sync::Arc};

use webrtc::{
    api::media_engine::*, rtp_transceiver::rtp_codec::RTCRtpCodecCapability, sdp::MediaDescription,
    track::track_remote::TrackRemote,
};

use crate::media;

pub fn track_match(
    md: &MediaDescription,
    tracks: &[Arc<TrackRemote>],
    rids: Option<Vec<String>>,
) -> Option<Arc<TrackRemote>> {
    if let Ok(codecs) = media::codecs_capability_from_media_description(md) {
        let media_tracks = track_match_codec(&codecs, tracks);
        for media_type in codec_sort() {
            if let Some(tracks) = media_tracks.get(&media_type) {
                if let Some(rids) = &rids {
                    for rid in rids {
                        for track in tracks {
                            if track.rid() == rid {
                                return Some(track.clone());
                            }
                        }
                    }
                } else {
                    return Some(tracks.first().unwrap().clone());
                }
            }
        }
    }
    None
}

pub fn track_match_codec(
    codecs: &[RTCRtpCodecCapability],
    tracks: &[Arc<TrackRemote>],
) -> HashMap<String, Vec<Arc<TrackRemote>>> {
    let mut matched_tracks = HashMap::new();
    for track in tracks {
        let capability = track.codec().capability;
        for codec in codecs {
            if codec.mime_type.clone() == capability.mime_type
                && codec.clock_rate == capability.clock_rate
            {
                matched_tracks
                    .entry(codec.mime_type.clone())
                    .or_insert(Vec::new())
                    .push(track.clone());
            }
        }
    }
    matched_tracks
}

pub fn codec_sort() -> Vec<String> {
    vec![
        MIME_TYPE_AV1.to_string(),
        MIME_TYPE_VP8.to_string(),
        MIME_TYPE_H264.to_string(),
        MIME_TYPE_VP9.to_string(),
        MIME_TYPE_OPUS.to_string(),
        MIME_TYPE_G722.to_string(),
        MIME_TYPE_PCMU.to_string(),
        MIME_TYPE_PCMA.to_string(),
    ]
}
