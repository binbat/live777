use std::io::Cursor;

use super::{CodecAdapter, TrackKind};
use crate::recorder::fmp4::nalu_to_avcc;
use anyhow::{Result, anyhow};
use bytes::{Bytes, BytesMut};
use rtc_rtp::codec::h265::{H265Packet, H265Payload};
use rtc_rtp::packet::Packet;
use rtc_rtp::packetizer::Depacketizer;
use scuffle_h265::{
    ConstantFrameRate, HEVCDecoderConfigurationRecord, NALUnitType, NumTemporalLayers,
    ParallelismType, ProfileAdditionalFlags, SpsNALUnit,
};

/// HEVC adapter that collects VPS/SPS/PPS and converts Annex-B to length-prefixed samples.
pub struct H265Adapter {
    timescale: u32,
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    codec_string: Option<String>,
    width: u32,
    height: u32,
}

impl Default for H265Adapter {
    fn default() -> Self {
        Self::new()
    }
}

impl H265Adapter {
    pub fn new() -> Self {
        Self {
            timescale: 90_000,
            vps: None,
            sps: None,
            pps: None,
            codec_string: None,
            width: 0,
            height: 0,
        }
    }

    fn store_nalu(target: &mut Option<Vec<u8>>, data: &[u8]) -> bool {
        if target.as_deref() != Some(data) {
            *target = Some(data.to_vec());
            true
        } else {
            false
        }
    }

    /// Build an RFC-compliant HEVC codec string from parsed SPS data.
    ///
    /// Format: `hev1.<profile_space><profile_idc>.<compat_flags>.<tier><level>.<constraint_flags>`
    /// per ISO/IEC 14496-15 Annex E.
    fn build_codec_string(sps: &[u8]) -> Option<String> {
        let parsed = SpsNALUnit::parse(Cursor::new(sps)).ok()?;
        let p = &parsed.rbsp.profile_tier_level.general_profile;

        let profile_space = match p.profile_space {
            1 => "A",
            2 => "B",
            3 => "C",
            _ => "",
        };
        let compat = format!("{:08X}", p.profile_compatibility_flag.bits());
        let tier = if p.tier_flag { "H" } else { "L" };
        let level = p.level_idc.unwrap_or(0);

        // Reconstruct the 48-bit general_constraint_indicator_flags from parsed fields.
        // Bit 47 is the MSB (progressive_source_flag).
        let mut constraint: u64 = 0;
        if p.progressive_source_flag {
            constraint |= 1u64 << 47;
        }
        if p.interlaced_source_flag {
            constraint |= 1u64 << 46;
        }
        if p.non_packed_constraint_flag {
            constraint |= 1u64 << 45;
        }
        if p.frame_only_constraint_flag {
            constraint |= 1u64 << 44;
        }
        match &p.additional_flags {
            ProfileAdditionalFlags::Full {
                max_12bit_constraint_flag,
                max_10bit_constraint_flag,
                max_8bit_constraint_flag,
                max_422chroma_constraint_flag,
                max_420chroma_constraint_flag,
                max_monochrome_constraint_flag,
                intra_constraint_flag,
                one_picture_only_constraint_flag,
                lower_bit_rate_constraint_flag,
                max_14bit_constraint_flag,
            } => {
                if *max_12bit_constraint_flag {
                    constraint |= 1u64 << 43;
                }
                if *max_10bit_constraint_flag {
                    constraint |= 1u64 << 42;
                }
                if *max_8bit_constraint_flag {
                    constraint |= 1u64 << 41;
                }
                if *max_422chroma_constraint_flag {
                    constraint |= 1u64 << 40;
                }
                if *max_420chroma_constraint_flag {
                    constraint |= 1u64 << 39;
                }
                if *max_monochrome_constraint_flag {
                    constraint |= 1u64 << 38;
                }
                if *intra_constraint_flag {
                    constraint |= 1u64 << 37;
                }
                if *one_picture_only_constraint_flag {
                    constraint |= 1u64 << 36;
                }
                if *lower_bit_rate_constraint_flag {
                    constraint |= 1u64 << 35;
                }
                if matches!(max_14bit_constraint_flag, Some(true)) {
                    constraint |= 1u64 << 34;
                }
            }
            ProfileAdditionalFlags::Main10Profile {
                one_picture_only_constraint_flag,
            } => {
                if *one_picture_only_constraint_flag {
                    constraint |= 1u64 << 36;
                }
            }
            ProfileAdditionalFlags::None => {}
        }

        // Format as 12 uppercase hex digits (= 6 bytes), strip trailing zero bytes.
        let hex = format!("{:012X}", constraint);
        let trimmed = hex.trim_end_matches("00");
        let constraint_str = if trimmed.is_empty() { "00" } else { trimmed };

        Some(format!(
            "hev1.{}{}.{}.{}{}.{}",
            profile_space, p.profile_idc, compat, tier, level, constraint_str,
        ))
    }

    fn update_codec_info(&mut self) {
        if let Some(ref sps) = self.sps
            && let Ok(parsed) = SpsNALUnit::parse(Cursor::new(sps))
        {
            self.width = parsed.rbsp.cropped_width() as u32;
            self.height = parsed.rbsp.cropped_height() as u32;
            if self.codec_string.is_none() {
                self.codec_string =
                    Self::build_codec_string(sps).or_else(|| Some("hev1".to_string()));
            }
        }
    }

    fn build_hvcc(&self) -> Option<Vec<u8>> {
        let vps = self.vps.as_ref()?;
        let sps = self.sps.as_ref()?;
        let pps = self.pps.as_ref()?;

        let parsed = SpsNALUnit::parse(Cursor::new(sps)).ok()?;
        let profile = parsed.rbsp.profile_tier_level.general_profile.clone();
        let general_constraint_indicator_flags = if sps.len() >= 13 {
            ((sps[7] as u64) << 40)
                | ((sps[8] as u64) << 32)
                | ((sps[9] as u64) << 24)
                | ((sps[10] as u64) << 16)
                | ((sps[11] as u64) << 8)
                | (sps[12] as u64)
        } else {
            0
        };

        let config = HEVCDecoderConfigurationRecord {
            general_profile_space: profile.profile_space,
            general_tier_flag: profile.tier_flag,
            general_profile_idc: profile.profile_idc,
            general_profile_compatibility_flags: profile.profile_compatibility_flag,
            general_constraint_indicator_flags,
            general_level_idc: profile.level_idc.unwrap_or_default(),
            min_spatial_segmentation_idc: 0,
            parallelism_type: ParallelismType(0),
            chroma_format_idc: parsed.rbsp.chroma_format_idc,
            bit_depth_luma_minus8: parsed.rbsp.bit_depth_luma_minus8,
            bit_depth_chroma_minus8: parsed.rbsp.bit_depth_chroma_minus8,
            avg_frame_rate: 0,
            constant_frame_rate: ConstantFrameRate(0),
            num_temporal_layers: NumTemporalLayers(parsed.rbsp.sps_max_sub_layers_minus1 + 1),
            temporal_id_nested: parsed.rbsp.sps_temporal_id_nesting_flag,
            length_size_minus_one: 3,
            arrays: vec![
                scuffle_h265::NaluArray {
                    array_completeness: true,
                    nal_unit_type: NALUnitType::VpsNut,
                    nalus: vec![Bytes::copy_from_slice(vps)],
                },
                scuffle_h265::NaluArray {
                    array_completeness: true,
                    nal_unit_type: NALUnitType::SpsNut,
                    nalus: vec![Bytes::copy_from_slice(sps)],
                },
                scuffle_h265::NaluArray {
                    array_completeness: true,
                    nal_unit_type: NALUnitType::PpsNut,
                    nalus: vec![Bytes::copy_from_slice(pps)],
                },
            ],
        };

        let mut buf = Vec::new();
        if config.mux(&mut buf).is_ok() {
            Some(buf)
        } else {
            None
        }
    }
}

impl CodecAdapter for H265Adapter {
    fn kind(&self) -> TrackKind {
        TrackKind::Video
    }

    fn timescale(&self) -> u32 {
        self.timescale
    }

    fn ready(&self) -> bool {
        self.vps.is_some() && self.sps.is_some() && self.pps.is_some()
    }

    fn convert_frame(&mut self, frame: &Bytes) -> (Vec<u8>, bool, bool) {
        let mut offset = 0usize;
        let mut avcc_payload = Vec::new();
        let mut random_access = false;
        let mut cfg_updated = false;
        let bytes = frame.as_ref();

        while offset + 3 < bytes.len() {
            let (prefix_len, start_pos) = if bytes[offset..].starts_with(&[0, 0, 0, 1]) {
                (4, offset)
            } else if bytes[offset..].starts_with(&[0, 0, 1]) {
                (3, offset)
            } else {
                offset += 1;
                continue;
            };

            let mut next = start_pos + prefix_len;
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
            let header_index = if prefix_len == 4 { 4 } else { 3 };
            if nalu.len() <= header_index + 1 {
                offset = next;
                continue;
            }
            let body = &nalu[header_index..];
            let nal_type = (body[0] >> 1) & 0x3F;

            match nal_type {
                32 => {
                    if Self::store_nalu(&mut self.vps, body) {
                        cfg_updated = true;
                    }
                }
                33 => {
                    if Self::store_nalu(&mut self.sps, body) {
                        cfg_updated = true;
                    }
                }
                34 => {
                    if Self::store_nalu(&mut self.pps, body) {
                        cfg_updated = true;
                    }
                }
                16..=21 => {
                    random_access = true;
                }
                _ => {}
            }

            avcc_payload.extend_from_slice(&nalu_to_avcc(&Bytes::copy_from_slice(nalu)));
            offset = next;
        }

        if cfg_updated {
            self.update_codec_info();
        }

        (avcc_payload, random_access, cfg_updated && self.ready())
    }

    fn codec_config(&self) -> Option<Vec<Vec<u8>>> {
        if self.ready() {
            self.build_hvcc().map(|hvcc| vec![hvcc])
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

/// H265 RTP parser that outputs Annex-B frames with start codes.
pub struct H265RtpParser {
    depacketizer: H265Packet,
    buffer: BytesMut,
    keyframe: bool,
}

impl Default for H265RtpParser {
    fn default() -> Self {
        Self::new()
    }
}

impl H265RtpParser {
    pub fn new() -> Self {
        Self {
            depacketizer: H265Packet::default(),
            buffer: BytesMut::new(),
            keyframe: false,
        }
    }

    fn append_start_code(&mut self) {
        self.buffer.extend_from_slice(&[0, 0, 0, 1]);
    }

    fn append_nalu(&mut self, nalu: &[u8]) {
        self.append_start_code();
        self.buffer.extend_from_slice(nalu);
    }

    fn mark_keyframe(&mut self, nal_type: u8) {
        if (16..=21).contains(&nal_type) {
            self.keyframe = true;
        }
    }

    pub fn push_packet(&mut self, pkt: &Packet) -> Result<Option<(BytesMut, bool)>> {
        if pkt.payload.is_empty() {
            return Ok(None);
        }

        self.depacketizer
            .depacketize(&pkt.payload)
            .map_err(|e| anyhow!(e))?;

        match self.depacketizer.payload() {
            H265Payload::H265SingleNALUnitPacket(nal) => {
                let header = nal.payload_header();
                let mut nalu = Vec::with_capacity(2 + nal.payload().len());
                nalu.extend_from_slice(&header.0.to_be_bytes());
                let payload = nal.payload();
                nalu.extend_from_slice(payload.as_ref());
                self.mark_keyframe(header.nalu_type());
                self.append_nalu(&nalu);
            }
            H265Payload::H265AggregationPacket(packet) => {
                let mut nal_units = Vec::new();
                if let Some(first) = packet.first_unit() {
                    nal_units.push(first.nal_unit());
                }
                for unit in packet.other_units() {
                    nal_units.push(unit.nal_unit());
                }

                for data in nal_units {
                    if data.len() >= 2 {
                        self.mark_keyframe((data[0] >> 1) & 0x3F);
                    }
                    self.append_nalu(data.as_ref());
                }
            }
            H265Payload::H265FragmentationUnitPacket(fu) => {
                let (header, fu_header, payload) = {
                    let header = fu.payload_header();
                    let fu_header = fu.fu_header();
                    let payload = fu.payload();
                    (header, fu_header, payload)
                };

                if fu_header.s() {
                    self.append_start_code();
                    let mut reconstructed = header.0;
                    let clear_mask: u16 = !(0b0111_1110 << 8);
                    reconstructed &= clear_mask;
                    reconstructed |= ((fu_header.fu_type() as u16) & 0x3F) << (8 + 1);
                    self.buffer.extend_from_slice(&reconstructed.to_be_bytes());
                    self.buffer.extend_from_slice(payload.as_ref());
                    self.mark_keyframe(fu_header.fu_type());
                } else {
                    self.buffer.extend_from_slice(payload.as_ref());
                }
            }
            H265Payload::H265PACIPacket(_) => {
                // Not used for media payloads, ignore.
            }
        }

        if pkt.header.marker {
            let mut out = BytesMut::new();
            std::mem::swap(&mut out, &mut self.buffer);
            let is_keyframe = self.keyframe;
            self.keyframe = false;
            return Ok(Some((out, is_keyframe)));
        }

        Ok(None)
    }
}

impl crate::recorder::codec::RtpParser for H265RtpParser {
    type Output = (BytesMut, bool);

    fn push_packet(&mut self, pkt: &Packet) -> Result<Option<Self::Output>> {
        self.push_packet(pkt)
    }
}
