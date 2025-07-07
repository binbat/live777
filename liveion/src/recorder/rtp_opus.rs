use anyhow::Result;
use bytes::BytesMut;
use webrtc::rtp::packet::Packet;

/// Very simple RTP parser for Opus.
/// For Opus over RTP we normally have exactly one complete Opus frame per RTP packet
/// (although the payload itself may aggregate multiple Opus frames). For recording
/// purposes we do not need to split them – we can treat the entire RTP payload as a
/// single sample for the MP4 track.
///
/// The parser therefore just forwards the payload without extra processing and
/// returns the RTP timestamp so that the caller can calculate the duration from
/// timestamp deltas.
#[derive(Default)]
pub struct OpusRtpParser;

impl OpusRtpParser {
    pub fn new() -> Self {
        Self {}
    }

    /// Push one RTP packet.
    ///
    /// Returns the raw payload as a BytesMut together with the original timestamp.
    pub fn push_packet(&mut self, pkt: Packet) -> Result<(BytesMut, u32)> {
        Ok((BytesMut::from(pkt.payload.as_ref()), pkt.header.timestamp))
    }
} 