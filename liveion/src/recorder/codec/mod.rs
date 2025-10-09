pub mod av1;
pub mod h264;
pub mod opus;
pub mod vp8;
pub mod vp9;

use av1::Av1Adapter;
pub use av1::Av1RtpParser;
use h264::H264Adapter;
pub use h264::H264RtpParser;
pub use opus::OpusRtpParser;
use vp8::Vp8Adapter;
pub use vp8::Vp8RtpParser;
use vp9::Vp9Adapter;
pub use vp9::Vp9RtpParser;

use anyhow::Result;
use bytes::Bytes;
use webrtc::rtp::packet::Packet;

/// Track category for a given adapter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
}

/// Event emitted by the adapter while parsing a frame
pub enum CodecEvent {
    /// Codec config (SPS/PPS, vpcC, etc.) is complete, ready to write init segment
    ConfigUpdated,
    /// This frame is a keyframe
    KeyFrame,
    /// Regular frame, no special events
    None,
}

/// Unified codec adapter interface that all specific codec implementations must implement.
/// Design goals:
/// 1. Eliminate Segmenter/Fmp4Writer dependencies on specific codec details;
/// 2. Facilitate future support for HEVC/VP9/AV1 and other codecs;
/// 3. Maintain zero dependencies, pure Rust implementation.
pub trait CodecAdapter: Send + Sync {
    /// Track type (audio/video)
    fn kind(&self) -> TrackKind;

    /// Default timescale, H264 commonly uses 90_000.
    fn timescale(&self) -> u32;

    /// Whether initialization config has been collected (SPS/PPS, vpcC, etc.).
    fn ready(&self) -> bool;

    /// Convert a raw access unit frame to BMFF 4-byte length prefix format, returns:
    /// - Vec<u8>  converted payload
    /// - bool     whether this frame is a keyframe
    /// - bool     whether this parsing caused codec config to be complete (first time collecting complete SPS/PPS, etc.)
    fn convert_frame(&mut self, frame: &Bytes) -> (Vec<u8>, bool, bool);

    /// Return codec config blob (e.g., [sps, pps]).
    fn codec_config(&self) -> Option<Vec<Vec<u8>>>;

    /// Return codec string like "avc1.42E01E" or "vp08.00.41.08".
    fn codec_string(&self) -> Option<String>;

    /// Video width, if applicable
    fn width(&self) -> u32 {
        0
    }

    /// Video height, if applicable
    fn height(&self) -> u32 {
        0
    }
}

/// Supported video codecs for recorder ingestion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Vp8,
    Vp9,
    Av1,
}

/// Factory helper to create a concrete codec adapter by codec kind.
pub fn create_video_adapter(codec: VideoCodec) -> Box<dyn CodecAdapter> {
    match codec {
        VideoCodec::H264 => Box::new(H264Adapter::new()),
        VideoCodec::Vp8 => Box::new(Vp8Adapter::new()),
        VideoCodec::Vp9 => Box::new(Vp9Adapter::new()),
        VideoCodec::Av1 => Box::new(Av1Adapter::new()),
    }
}

/// Unified RTP parser trait so that different codecs (H264/Opus/…) share the same façade.
///
/// Associated type `Output` represents the parsed unit – for video it could be
/// `(BytesMut, bool)` (frame + is_idr), for audio `(BytesMut, u32)` (payload + timestamp).
/// The method returns `Ok(Some(x))` when a full unit is ready, or `Ok(None)` when the
/// parser needs more RTP packets.
pub trait RtpParser {
    type Output;
    fn push_packet(&mut self, pkt: &Packet) -> Result<Option<Self::Output>>;
}
