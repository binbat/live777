use super::{CodecAdapter, TrackKind};
use crate::recorder::fmp4::nalu_to_avcc;
use anyhow::{Result, anyhow};
use bytes::Bytes;
use bytes::BytesMut;
use webrtc::rtp::packet::Packet;
use webrtc::rtp::{codecs::h264::H264Packet, packetizer::Depacketizer};

/// Simple H264 Annex-B parser that splits a frame into NALUs and converts to AVCC.
pub struct H264Adapter {
    timescale: u32,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    codec_string: Option<String>,
    width: u32,
    height: u32,
}

impl Default for H264Adapter {
    fn default() -> Self {
        Self::new()
    }
}

impl H264Adapter {
    pub fn new() -> Self {
        Self {
            timescale: 90_000,
            sps: None,
            pps: None,
            codec_string: None,
            width: 0,
            height: 0,
        }
    }

    /// Parse SPS bytes to calculate codec string
    fn update_codec_string(&mut self) {
        if self.codec_string.is_some() {
            return;
        }
        if let Some(ref sps) = self.sps {
            if sps.len() >= 4 {
                let profile_idc = sps[1];
                let constraints = sps[2];
                let level_idc = sps[3];
                self.codec_string = Some(format!(
                    "avc1.{profile_idc:02x}{constraints:02x}{level_idc:02x}"
                ));
            } else {
                self.codec_string = Some("avc1".to_string());
            }
        }
    }

    fn parse_dimensions(&mut self, sps_bytes: &[u8]) {
        use h264_reader::{
            nal::sps::SeqParameterSet,
            rbsp::{BitReader, decode_nal},
        };

        if let Ok(rbsp) = decode_nal(sps_bytes)
            && let Ok(sps) = SeqParameterSet::from_bits(BitReader::new(&rbsp[..]))
            && let Ok((w, h)) = sps.pixel_dimensions()
        {
            self.width = w;
            self.height = h;
        }
    }
}

impl CodecAdapter for H264Adapter {
    fn kind(&self) -> TrackKind {
        TrackKind::Video
    }

    fn timescale(&self) -> u32 {
        self.timescale
    }

    fn ready(&self) -> bool {
        self.sps.is_some() && self.pps.is_some()
    }

    fn convert_frame(&mut self, frame: &Bytes) -> (Vec<u8>, bool, bool) {
        let mut offset = 0usize;
        let mut avcc_payload = Vec::<u8>::new();
        let mut is_idr = false;
        let mut cfg_updated = false;

        let bytes = frame.as_ref();
        while offset + 3 < bytes.len() {
            // start code detection
            let (start_code_len, start_pos) = if bytes[offset..].starts_with(&[0, 0, 1]) {
                (3, offset)
            } else if bytes[offset..].starts_with(&[0, 0, 0, 1]) {
                (4, offset)
            } else {
                offset += 1;
                continue;
            };

            let mut next = start_pos + start_code_len;
            while next + 3 < bytes.len()
                && !bytes[next..].starts_with(&[0, 0, 1])
                && !bytes[next..].starts_with(&[0, 0, 0, 1])
            {
                next += 1;
            }
            if next + 3 >= bytes.len() {
                next = bytes.len();
            }

            let nalu = &bytes[start_pos..next];
            let header_idx = if nalu.starts_with(&[0, 0, 0, 1]) {
                4
            } else {
                3
            };
            if nalu.len() <= header_idx {
                offset = next;
                continue;
            }
            let nal_type = nalu[header_idx] & 0x1F;

            match nal_type {
                7 => {
                    if self.sps.is_none() {
                        self.sps = Some(nalu[header_idx..].to_vec());
                        self.parse_dimensions(&nalu[header_idx..]);
                        cfg_updated = true;
                    }
                }
                8 => {
                    if self.pps.is_none() {
                        self.pps = Some(nalu[header_idx..].to_vec());
                        cfg_updated = true;
                    }
                }
                5 => {
                    is_idr = true;
                }
                _ => {}
            }

            avcc_payload.extend_from_slice(&nalu_to_avcc(&Bytes::copy_from_slice(nalu)));
            offset = next;
        }

        if cfg_updated {
            self.update_codec_string();
        }

        (avcc_payload, is_idr, cfg_updated && self.ready())
    }

    fn codec_config(&self) -> Option<Vec<Vec<u8>>> {
        if self.ready() {
            Some(vec![self.sps.as_ref()?.clone(), self.pps.as_ref()?.clone()])
        } else {
            None
        }
    }

    fn codec_string(&self) -> Option<String> {
        self.codec_string.clone()
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }
}

/// Assemble WebRTC RTP (H264) packets into a complete Annex-B formatted frame.
pub struct H264RtpParser {
    depacketizer: H264Packet,
    buffer: BytesMut, // Current frame buffer (Annex-B with 0x00000001 start code)
    idr: bool,        // Whether the current frame contains an IDR
}

impl Default for H264RtpParser {
    fn default() -> Self {
        Self::new()
    }
}

impl H264RtpParser {
    pub fn new() -> Self {
        Self {
            depacketizer: H264Packet::default(),
            buffer: BytesMut::new(),
            idr: false,
        }
    }

    /// Push a RTP packet. If it returns `Some((frame, is_idr))` it means a frame has been assembled.
    pub fn push_packet(&mut self, pkt: &Packet) -> Result<Option<(BytesMut, bool)>> {
        // Fast-path IDR detection for single-NALU packets (payload header available)
        if let Some(&b0) = pkt.payload.first() {
            if (b0 & 0x1F) == 5 {
                self.idr = true;
            }
        }

        // Use webrtc-rs depacketizer to convert RTP payload into a complete NALU
        let nalu = self
            .depacketizer
            .depacketize(&pkt.payload)
            .map_err(|e| anyhow!(e))?;

        // Prepend Annex-B start code and append NALU to current frame buffer
        self.push_annexb_prefix();
        self.buffer.extend_from_slice(&nalu);

        // Detect IDR (NALU type 5) also from depacketized NALU
        if !nalu.is_empty() && (nalu[0] & 0x1F) == 5 {
            self.idr = true;
        }

        // Frame completes when marker bit is set
        if pkt.header.marker {
            let mut out = BytesMut::new();
            std::mem::swap(&mut out, &mut self.buffer);
            let idr = self.idr;
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
}

// Implement unified RTP parser trait
impl crate::recorder::codec::RtpParser for H264RtpParser {
    type Output = (BytesMut, bool);

    fn push_packet(&mut self, pkt: &Packet) -> Result<Option<Self::Output>> {
        H264RtpParser::push_packet(self, pkt)
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
        let res = parser.push_packet(&pkt).expect("Failed to push packet");
        assert!(res.is_some(), "Parser should output a frame on marker");
        let (frame, is_idr) = res.expect("Expected frame to be returned");
        assert!(is_idr, "Frame should be detected as IDR");
        // Frame must start with Annex-B start code
        assert!(frame.starts_with(&[0, 0, 0, 1]));
    }
}
