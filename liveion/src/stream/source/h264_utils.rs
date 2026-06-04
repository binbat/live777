//! H.264 Annex-B parser and RTP packetizer utilities.
//! Optimized for near-zero-copy using the `bytes` crate.

use bytes::{Buf, Bytes, BytesMut};

/// NAL unit type enumeration for H.264.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NalType {
    Sps,
    Pps,
    Aud,
    Sei,
    Idr,
    Slice,
    Other(u8),
}

impl NalType {
    pub fn from_header(header_byte: u8) -> Self {
        match header_byte & 0x1F {
            1 => NalType::Slice,
            5 => NalType::Idr,
            6 => NalType::Sei,
            7 => NalType::Sps,
            8 => NalType::Pps,
            9 => NalType::Aud,
            other => NalType::Other(other),
        }
    }

    pub fn is_vcl(&self) -> bool {
        matches!(self, NalType::Slice | NalType::Idr)
    }
}

#[derive(Debug, Clone)]
pub struct NalUnit {
    pub nal_type: NalType,
    pub data: Bytes,
}

pub struct AnnexBParser {
    buffer: BytesMut,
}

impl AnnexBParser {
    pub fn new() -> Self {
        Self {
            buffer: BytesMut::with_capacity(256 * 1024),
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
    }

    pub fn extract_nals(&mut self) -> Vec<NalUnit> {
        let mut nals = Vec::new();

        loop {
            let buf = &self.buffer[..];
            if buf.len() < 3 {
                break;
            }

            // Find the first start code
            if let Some((start_pos, start_len)) = find_start_code(buf) {
                if start_pos > 0 {
                    self.buffer.advance(start_pos);
                    continue;
                }

                // Skip the start code we just found
                let search_buf = &self.buffer[start_len..];
                if let Some((next_pos, _next_len)) = find_start_code(search_buf) {
                    // Current NAL found!
                    self.buffer.advance(start_len);
                    let nal_data = self.buffer.split_to(next_pos).freeze();

                    if !nal_data.is_empty() {
                        let nal_type = NalType::from_header(nal_data[0]);
                        nals.push(NalUnit {
                            nal_type,
                            data: nal_data,
                        });
                    }
                    continue;
                } else {
                    break;
                }
            } else {
                if self.buffer.len() > 1024 * 1024 {
                    self.buffer.clear();
                }
                break;
            }
        }
        nals
    }
}

fn find_start_code(buf: &[u8]) -> Option<(usize, usize)> {
    if buf.len() < 3 {
        return None;
    }
    for i in 0..buf.len() - 2 {
        if buf[i] == 0 && buf[i + 1] == 0 {
            if buf[i + 2] == 1 {
                return Some((i, 3));
            }
            if i + 3 < buf.len() && buf[i + 2] == 0 && buf[i + 3] == 1 {
                return Some((i, 4));
            }
        }
    }
    None
}

impl Default for AnnexBParser {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct RtpHeader {
    pub version: u8,
    pub padding: bool,
    pub extension: bool,
    pub marker: bool,
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

impl RtpHeader {
    pub fn to_bytes(&self, buf: &mut [u8]) {
        buf[0] = (self.version << 6) | ((self.padding as u8) << 5) | ((self.extension as u8) << 4);
        buf[1] = ((self.marker as u8) << 7) | (self.payload_type & 0x7F);
        buf[2..4].copy_from_slice(&self.sequence.to_be_bytes());
        buf[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        buf[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
    }
}

#[derive(Debug, Clone)]
pub struct RtpPacket {
    pub header: RtpHeader,
    pub payload: Bytes,
}

impl RtpPacket {
    pub fn to_bytes(&self) -> Bytes {
        let mut b = BytesMut::with_capacity(12 + self.payload.len());
        b.extend_from_slice(&[0u8; 12]);
        self.header.to_bytes(&mut b[0..12]);
        b.extend_from_slice(&self.payload);
        b.freeze()
    }
}

pub struct H264Packetizer {
    mtu: usize,
    payload_type: u8,
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    clock_rate: u32,
    cached_sps: Option<Bytes>,
    cached_pps: Option<Bytes>,
    sps_pps_timestamp: u32,
}

impl H264Packetizer {
    pub fn new(mtu: usize, payload_type: u8, clock_rate: u32) -> Self {
        Self {
            mtu,
            payload_type,
            ssrc: rand::random(),
            sequence: rand::random(),
            timestamp: rand::random(),
            clock_rate,
            cached_sps: None,
            cached_pps: None,
            sps_pps_timestamp: 0,
        }
    }

    /// Helper for FFI: packetize raw bytes directly
    pub fn packetize_raw(&mut self, data: &Bytes) -> Vec<RtpPacket> {
        let nal_type = NalType::from_header(data[0]);
        let nal = NalUnit {
            nal_type,
            data: data.clone(),
        };
        self.packetize(&nal)
    }

    pub fn packetize(&mut self, nal: &NalUnit) -> Vec<RtpPacket> {
        let mut packets = Vec::new();
        match nal.nal_type {
            NalType::Sps => {
                self.cached_sps = Some(nal.data.clone());
                self.sps_pps_timestamp = self.timestamp;
            }
            NalType::Pps => {
                self.cached_pps = Some(nal.data.clone());
                self.sps_pps_timestamp = self.timestamp;
            }
            NalType::Idr => {
                if self.sps_pps_timestamp != self.timestamp {
                    if let Some(sps) = &self.cached_sps {
                        packets.push(self.create_single(sps.clone(), false));
                    }
                    if let Some(pps) = &self.cached_pps {
                        packets.push(self.create_single(pps.clone(), false));
                    }
                    self.sps_pps_timestamp = self.timestamp;
                }
            }
            _ => {}
        }

        if nal.data.len() <= self.mtu - 12 {
            packets.push(self.create_single(nal.data.clone(), nal.nal_type.is_vcl()));
        } else {
            packets.extend(self.create_fua(nal));
        }
        packets
    }

    fn create_single(&mut self, data: Bytes, marker: bool) -> RtpPacket {
        let p = RtpPacket {
            header: RtpHeader {
                version: 2,
                padding: false,
                extension: false,
                marker,
                payload_type: self.payload_type,
                sequence: self.sequence,
                timestamp: self.timestamp,
                ssrc: self.ssrc,
            },
            payload: data,
        };
        self.sequence = self.sequence.wrapping_add(1);
        p
    }

    fn create_fua(&mut self, nal: &NalUnit) -> Vec<RtpPacket> {
        let mut packets = Vec::new();
        let header = nal.data[0];
        let nri = header & 0xE0;
        let typ = header & 0x1F;
        let payload = nal.data.slice(1..);
        let max_size = self.mtu - 12 - 2;

        let mut offset = 0;
        while offset < payload.len() {
            let start = offset == 0;
            let size = std::cmp::min(max_size, payload.len() - offset);
            let end = offset + size == payload.len();

            let mut f_payload = BytesMut::with_capacity(2 + size);
            f_payload
                .extend_from_slice(&[nri | 28, ((start as u8) << 7) | ((end as u8) << 6) | typ]);
            f_payload.extend_from_slice(&payload.slice(offset..offset + size));

            packets.push(RtpPacket {
                header: RtpHeader {
                    version: 2,
                    padding: false,
                    extension: false,
                    marker: end && nal.nal_type.is_vcl(),
                    payload_type: self.payload_type,
                    sequence: self.sequence,
                    timestamp: self.timestamp,
                    ssrc: self.ssrc,
                },
                payload: f_payload.freeze(),
            });
            self.sequence = self.sequence.wrapping_add(1);
            offset += size;
        }
        packets
    }

    pub fn advance_timestamp(&mut self, increment: u32) {
        self.timestamp = self.timestamp.wrapping_add(increment);
    }

    pub fn cached_sps(&self) -> Option<Bytes> {
        self.cached_sps.clone()
    }
    pub fn cached_pps(&self) -> Option<Bytes> {
        self.cached_pps.clone()
    }

    pub fn get_sprop_parameter_sets(&self) -> Option<String> {
        use base64::prelude::*;
        if let (Some(sps), Some(pps)) = (&self.cached_sps, &self.cached_pps) {
            Some(format!(
                "{},{}",
                BASE64_STANDARD.encode(sps),
                BASE64_STANDARD.encode(pps)
            ))
        } else {
            None
        }
    }

    pub fn get_current_timestamp(&self) -> u32 {
        self.timestamp
    }
}

pub fn parse_profile_level_id(sps: &[u8]) -> Option<String> {
    if sps.len() >= 4 {
        Some(format!("{:02x}{:02x}{:02x}", sps[1], sps[2], sps[3]))
    } else {
        None
    }
}
