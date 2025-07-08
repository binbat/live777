use anyhow::{anyhow, Result};
use bytes::BytesMut;
use webrtc::rtp::packet::Packet;

/// Assemble WebRTC RTP (H264) packets into a complete Annex-B formatted frame.
/// Refer to xiu `whip2rtmp` implementation idea:
/// 1. Handle three common encapsulation types: single NALU, STAP-A, and FU-A.
/// 2. Output a frame when the marker bit is set, and return whether the frame is IDR.
pub struct H264RtpParser {
    buffer: BytesMut, // Current frame buffer (Annex-B with 0x00000001 start code)
    idr: bool,        // Whether the current frame contains an IDR
}

impl H264RtpParser {
    pub fn new() -> Self {
        Self {
            buffer: BytesMut::new(),
            idr: false,
        }
    }

    /// Push a RTP packet. If it returns `Some((frame, is_idr))` it means a frame has been assembled.
    pub fn push_packet(&mut self, pkt: Packet) -> Result<Option<(BytesMut, bool)>> {
        let payload = pkt.payload;
        if payload.is_empty() {
            return Ok(None);
        }
        let nal_type = payload[0] & 0x1F;
        match nal_type {
            1..=23 => {
                // Single NALU
                self.push_annexb_nalu(&payload);
                if nal_type == 5 {
                    self.idr = true;
                }
            }
            24 => {
                // STAP-A: skip one byte header, then (size | nalu)*
                let mut offset = 1;
                while offset + 2 <= payload.len() {
                    let size = ((payload[offset] as usize) << 8) | payload[offset + 1] as usize;
                    offset += 2;
                    if offset + size > payload.len() {
                        break; // malformed data
                    }
                    let nalu = &payload[offset..offset + size];
                    let nalu_type_inner = nalu[0] & 0x1F;
                    if nalu_type_inner == 5 {
                        self.idr = true;
                    }
                    self.push_annexb_nalu(nalu);
                    offset += size;
                }
            }
            28 => {
                // FU-A
                if payload.len() < 2 {
                    return Err(anyhow!("FU-A payload too short"));
                }
                let fu_header = payload[1];
                let start = (fu_header & 0x80) != 0;
                let end = (fu_header & 0x40) != 0;
                let nal_unit_type = fu_header & 0x1F;
                let nri = payload[0] & 0x60; // F|NRI from original header (F=0)
                if start {
                    // Rebuild NALU header and write start code
                    let reconstructed = nri | nal_unit_type;
                    self.push_annexb_prefix();
                    self.buffer.extend_from_slice(&[reconstructed]);
                    if nal_unit_type == 5 {
                        self.idr = true;
                    }
                    self.buffer.extend_from_slice(&payload[2..]);
                } else {
                    self.buffer.extend_from_slice(&payload[2..]);
                }
                if !end {
                    // Not finished yet
                }
            }
            _ => {
                // Other types not handled
            }
        }

        // Frame ends when the marker bit is set
        if pkt.header.marker {
            let mut out = BytesMut::new();
            std::mem::swap(&mut out, &mut self.buffer);
            let idr = self.idr;
            // Reset state
            self.idr = false;
            Ok(Some((out, idr)))
        } else {
            Ok(None)
        }
    }

    #[inline]
    fn push_annexb_prefix(&mut self) {
        self.buffer.extend_from_slice(&[0, 0, 0, 1]);
    }

    #[inline]
    fn push_annexb_nalu(&mut self, nalu: &[u8]) {
        self.push_annexb_prefix();
        self.buffer.extend_from_slice(nalu);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use webrtc::rtp::packet::Packet;

    #[test]
    fn test_single_nalu_idr_detection() {
        // Construct a minimal RTP packet carrying a single IDR NALU (type 5)
        let mut pkt = Packet::default();
        pkt.header.marker = true;
        pkt.header.timestamp = 0;
        pkt.payload = Bytes::from_static(&[0x65, 0xAA, 0xBB, 0xCC]); // 0x65 => nal_ref_idc=3, nal_type=5 (IDR)

        let mut parser = H264RtpParser::new();
        let res = parser.push_packet(pkt).unwrap();
        assert!(res.is_some(), "Parser should output a frame on marker");
        let (frame, is_idr) = res.unwrap();
        assert!(is_idr, "Frame should be detected as IDR");
        // Frame must start with Annex-B start code
        assert!(frame.starts_with(&[0, 0, 0, 1]));
    }
}
