#![cfg(feature = "rsmpeg")]

use std::time::Duration;

use livetwo::source::{FrameGenerator, FrameGeneratorConfig, MediaFrame, VideoCodec};
use livetwo::whipsynth::source::SourceFrame;
use livetwo::whipsynth::packetizer::{Packetizer, PacketizerConfig};
use webrtc::media_stream::Track;

#[test]
fn whipsynth_video_only_produces_rtp_packets() {
    let config = FrameGeneratorConfig {
        video_codec: VideoCodec::Vp8,
        audio_codec: None,
        width: 320,
        height: 240,
        fps: 15,
        duration: Some(Duration::from_millis(200)),
    };

    let mut generator = FrameGenerator::new(&config).expect("failed to create generator");
    let mut packetizer_config = PacketizerConfig::new(VideoCodec::Vp8, None);
    // Use fixed SSRC/sequence for deterministic tests.
    packetizer_config.video_ssrc = 0x12345678;
    packetizer_config.video_sequence_start = 1000;
    let mut packetizer = Packetizer::new(&packetizer_config).expect("failed to create packetizer");

    let mut total_packets = 0;
    loop {
        match generator.next_frame().expect("generator error") {
            SourceFrame::Frame(frame) => {
                let packets = match frame {
                    MediaFrame::Video(frame) => {
                        packetizer.packetize_video(&frame).expect("packetize error")
                    }
                    MediaFrame::Audio(_) => continue,
                };
                total_packets += packets.len();
                assert!(
                    !packets.is_empty(),
                    "expected RTP packets for a video frame"
                );
                for packet in &packets {
                    assert_eq!(packet.header.ssrc, 0x12345678);
                    assert!(packet.header.sequence_number >= 1000);
                    assert!(!packet.payload.is_empty());
                }
            }
            SourceFrame::Empty => continue,
            SourceFrame::End => break,
        }
    }

    assert!(total_packets > 0, "expected video RTP packets");
}

#[test]
fn whipsynth_h265_produces_rtp_packets() {
    let config = FrameGeneratorConfig {
        video_codec: VideoCodec::H265,
        audio_codec: None,
        width: 320,
        height: 240,
        fps: 15,
        duration: Some(Duration::from_millis(200)),
    };

    let mut generator = FrameGenerator::new(&config).expect("failed to create generator");
    let packetizer_config = PacketizerConfig::new(VideoCodec::H265, None);
    let mut packetizer = Packetizer::new(&packetizer_config).expect("failed to create packetizer");

    let mut total_packets = 0;
    loop {
        match generator.next_frame().expect("generator error") {
            SourceFrame::Frame(frame) => {
                let packets = match frame {
                    MediaFrame::Video(frame) => {
                        packetizer.packetize_video(&frame).expect("packetize error")
                    }
                    MediaFrame::Audio(_) => continue,
                };
                total_packets += packets.len();
            }
            SourceFrame::Empty => continue,
            SourceFrame::End => break,
        }
    }

    assert!(total_packets > 0, "expected H265 RTP packets");
}

#[test]
fn whipsynth_mixed_stream_produces_audio_and_video_packets() {
    let config = FrameGeneratorConfig {
        video_codec: VideoCodec::Vp8,
        audio_codec: Some(livetwo::source::AudioCodec::Opus),
        width: 320,
        height: 240,
        fps: 15,
        duration: Some(Duration::from_millis(200)),
    };

    let mut generator = FrameGenerator::new(&config).expect("failed to create generator");
    let packetizer_config =
        PacketizerConfig::new(VideoCodec::Vp8, Some(livetwo::source::AudioCodec::Opus));
    let mut packetizer = Packetizer::new(&packetizer_config).expect("failed to create packetizer");

    let mut video_packets = 0;
    let mut audio_packets = 0;
    loop {
        match generator.next_frame().expect("generator error") {
            SourceFrame::Frame(frame) => match frame {
                MediaFrame::Video(frame) => {
                    video_packets += packetizer
                        .packetize_video(&frame)
                        .expect("packetize error")
                        .len();
                }
                MediaFrame::Audio(frame) => {
                    audio_packets += packetizer
                        .packetize_audio(&frame)
                        .expect("packetize error")
                        .len();
                }
            },
            SourceFrame::Empty => continue,
            SourceFrame::End => break,
        }
    }

    assert!(video_packets > 0, "expected video RTP packets");
    assert!(audio_packets > 0, "expected audio RTP packets");
}

#[tokio::test]
async fn whipsynth_h265_track_includes_sprop_fmtp() {
    let mut packetizer_config = PacketizerConfig::new(VideoCodec::H265, None);
    packetizer_config.video_ssrc = 0x12345678;
    packetizer_config.h265_sprop =
        Some("sprop-vps=QAEMAf//AWAAAAMAsAAAAwAAAwBdoRAgQA==;sprop-sps=QgEBAWAAAAMAsAAAAwAAAwBdoAKAgC0WWVmkkyyAQAAAMACAAAAQD4I=;sprop-pps=RAHAcvBSAA==".to_owned());
    let packetizer = Packetizer::new(&packetizer_config).expect("failed to create packetizer");

    let track = packetizer
        .video_track("test-stream")
        .expect("video track required");
    let codec = track
        .codec(0x12345678)
        .await
        .expect("codec for SSRC should exist");

    assert_eq!(codec.mime_type, "video/H265");
    assert!(
        codec.sdp_fmtp_line.contains("sprop-vps="),
        "H265 fmtp line should contain sprop-vps: {}",
        codec.sdp_fmtp_line
    );
    assert!(
        codec.sdp_fmtp_line.contains("sprop-sps="),
        "H265 fmtp line should contain sprop-sps: {}",
        codec.sdp_fmtp_line
    );
    assert!(
        codec.sdp_fmtp_line.contains("sprop-pps="),
        "H265 fmtp line should contain sprop-pps: {}",
        codec.sdp_fmtp_line
    );
}
