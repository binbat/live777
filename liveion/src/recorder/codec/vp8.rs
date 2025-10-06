use super::{CodecAdapter, TrackKind};
use anyhow::{Result, anyhow};
use bytes::{Bytes, BytesMut};
use webrtc::rtp::packet::Packet;
use webrtc::rtp::{codecs::vp8::Vp8Packet, packetizer::Depacketizer};

/// VP8 adapter – a minimal adapter that forwards complete VP8 frame payloads
/// and detects keyframes using the first byte of the VP8 payload
/// (P bit and partition index). VP8 keyframe has P=0 and start of partition 0
/// contains an uncompressed header where the first 3 bits of the first byte are 0b000.
/// For our simplified detection, we rely on webrtc::rtp::vp8 depacketizer to output
/// a full frame payload and then classify keyframe by checking the first payload byte's
/// keyframe bit (bit 0 of first byte of payload after VP8 payload descriptor extension).
pub struct Vp8Adapter {
    timescale: u32,
    width: u32,
    height: u32,
}

impl Default for Vp8Adapter {
    fn default() -> Self {
        Self::new()
    }
}

impl Vp8Adapter {
    pub fn new() -> Self {
        Self {
            timescale: 90_000,
            width: 0,
            height: 0,
        }
    }
}

impl CodecAdapter for Vp8Adapter {
    fn kind(&self) -> TrackKind {
        TrackKind::Video
    }
    fn timescale(&self) -> u32 {
        self.timescale
    }
    fn ready(&self) -> bool {
        self.width > 0 && self.height > 0
    }
    fn convert_frame(&mut self, frame: &Bytes) -> (Vec<u8>, bool, bool) {
        let payload = frame.as_ref();
        let is_key = if !payload.is_empty() {
            // For VP8, keyframe detection in encoded frame: first byte LSB=0 indicates keyframe
            (payload[0] & 0x01) == 0
        } else {
            false
        };

        let mut cfg_updated = false;
        if is_key {
            cfg_updated = self.update_dimensions_from_keyframe(payload);
        }

        (payload.to_vec(), is_key, cfg_updated && self.ready())
    }
    fn codec_config(&self) -> Option<Vec<Vec<u8>>> {
        Some(vec![])
    }
    fn codec_string(&self) -> Option<String> {
        Some("vp08.00.10.08".to_string())
    }
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
}

impl Vp8Adapter {
    fn update_dimensions_from_keyframe(&mut self, frame: &[u8]) -> bool {
        let was_ready = self.ready();

        // VP8 keyframe starts with uncompressed data chunk header:
        // Start code bytes 0x9D 0x01 0x2A followed by 2 bytes width, 2 bytes height (little-endian),
        // with 14-bit values and 2-bit scaling fields (ignored here).
        let search_len = frame.len().min(64);
        let hay = &frame[..search_len];
        for i in 0..hay.len().saturating_sub(3) {
            if hay[i] == 0x9D && hay[i + 1] == 0x01 && hay[i + 2] == 0x2A {
                if i + 7 < hay.len() {
                    let w_raw = u16::from_le_bytes([hay[i + 3], hay[i + 4]]);
                    let h_raw = u16::from_le_bytes([hay[i + 5], hay[i + 6]]);
                    let width = (w_raw & 0x3FFF) as u32;
                    let height = (h_raw & 0x3FFF) as u32;
                    if width > 0 && height > 0 {
                        self.width = width;
                        self.height = height;
                        break;
                    }
                }
                break;
            }
        }

        !was_ready && self.ready()
    }
}

/// Assemble WebRTC RTP (VP8) packets into a complete VP8 frame.
pub struct Vp8RtpParser {
    depacketizer: Vp8Packet,
    buffer: BytesMut,
}

impl Default for Vp8RtpParser {
    fn default() -> Self {
        Self::new()
    }
}

impl Vp8RtpParser {
    pub fn new() -> Self {
        Self {
            depacketizer: Vp8Packet::default(),
            buffer: BytesMut::new(),
        }
    }

    /// Returns Some(BytesMut) when a full frame is reconstructed.
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

impl crate::recorder::codec::RtpParser for Vp8RtpParser {
    type Output = BytesMut;
    fn push_packet(&mut self, pkt: &Packet) -> Result<Option<Self::Output>> {
        Vp8RtpParser::push_packet(self, pkt)
    }
}
