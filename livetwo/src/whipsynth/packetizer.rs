use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use bytes::Bytes;
use rsmpeg::ffi;
use rtc::peer_connection::configuration::media_engine::*;
use rtc::rtp::{codec::*, header::Header, packet::Packet, packetizer::Payloader};
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use tracing::{debug, trace};
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;

use crate::payload::{RTP_OUTBOUND_MTU, payload_annex_b};
use crate::source::{AudioCodec, EncodedFrame, VideoCodec};

/// Maximum RTP payload size used by all payloaders. We reserve the 12-byte RTP
/// header so that the final on-wire RTP packet does not exceed `RTP_OUTBOUND_MTU`.
const MAX_RTP_PAYLOAD_SIZE: usize = RTP_OUTBOUND_MTU - 12;

/// Configuration for a [`Packetizer`].
#[derive(Debug, Clone)]
pub struct PacketizerConfig {
    pub video_codec: VideoCodec,
    pub audio_codec: Option<AudioCodec>,
    pub video_ssrc: u32,
    pub audio_ssrc: u32,
    pub video_sequence_start: u16,
    pub audio_sequence_start: u16,
    /// Video resolution and frame rate. Used to derive AV1 level-idx for the
    /// SDP fmtp line. Defaults are 1280x720 @ 30 fps.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// H265 sprop parameters (`sprop-vps=...;sprop-sps=...;sprop-pps=...`).
    /// If `None` and the video codec is H265, the packetizer will leave the
    /// H265 fmtp line empty.
    pub h265_sprop: Option<String>,
}

impl PacketizerConfig {
    pub fn new(video_codec: VideoCodec, audio_codec: Option<AudioCodec>) -> Self {
        Self {
            video_codec,
            audio_codec,
            video_ssrc: rand::random(),
            audio_ssrc: rand::random(),
            video_sequence_start: rand::random(),
            audio_sequence_start: rand::random(),
            width: 1280,
            height: 720,
            fps: 30,
            h265_sprop: None,
        }
    }
}

/// Packetizer turns encoded frames into RTP packets ready to be written to a
/// WebRTC track.
pub struct Packetizer {
    video: Option<TrackPacketizer>,
    audio: Option<TrackPacketizer>,
    width: u32,
    height: u32,
    fps: u32,
    h265_sprop: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum TrackKind {
    Video(VideoCodec),
    Audio(AudioCodec),
}

struct TrackPacketizer {
    kind: TrackKind,
    /// Payloader for RTP packetization. `None` for H265 video, which uses its
    /// own Annex-B payloading path (`payload_annex_b`) instead.
    payloader: Option<Box<dyn Payloader + Send>>,
    ssrc: u32,
    payload_type: u8,
    sequence_number: u16,
    clock_rate: u32,
    timestamp_offset: u32,
}

impl Packetizer {
    /// Create a packetizer from a config.
    pub fn new(config: &PacketizerConfig) -> Result<Self> {
        let video = Some(TrackPacketizer {
            kind: TrackKind::Video(config.video_codec),
            // H265 uses its own Annex-B payloading path; skip allocating the
            // unused HevcPayloader.
            payloader: (config.video_codec != VideoCodec::H265)
                .then(|| video_payloader(config.video_codec)),
            ssrc: config.video_ssrc,
            payload_type: config.video_codec.payload_type(),
            sequence_number: config.video_sequence_start,
            clock_rate: 90_000,
            timestamp_offset: rand::random(),
        });

        let audio = config.audio_codec.map(|audio_codec| TrackPacketizer {
            kind: TrackKind::Audio(audio_codec),
            payloader: Some(audio_payloader(audio_codec)),
            ssrc: config.audio_ssrc,
            payload_type: audio_codec.payload_type(),
            sequence_number: config.audio_sequence_start,
            clock_rate: audio_codec.rtp_clock_rate() as u32,
            timestamp_offset: rand::random(),
        });

        Ok(Self {
            video,
            audio,
            width: config.width,
            height: config.height,
            fps: config.fps,
            h265_sprop: config.h265_sprop.clone(),
        })
    }

    /// Create the video WebRTC track that matches this packetizer.
    pub fn video_track(&self, input_id: &str) -> Option<Arc<TrackLocalStaticRTP>> {
        self.video.as_ref().map(|track| {
            let TrackKind::Video(video_codec) = track.kind else {
                unreachable!();
            };
            let track_id = format!("{}-video", input_id);
            let media_track = rtc::media_stream::MediaStreamTrack::new(
                input_id.to_owned(),
                track_id.clone(),
                track_id,
                RtpCodecKind::Video,
                vec![RTCRtpEncodingParameters {
                    rtp_coding_parameters: RTCRtpCodingParameters {
                        ssrc: Some(track.ssrc),
                        ..Default::default()
                    },
                    codec: video_rtp_codec(
                        video_codec,
                        self.h265_sprop.as_deref(),
                        self.width,
                        self.height,
                        self.fps,
                    ),
                    ..Default::default()
                }],
            );
            Arc::new(TrackLocalStaticRTP::new(media_track))
        })
    }

    /// Create the audio WebRTC track that matches this packetizer.
    pub fn audio_track(&self, input_id: &str) -> Option<Arc<TrackLocalStaticRTP>> {
        self.audio.as_ref().map(|track| {
            let TrackKind::Audio(audio_codec) = track.kind else {
                unreachable!();
            };
            let track_id = format!("{}-audio", input_id);
            let media_track = rtc::media_stream::MediaStreamTrack::new(
                input_id.to_owned(),
                track_id.clone(),
                track_id,
                RtpCodecKind::Audio,
                vec![RTCRtpEncodingParameters {
                    rtp_coding_parameters: RTCRtpCodingParameters {
                        ssrc: Some(track.ssrc),
                        ..Default::default()
                    },
                    codec: audio_rtp_codec(audio_codec),
                    ..Default::default()
                }],
            );
            Arc::new(TrackLocalStaticRTP::new(media_track))
        })
    }

    /// Return the configured audio codec, if any.
    pub fn audio_codec(&self) -> Option<AudioCodec> {
        self.audio.as_ref().and_then(|track| match track.kind {
            TrackKind::Audio(codec) => Some(codec),
            _ => None,
        })
    }

    /// Update the video RTP payload type after SDP negotiation.
    pub fn set_video_payload_type(&mut self, payload_type: u8) {
        if let Some(track) = self.video.as_mut() {
            track.payload_type = payload_type;
        }
    }

    /// Update the audio RTP payload type after SDP negotiation.
    pub fn set_audio_payload_type(&mut self, payload_type: u8) {
        if let Some(track) = self.audio.as_mut() {
            track.payload_type = payload_type;
        }
    }

    /// Packetize a video frame into RTP packets.
    pub fn packetize_video(&mut self, frame: &EncodedFrame) -> Result<Vec<Packet>> {
        let track = self.video.as_mut().context("no video track")?;
        let TrackKind::Video(video_codec) = track.kind else {
            anyhow::bail!("video track missing");
        };

        let timestamp = pts_to_rtp_timestamp(frame.pts, frame.time_base, track.clock_rate)
            .wrapping_add(track.timestamp_offset);

        let payloads = if video_codec == VideoCodec::H265 {
            payload_annex_b(&frame.data, MAX_RTP_PAYLOAD_SIZE)
        } else {
            track
                .payloader
                .as_mut()
                .context("video payloader missing")?
                .payload(MAX_RTP_PAYLOAD_SIZE, &Bytes::copy_from_slice(&frame.data))
                .map_err(|e| anyhow::anyhow!("video payload error: {e}"))?
        };

        let length = payloads.len();
        let mut packets = Vec::with_capacity(length);
        for (i, payload) in payloads.into_iter().enumerate() {
            let mut header = base_header(track.ssrc, track.payload_type, track.sequence_number);
            header.timestamp = timestamp;
            header.marker = i == length - 1;
            track.sequence_number = track.sequence_number.wrapping_add(1);

            trace!(
                "video packet: seq={} ts={} marker={} len={}",
                header.sequence_number,
                header.timestamp,
                header.marker,
                payload.len()
            );

            packets.push(Packet { header, payload });
        }

        debug!(
            "packetized video frame {} into {} packets",
            frame.pts,
            packets.len()
        );
        Ok(packets)
    }

    /// Packetize an audio frame into RTP packets.
    pub fn packetize_audio(&mut self, frame: &EncodedFrame) -> Result<Vec<Packet>> {
        let track = self.audio.as_mut().context("no audio track")?;

        let timestamp = pts_to_rtp_timestamp(frame.pts, frame.time_base, track.clock_rate)
            .wrapping_add(track.timestamp_offset);

        let payloads = track
            .payloader
            .as_mut()
            .context("audio payloader missing")?
            .payload(MAX_RTP_PAYLOAD_SIZE, &Bytes::copy_from_slice(&frame.data))
            .map_err(|e| anyhow::anyhow!("audio payload error: {e}"))?;

        let length = payloads.len();
        let mut packets = Vec::with_capacity(length);
        for (i, payload) in payloads.into_iter().enumerate() {
            let mut header = base_header(track.ssrc, track.payload_type, track.sequence_number);
            header.timestamp = timestamp;
            header.marker = i == length - 1;
            track.sequence_number = track.sequence_number.wrapping_add(1);

            trace!(
                "audio packet: seq={} ts={} marker={} len={}",
                header.sequence_number,
                header.timestamp,
                header.marker,
                payload.len()
            );

            packets.push(Packet { header, payload });
        }

        Ok(packets)
    }
}

fn base_header(ssrc: u32, payload_type: u8, sequence_number: u16) -> Header {
    Header {
        version: 2,
        padding: false,
        extension: false,
        marker: false,
        payload_type,
        sequence_number,
        timestamp: 0,
        ssrc,
        csrc: vec![],
        ..Default::default()
    }
}

fn pts_to_rtp_timestamp(pts: i64, time_base: ffi::AVRational, clock_rate: u32) -> u32 {
    let den = time_base.den as i64;
    let num = time_base.num as i64;
    if den == 0 {
        return 0;
    }
    // Use i128 for the intermediate product so that large PTS values do not
    // saturate before the final division.
    let ts = (pts as i128)
        .saturating_mul(clock_rate as i128)
        .saturating_mul(num as i128)
        .checked_div(den as i128)
        .unwrap_or(0);
    ts as u32
}

fn video_payloader(codec: VideoCodec) -> Box<dyn Payloader + Send> {
    match codec {
        VideoCodec::Vp8 => Box::<vp8::Vp8Payloader>::default(),
        VideoCodec::Vp9 => Box::<vp9::Vp9Payloader>::default(),
        VideoCodec::H264 => Box::<h264::H264Payloader>::default(),
        // H265 uses its own Annex-B payloading path (payload_annex_b); the
        // Packetizer never calls video_payloader for H265.
        VideoCodec::H265 => unreachable!("H265 uses payload_annex_b, not the Payloader trait"),
        VideoCodec::Av1 => Box::<av1::Av1Payloader>::default(),
    }
}

fn audio_payloader(codec: AudioCodec) -> Box<dyn Payloader + Send> {
    match codec {
        AudioCodec::Opus => Box::<opus::OpusPayloader>::default(),
        AudioCodec::G722 => Box::<g7xx::G722Payloader>::default(),
    }
}

/// RTCP feedback parameters common to all video codecs.
///
/// Constructed once via `LazyLock` so that repeated calls to `video_rtp_codec`
/// (once per track) do not re-allocate the same four entries.
pub(crate) static VIDEO_RTCP_FEEDBACK: LazyLock<Vec<rtc::rtp_transceiver::rtp_sender::RTCPFeedback>> =
    LazyLock::new(|| {
        vec![
            rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
                typ: "goog-remb".to_owned(),
                parameter: "".to_owned(),
            },
            rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
                typ: "ccm".to_owned(),
                parameter: "fir".to_owned(),
            },
            rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
                typ: "nack".to_owned(),
                parameter: "".to_owned(),
            },
            rtc::rtp_transceiver::rtp_sender::RTCPFeedback {
                typ: "nack".to_owned(),
                parameter: "pli".to_owned(),
            },
        ]
    });

fn video_rtp_codec(
    codec: VideoCodec,
    h265_sprop: Option<&str>,
    width: u32,
    height: u32,
    fps: u32,
) -> RTCRtpCodec {
    let rtcp_feedback = VIDEO_RTCP_FEEDBACK.clone();
    match codec {
        VideoCodec::Vp8 => RTCRtpCodec {
            mime_type: MIME_TYPE_VP8.to_owned(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: "".to_owned(),
            rtcp_feedback,
        },
        VideoCodec::Vp9 => RTCRtpCodec {
            mime_type: MIME_TYPE_VP9.to_owned(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: "profile-id=0".to_owned(),
            rtcp_feedback,
        },
        VideoCodec::H264 => RTCRtpCodec {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f"
                .to_owned(),
            rtcp_feedback,
        },
        VideoCodec::H265 => RTCRtpCodec {
            mime_type: MIME_TYPE_HEVC.to_owned(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: h265_sprop.unwrap_or("").to_owned(),
            rtcp_feedback,
        },
        VideoCodec::Av1 => RTCRtpCodec {
            mime_type: MIME_TYPE_AV1.to_owned(),
            clock_rate: 90_000,
            channels: 0,
            sdp_fmtp_line: format!(
                "profile-id=0;level-idx={};tier=0",
                av1_level_idx(width, height, fps)
            ),
            rtcp_feedback,
        },
    }
}

/// Compute an AV1 level-idx that satisfies the frame size and display rate
/// constraints for the configured resolution and frame rate.
///
/// The AV1 RTP spec infers `level-idx=5` when the parameter is absent, which
/// corresponds to Level 3.1 (roughly 1280x720 @ 30 fps). Streams with a higher
/// resolution or frame rate must declare a higher level, otherwise browsers
/// may drop frames once the stream exceeds the inferred level.
pub(crate) fn av1_level_idx(width: u32, height: u32, fps: u32) -> u8 {
    // AV1 level limits taken from AV1 spec Annex A. Each entry is
    // (level_idx, max_frame_size, max_display_rate).
    // Tier is always 0 (Main tier) for WebRTC.
    let frame_size = (width as u64).saturating_mul(height as u64);
    let display_rate = frame_size.saturating_mul(fps.max(1) as u64);

    #[rustfmt::skip]
    const LEVELS: [(u8, u64, u64); 20] = [
        // level_idx, max_frame_size, max_display_rate
        (0,  101760,   4423680),
        (1,  230400,   8363520),
        (2,  230400,  14155776),
        (3,  345600,  23527680),
        (4,  345600,  35651584),
        (5,  921600,  53960704),
        (6,  921600,  82493440),
        (7,  2073600, 117440512),
        (8,  2073600, 165675008),
        (9,  2073600, 248446976),
        (10, 3686400, 366219008),
        (11, 3686400, 537055232),
        (12, 8294400, 778567680),
        (13, 8294400, 1167085568),
        (14, 8294400, 1620356096),
        (15, 8294400, 2227948800),
        (16, 33177600, 3121096704),
        (17, 33177600, 4456908800),
        (18, 33177600, 6235672576),
        (19, 33177600, 8912389120),
    ];

    for (idx, max_size, max_rate) in LEVELS {
        if frame_size <= max_size && display_rate <= max_rate {
            return idx;
        }
    }
    19
}

fn audio_rtp_codec(codec: AudioCodec) -> RTCRtpCodec {
    match codec {
        AudioCodec::Opus => RTCRtpCodec {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48_000,
            channels: 2,
            sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
            rtcp_feedback: vec![],
        },
        AudioCodec::G722 => RTCRtpCodec {
            mime_type: MIME_TYPE_G722.to_owned(),
            clock_rate: 8_000,
            channels: 1,
            sdp_fmtp_line: "".to_owned(),
            rtcp_feedback: vec![],
        },
    }
}

// H.265 Annex-B payloading is shared with livetwo::payload::h265::payload_annex_b.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{FrameGenerator, FrameGeneratorConfig, MediaFrame};
    use crate::whipsynth::source::SourceFrame;
    use rtc::rtp::codec::av1::Av1Depacketizer;
    use rtc::rtp::packetizer::Depacketizer;
    use webrtc::media_stream::Track;

    /// Verify that AV1 frames produced by the synthetic source can be
    /// packetized by our Packetizer and then depacketized back into a valid
    /// low-overhead bitstream. This catches both our own packetizer setup and
    /// the underlying rtc-rtp AV1 payloader/depacketizer pair.
    #[test]
    fn av1_packetizer_round_trip() {
        let config = FrameGeneratorConfig {
            video_codec: VideoCodec::Av1,
            audio_codec: None,
            width: 320,
            height: 240,
            fps: 30,
            duration: None,
        };

        let mut generator = FrameGenerator::new(&config).expect("create AV1 generator");
        let mut packetizer_config = PacketizerConfig::new(VideoCodec::Av1, None);
        packetizer_config.width = config.width;
        packetizer_config.height = config.height;
        packetizer_config.fps = config.fps;
        let mut packetizer = Packetizer::new(&packetizer_config).expect("create AV1 packetizer");

        // Collect a few real encoded AV1 frames.
        let mut encoded_frames = Vec::new();
        while encoded_frames.len() < 5 {
            match generator.next_frame().expect("generate frame") {
                SourceFrame::Frame(MediaFrame::Video(frame)) => encoded_frames.push(frame),
                SourceFrame::Frame(MediaFrame::Audio(_)) => unreachable!("no audio track"),
                SourceFrame::Empty => {}
                SourceFrame::End => panic!("generator ended unexpectedly"),
            }
        }

        for frame in encoded_frames {
            let packets = packetizer
                .packetize_video(&frame)
                .expect("packetize AV1 frame");
            assert!(
                !packets.is_empty(),
                "AV1 frame must produce at least one RTP packet"
            );

            let mut depacketizer = Av1Depacketizer::default();
            let mut depacketized = Vec::new();
            for packet in packets {
                assert_eq!(packet.header.payload_type, 41, "AV1 payload type mismatch");
                let out = depacketizer
                    .depacketize(&packet.payload)
                    .expect("depacketize AV1 RTP packet");
                depacketized.extend_from_slice(&out);
            }

            // The depacketizer output should be a non-empty low-overhead
            // bitstream (OBUs with size fields). At minimum it must start
            // with a valid OBU header.
            assert!(
                !depacketized.is_empty(),
                "depacketized AV1 bitstream is empty"
            );
            assert_eq!(
                depacketized[0] & 0x80,
                0,
                "forbidden bit set in first depacketized OBU header"
            );
        }
    }

    #[test]
    fn av1_level_idx_matches_common_resolutions() {
        assert_eq!(av1_level_idx(320, 240, 30), 0);
        assert_eq!(av1_level_idx(640, 360, 30), 1);
        assert_eq!(av1_level_idx(1280, 720, 30), 5);
        assert_eq!(av1_level_idx(1920, 1080, 30), 7);
        assert_eq!(av1_level_idx(1920, 1080, 60), 8);
        assert_eq!(av1_level_idx(3840, 2160, 30), 12);
        assert_eq!(av1_level_idx(3840, 2160, 60), 12);
    }

    #[test]
    fn av1_track_fmtp_includes_level_idx() {
        let mut config = PacketizerConfig::new(VideoCodec::Av1, None);
        config.width = 1920;
        config.height = 1080;
        config.fps = 30;
        config.video_ssrc = 0x12345678;
        let packetizer = Packetizer::new(&config).expect("create AV1 packetizer");

        let track = packetizer
            .video_track("test-stream")
            .expect("video track required");

        // `TrackLocalStaticRTP::codec` is async.
        let rt = tokio::runtime::Runtime::new().expect("create tokio runtime");
        let codec = rt
            .block_on(track.codec(0x12345678))
            .expect("codec for SSRC should exist");

        assert_eq!(codec.mime_type, "video/AV1");
        assert!(
            codec.sdp_fmtp_line.contains("profile-id=0"),
            "AV1 fmtp should contain profile-id=0: {}",
            codec.sdp_fmtp_line
        );
        assert!(
            codec.sdp_fmtp_line.contains("level-idx=7"),
            "AV1 fmtp should contain level-idx=7 for 1080p30: {}",
            codec.sdp_fmtp_line
        );
        assert!(
            codec.sdp_fmtp_line.contains("tier=0"),
            "AV1 fmtp should contain tier=0: {}",
            codec.sdp_fmtp_line
        );
    }
}
