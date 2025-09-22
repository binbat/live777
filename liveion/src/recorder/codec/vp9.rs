use super::{CodecAdapter, TrackKind};
use anyhow::{Result, anyhow};
use bytes::{Bytes, BytesMut};
use webrtc::rtp::packet::Packet;
use webrtc::rtp::{codecs::vp9::Vp9Packet, packetizer::Depacketizer};

/// Minimal VP9 adapter. For fMP4 we will carry raw frame bytes into samples.
/// Keyframe detection is approximated by parsing the first byte of the VP9 frame header.
/// In VP9 Bitstream, a keyframe has frame_type=0 in the uncompressed header.
/// We avoid deep parsing; for robustness, default to non-key when uncertain.
pub struct Vp9Adapter {
    timescale: u32,
    width: u32,
    height: u32,
}

impl Default for Vp9Adapter {
    fn default() -> Self {
        Self::new()
    }
}

impl Vp9Adapter {
    pub fn new() -> Self {
        Self {
            timescale: 90_000,
            width: 0,
            height: 0,
        }
    }
}

impl CodecAdapter for Vp9Adapter {
    fn kind(&self) -> TrackKind {
        TrackKind::Video
    }
    fn timescale(&self) -> u32 {
        self.timescale
    }
    fn ready(&self) -> bool {
        true
    }
    fn convert_frame(&mut self, frame: &Bytes) -> (Vec<u8>, bool, bool) {
        let payload = frame.as_ref();
        // VP9 uncompressed header: frame_type bit is bit5 (0=key, 1=inter)
        let is_key = if !payload.is_empty() {
            ((payload[0] >> 5) & 0x01) == 0
        } else {
            false
        };
        (payload.to_vec(), is_key, false)
    }
    fn codec_config(&self) -> Option<Vec<Vec<u8>>> {
        Some(vec![])
    }
    fn codec_string(&self) -> Option<String> {
        Some("vp09.00.10.08".to_string())
    }
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
}

/// Assemble WebRTC RTP (VP9) packets into a complete VP9 frame.
pub struct Vp9RtpParser {
    depacketizer: Vp9Packet,
    buffer: BytesMut,
}

impl Default for Vp9RtpParser {
    fn default() -> Self {
        Self::new()
    }
}

impl Vp9RtpParser {
    pub fn new() -> Self {
        Self {
            depacketizer: Vp9Packet::default(),
            buffer: BytesMut::new(),
        }
    }

    pub fn push_packet(&mut self, pkt: &Packet) -> Result<Option<BytesMut>> {
        let payload = self
            .depacketizer
            .depacketize(&pkt.payload)
            .map_err(|e| anyhow!(e))?;
        self.buffer.extend_from_slice(&payload);
        if pkt.header.marker {
            let mut out = BytesMut::new();
            std::mem::swap(&mut out, &mut self.buffer);
            Ok(Some(out))
        } else {
            Ok(None)
        }
    }
}

impl crate::recorder::codec::RtpParser for Vp9RtpParser {
    type Output = BytesMut;
    fn push_packet(&mut self, pkt: &Packet) -> Result<Option<Self::Output>> {
        Vp9RtpParser::push_packet(self, pkt)
    }
}
