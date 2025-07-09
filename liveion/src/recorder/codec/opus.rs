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

// Implement unified RTP parser trait (always returns Some because each RTP packet is a full Opus sample)
impl crate::recorder::codec::RtpParser for OpusRtpParser {
    type Output = (BytesMut, u32);

    fn push_packet(&mut self, pkt: Packet) -> Result<Option<Self::Output>> {
        // Re-use the existing pass-through logic
        OpusRtpParser::push_packet(self, pkt).map(|v| Some(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use webrtc::rtp::packet::Packet;

    #[test]
    fn test_opus_parser_pass_through() {
        let mut pkt = Packet::default();
        pkt.header.timestamp = 960;
        pkt.payload = Bytes::from_static(&[1, 2, 3, 4]);

        let mut parser = OpusRtpParser::new();
        let (out, ts) = parser.push_packet(pkt).unwrap();
        assert_eq!(ts, 960);
        assert_eq!(out.as_ref(), &[1, 2, 3, 4]);
    }
}
