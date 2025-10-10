use super::{CodecAdapter, TrackKind};
use anyhow::{Result, anyhow};
use bytes::{BufMut, Bytes, BytesMut};
use webrtc::rtp::packet::Packet;

const TIMESCALE: u32 = 90_000;
const OBU_TYPE_SEQUENCE_HEADER: u8 = 1;
const MAX_TEMPORAL_UNIT_SIZE: usize = 3 * 1024 * 1024;
const MAX_OBUS_PER_TEMPORAL_UNIT: usize = 10;

pub struct Av1Adapter {
    sequence_header: Option<Vec<u8>>,
    av1c_record: Option<Vec<u8>>,
    codec_string: Option<String>,
    width: u32,
    height: u32,
}

impl Av1Adapter {
    pub fn new() -> Self {
        Self {
            sequence_header: None,
            av1c_record: None,
            codec_string: None,
            width: 0,
            height: 0,
        }
    }

    fn update_sequence_header_from_obu(&mut self, obu_without_size: &[u8]) -> Result<bool> {
        if self
            .sequence_header
            .as_ref()
            .map(|existing| existing.as_slice() == obu_without_size)
            .unwrap_or(false)
        {
            return Ok(false);
        }

        let info = SequenceHeader::parse(obu_without_size)?;
        let codec_string = build_codec_string(&info);
        
        // The av1C record's ConfigOBUs field must contain the marshalled sequence header
        // (i.e., with size field). We need to create an OBU with size field.
        let payload_size = obu_without_size.len() - 1;
        let mut obu_with_size = Vec::new();
        obu_with_size.push(obu_without_size[0] | 0x02); // Set has_size_field bit
        
        // Write LEB128 size
        let mut size_buf = BytesMut::new();
        write_leb128(&mut size_buf, payload_size);
        obu_with_size.extend_from_slice(&size_buf);
        obu_with_size.extend_from_slice(&obu_without_size[1..]);
        
        let av1c = build_av1c_record(&info, &obu_with_size);

        self.width = info.max_frame_width_minus1 + 1;
        self.height = info.max_frame_height_minus1 + 1;
        self.codec_string = Some(codec_string);
        self.sequence_header = Some(obu_without_size.to_vec());
        self.av1c_record = Some(av1c);
        Ok(true)
    }
}

impl Default for Av1Adapter {
    fn default() -> Self {
        Self::new()
    }
}

impl CodecAdapter for Av1Adapter {
    fn kind(&self) -> TrackKind {
        TrackKind::Video
    }

    fn timescale(&self) -> u32 {
        TIMESCALE
    }

    fn ready(&self) -> bool {
        self.sequence_header.is_some()
    }

    fn convert_frame(&mut self, frame: &Bytes) -> (Vec<u8>, bool, bool) {
        let mut config_updated = false;
        let mut is_random_access = false;

        // Parse the temporal unit and ensure all OBUs have size fields
        match parse_temporal_unit(frame.as_ref()) {
            Ok(obus) => {
                tracing::trace!("[av1] parsed {} OBUs from temporal unit (input size: {})", obus.len(), frame.len());
                
                for obu in &obus {
                    let obu_type = (obu[0] >> 3) & 0x0F;
                    tracing::trace!("[av1] OBU type: {}, size: {}, header: 0x{:02x}", obu_type, obu.len(), obu[0]);
                    
                    if obu_type == OBU_TYPE_SEQUENCE_HEADER {
                        // Need to parse the OBU to update sequence header
                        match self.update_sequence_header_from_obu(obu) {
                            Ok(updated) => {
                                if updated {
                                    tracing::info!("[av1] sequence header updated: {}x{}, codec: {}", 
                                        self.width, self.height, 
                                        self.codec_string.as_ref().unwrap_or(&"unknown".to_string()));
                                    config_updated = true;
                                }
                            }
                            Err(err) => {
                                tracing::warn!("[av1] failed to parse sequence header: {err}");
                            }
                        }
                        // According to AV1 spec and mediamtx implementation:
                        // A temporal unit is random access if it contains a sequence header
                        is_random_access = true;
                    }
                }

                // Marshal the temporal unit to ensure all OBUs have size fields
                // This matches the mediamtx implementation using av1.Bitstream.Marshal()
                let marshalled = marshal_bitstream(&obus);
                tracing::trace!("[av1] marshalled bitstream: {} OBUs, output size: {}", obus.len(), marshalled.len());
                
                return (marshalled, is_random_access, config_updated && self.ready());
            }
            Err(err) => {
                tracing::warn!("[av1] failed to parse temporal unit: {err}");
            }
        }

        // Fallback: return the frame as-is
        tracing::trace!("[av1] fallback: returning frame as-is (size: {})", frame.len());
        (
            frame.to_vec(),
            is_random_access,
            config_updated && self.ready(),
        )
    }

    fn codec_config(&self) -> Option<Vec<Vec<u8>>> {
        self.av1c_record.as_ref().map(|record| vec![record.clone()])
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

pub struct Av1RtpParser {
    first_packet_received: bool,
    fragments: Vec<Vec<u8>>,
    fragments_size: usize,
    fragment_next_seq: Option<u16>,
    temporal_unit: Vec<Vec<u8>>,
    temporal_unit_size: usize,
}

impl Default for Av1RtpParser {
    fn default() -> Self {
        Self::new()
    }
}

impl Av1RtpParser {
    pub fn new() -> Self {
        Self {
            first_packet_received: false,
            fragments: Vec::new(),
            fragments_size: 0,
            fragment_next_seq: None,
            temporal_unit: Vec::new(),
            temporal_unit_size: 0,
        }
    }

    fn reset_fragments(&mut self) {
        self.fragments.clear();
        self.fragments_size = 0;
        self.fragment_next_seq = None;
    }

    fn reset_temporal_unit(&mut self) {
        self.temporal_unit.clear();
        self.temporal_unit_size = 0;
    }

    fn decode_obus(&mut self, pkt: &Packet) -> Result<Option<Vec<Vec<u8>>>> {
        if pkt.payload.len() < 2 {
            self.reset_fragments();
            return Err(anyhow!("invalid payload size"));
        }

        let header = pkt.payload[0];
        let z = (header & 0x80) != 0;
        let y = (header & 0x40) != 0;
        let w = (header >> 4) & 0x03;
        let mut payload = &pkt.payload[1..];
        let mut obus = Vec::new();

        tracing::trace!("[av1-rtp] aggregation header: z={}, y={}, w={}, payload_len={}", z, y, w, pkt.payload.len());

        while !payload.is_empty() {
            let obu = if w == 0 || (obus.len() as u8) < (w - 1) {
                let (size, leb_len) = read_leb128(payload)?;
                payload = &payload[leb_len..];

                if size == 0 || payload.len() < size {
                    self.reset_fragments();
                    return Err(anyhow!("invalid OBU size"));
                }

                let obu = payload[..size].to_vec();
                tracing::trace!("[av1-rtp] extracted OBU: size={}, header=0x{:02x}", size, obu.first().copied().unwrap_or(0));
                payload = &payload[size..];
                obu
            } else {
                let obu = payload.to_vec();
                tracing::trace!("[av1-rtp] extracted final OBU: size={}, header=0x{:02x}", obu.len(), obu.first().copied().unwrap_or(0));
                payload = &[];
                obu
            };

            obus.push(obu);
        }

        if w != 0 && obus.len() != w as usize {
            self.reset_fragments();
            return Err(anyhow!("invalid W field"));
        }

        if z {
            if self.fragments_size == 0 {
                self.reset_fragments();
                if !self.first_packet_received {
                    return Ok(None);
                }
                return Err(anyhow!(
                    "received a subsequent fragment without previous fragments"
                ));
            }

            self.first_packet_received = true;

            if let Some(expected) = self.fragment_next_seq
                && pkt.header.sequence_number != expected
            {
                self.reset_fragments();
                return Ok(None);
            }

            self.fragments_size += obus[0].len();
            if self.fragments_size > MAX_TEMPORAL_UNIT_SIZE {
                let size = self.fragments_size;
                self.reset_fragments();
                return Err(anyhow!(
                    "temporal unit size ({size}) exceeds maximum allowed"
                ));
            }

            self.fragments.push(obus[0].clone());
            self.fragment_next_seq = Some(pkt.header.sequence_number.wrapping_add(1));

            if obus.len() == 1 && y {
                return Ok(None);
            }

            obus[0] = join_fragments(&self.fragments, self.fragments_size);
            self.reset_fragments();
        } else {
            self.first_packet_received = true;
        }

        if y {
            if let Some(last) = obus.pop() {
                self.fragments_size = last.len();
                self.fragments.clear();
                self.fragments.push(last);
                self.fragment_next_seq = Some(pkt.header.sequence_number.wrapping_add(1));

                if obus.is_empty() {
                    return Ok(None);
                }
            } else {
                return Ok(None);
            }
        } else {
            self.fragment_next_seq = Some(pkt.header.sequence_number.wrapping_add(1));
        }

        Ok(Some(obus))
    }

    pub fn push_packet(&mut self, pkt: &Packet) -> Result<Option<BytesMut>> {
        let obus = match self.decode_obus(pkt)? {
            Some(obus) => obus,
            None => return Ok(None),
        };

        let obu_count = self.temporal_unit.len() + obus.len();
        if obu_count > MAX_OBUS_PER_TEMPORAL_UNIT {
            self.reset_temporal_unit();
            return Err(anyhow!(
                "OBU count ({obu_count}) exceeds maximum allowed ({MAX_OBUS_PER_TEMPORAL_UNIT})"
            ));
        }

        let additional_size: usize = obus.iter().map(|obu| obu.len()).sum();
        if self.temporal_unit_size + additional_size > MAX_TEMPORAL_UNIT_SIZE {
            let size = self.temporal_unit_size + additional_size;
            self.reset_temporal_unit();
            return Err(anyhow!(
                "temporal unit size ({size}) exceeds maximum allowed ({MAX_TEMPORAL_UNIT_SIZE})"
            ));
        }

        self.temporal_unit.extend(obus);
        self.temporal_unit_size += additional_size;

        if !pkt.header.marker {
            return Ok(None);
        }

        if self.temporal_unit.is_empty() {
            return Ok(None);
        }

        let bitstream = pack_temporal_unit(&self.temporal_unit);
        self.reset_temporal_unit();
        Ok(Some(bitstream))
    }
}

fn join_fragments(fragments: &[Vec<u8>], total_size: usize) -> Vec<u8> {
    let mut joined = Vec::with_capacity(total_size);
    for fragment in fragments {
        joined.extend_from_slice(fragment);
    }
    joined
}

fn pack_temporal_unit(obus: &[Vec<u8>]) -> BytesMut {
    let capacity: usize = obus
        .iter()
        .map(|obu| {
            if obu.is_empty() {
                return 0;
            }
            let payload_len = obu.len().saturating_sub(1);
            1 + leb128_size(payload_len) + payload_len
        })
        .sum();

    let mut buf = BytesMut::with_capacity(capacity);
    for obu in obus {
        if obu.is_empty() {
            continue;
        }

        let payload_len = obu.len() - 1;
        let header = obu[0] | 0x02;
        buf.put_u8(header);
        write_leb128(&mut buf, payload_len);
        buf.extend_from_slice(&obu[1..]);
    }

    buf
}

impl crate::recorder::codec::RtpParser for Av1RtpParser {
    type Output = BytesMut;

    fn push_packet(&mut self, pkt: &Packet) -> Result<Option<Self::Output>> {
        self.push_packet(pkt)
    }
}

/// Parse a temporal unit into individual OBUs without size fields.
/// This matches av1.Bitstream.Unmarshal() in mediacommon.
/// Returns a Vec of OBUs where each OBU has the has_size_field bit cleared.
fn parse_temporal_unit(data: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut cursor = data;
    let mut result = Vec::new();

    while !cursor.is_empty() {
        let header = cursor[0];
        
        // Check forbidden bit
        if header & 0x80 != 0 {
            return Err(anyhow!("forbidden bit set in OBU header"));
        }

        // Check if has_size_field is set
        let has_size_field = (header & 0x02) != 0;
        if !has_size_field {
            return Err(anyhow!("OBU size field missing"));
        }

        // Read LEB128 size
        let (payload_len, leb_len) = read_leb128(&cursor[1..])?;
        let total_len = 1 + leb_len + payload_len;
        
        if cursor.len() < total_len {
            return Err(anyhow!("not enough bytes for OBU"));
        }

        // Create OBU without size field (clear bit 1 of header)
        let mut obu = Vec::with_capacity(1 + payload_len);
        obu.push(header & 0b11111101); // Clear has_size_field bit
        obu.extend_from_slice(&cursor[1 + leb_len..total_len]);

        result.push(obu);
        cursor = &cursor[total_len..];
    }

    Ok(result)
}

/// Marshal a bitstream (list of OBUs without size fields) into Low Overhead Bitstream Format.
/// This matches av1.Bitstream.Marshal() in mediacommon.
/// All OBUs in the output will have size fields.
fn marshal_bitstream(obus: &[Vec<u8>]) -> Vec<u8> {
    // Calculate total size needed
    let mut total_size = 0;
    for obu in obus {
        let has_size = (obu[0] & 0x02) != 0;
        if !has_size {
            let payload_size = obu.len() - 1;
            total_size += 1 + leb128_size(payload_size) + payload_size;
        } else {
            total_size += obu.len();
        }
    }

    let mut buf = Vec::with_capacity(total_size);
    
    for obu in obus {
        let has_size = (obu[0] & 0x02) != 0;
        
        if !has_size {
            // Add size field
            let payload_size = obu.len() - 1;
            let header_with_size = obu[0] | 0x02; // Set has_size_field bit
            buf.push(header_with_size);
            
            // Write LEB128 size
            let mut size_buf = BytesMut::new();
            write_leb128(&mut size_buf, payload_size);
            buf.extend_from_slice(&size_buf);
            
            // Write payload
            buf.extend_from_slice(&obu[1..]);
            
            tracing::trace!("[av1] marshalled OBU: type={}, header=0x{:02x}, size={}, total={}", 
                (obu[0] >> 3) & 0x0F, header_with_size, payload_size, 1 + size_buf.len() + payload_size);
        } else {
            // Already has size field, copy as-is
            buf.extend_from_slice(obu);
            tracing::trace!("[av1] OBU already has size field, copied as-is: {}", obu.len());
        }
    }

    buf
}

fn read_leb128(buf: &[u8]) -> Result<(usize, usize)> {
    let mut value: usize = 0;
    let mut shift = 0;
    for (i, byte) in buf.iter().copied().enumerate() {
        value |= ((byte & 0x7F) as usize) << shift;
        if (byte & 0x80) == 0 {
            return Ok((value, i + 1));
        }
        shift += 7;
        if shift > 28 {
            break;
        }
    }
    Err(anyhow!("invalid LEB128"))
}

fn leb128_size(mut value: usize) -> usize {
    let mut size = 1;
    while value >= 0x80 {
        value >>= 7;
        size += 1;
    }
    size
}

fn write_leb128(buf: &mut BytesMut, mut value: usize) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
            buf.put_u8(byte);
        } else {
            buf.put_u8(byte);
            break;
        }
    }
}

#[derive(Clone, Debug)]
struct SequenceHeader {
    seq_profile: u8,
    seq_level_idx: Vec<u8>,
    seq_tier: Vec<bool>,
    max_frame_width_minus1: u32,
    max_frame_height_minus1: u32,
    color_config: ColorConfig,
}

#[derive(Clone, Debug)]
struct ColorConfig {
    #[allow(dead_code)]
    high_bit_depth: bool,
    #[allow(dead_code)]
    twelve_bit: bool,
    bit_depth: u8,
    mono_chrome: bool,
    #[allow(dead_code)]
    color_primaries: u8,
    #[allow(dead_code)]
    transfer_characteristics: u8,
    #[allow(dead_code)]
    matrix_coefficients: u8,
    color_range: bool,
    subsampling_x: bool,
    subsampling_y: bool,
    chroma_sample_position: u8,
}

impl SequenceHeader {
    fn parse(obu: &[u8]) -> Result<Self> {
        if obu.is_empty() {
            return Err(anyhow!("empty OBU"));
        }
        let obu_type = (obu[0] >> 3) & 0x0F;
        if obu_type != OBU_TYPE_SEQUENCE_HEADER {
            return Err(anyhow!("not a sequence header OBU"));
        }

        let mut br = BitReader::new(&obu[1..]);

        let seq_profile = br.read_bits(3)? as u8;
        let _still_picture = br.read_flag()?;
        let reduced_still_picture_header = br.read_flag()?;

        let (seq_level_idx, seq_tier) = if reduced_still_picture_header {
            let level = br.read_bits(5)? as u8;
            (vec![level], vec![false])
        } else {
            let timing_info_present = br.read_flag()?;
            if timing_info_present {
                br.skip_bits(32)?;
                br.skip_bits(32)?;
                let decoder_model_info_present_flag = br.read_flag()?;
                if decoder_model_info_present_flag {
                    return Err(anyhow!("decoder_model_info_present_flag unsupported"));
                }
            }

            let initial_display_delay_present_flag = br.read_flag()?;
            let operating_points_cnt_minus1 = br.read_bits(5)? as u8;
            let mut seq_level_idx = Vec::with_capacity((operating_points_cnt_minus1 + 1) as usize);
            let mut seq_tier = Vec::with_capacity((operating_points_cnt_minus1 + 1) as usize);

            for _ in 0..=operating_points_cnt_minus1 {
                br.skip_bits(12)?; // operating_point_idc
                let level = br.read_bits(5)? as u8;
                let tier = if level > 7 { br.read_flag()? } else { false };
                seq_level_idx.push(level);
                seq_tier.push(tier);

                if initial_display_delay_present_flag {
                    let present = br.read_flag()?;
                    if present {
                        br.read_bits(4)?;
                        return Err(anyhow!("initial_display_delay_present_flag unsupported"));
                    }
                }
            }

            (seq_level_idx, seq_tier)
        };

        let frame_width_bits_minus1 = br.read_bits(4)? as usize;
        let frame_height_bits_minus1 = br.read_bits(4)? as usize;
        let max_frame_width_minus1 = br.read_bits(frame_width_bits_minus1 + 1)? as u32;
        let max_frame_height_minus1 = br.read_bits(frame_height_bits_minus1 + 1)? as u32;

        let frame_id_numbers_present_flag = if reduced_still_picture_header {
            false
        } else {
            br.read_flag()?
        };
        if frame_id_numbers_present_flag {
            br.read_bits(4)?;
            br.read_bits(3)?;
        }

        let _use_128x128_superblock = br.read_flag()?;
        let _enable_filter_intra = br.read_flag()?;
        let _enable_intra_edge_filter = br.read_flag()?;

        if !reduced_still_picture_header {
            let _enable_interintra_compound = br.read_flag()?;
            let _enable_masked_compound = br.read_flag()?;
            let _enable_warped_motion = br.read_flag()?;
            let _enable_dual_filter = br.read_flag()?;
            let enable_order_hint = br.read_flag()?;

            if enable_order_hint {
                let _enable_jnt_comp = br.read_flag()?;
                let _enable_ref_frame_mvs = br.read_flag()?;
            }

            let seq_choose_screen_content_tools = br.read_flag()?;
            let seq_force_screen_content_tools = if seq_choose_screen_content_tools {
                2u8
            } else {
                br.read_bits(1)? as u8
            };

            if seq_force_screen_content_tools > 0 {
                let seq_choose_integer_mv = br.read_flag()?;
                if !seq_choose_integer_mv {
                    br.read_bits(1)?;
                }
            }

            if enable_order_hint {
                br.read_bits(3)?;
            }
        }

        let _enable_super_res = br.read_flag()?;
        let _enable_cdef = br.read_flag()?;
        let _enable_restoration = br.read_flag()?;

        let color_config = ColorConfig::parse(seq_profile, &mut br)?;

        let _film_grain = br.read_flag()?;

        Ok(SequenceHeader {
            seq_profile,
            seq_level_idx,
            seq_tier,
            max_frame_width_minus1,
            max_frame_height_minus1,
            color_config,
        })
    }
}

impl ColorConfig {
    fn parse(seq_profile: u8, br: &mut BitReader<'_>) -> Result<Self> {
        let high_bit_depth = br.read_flag()?;
        let mut twelve_bit = false;
        let bit_depth = if seq_profile == 2 && high_bit_depth {
            twelve_bit = br.read_flag()?;
            if twelve_bit { 12 } else { 10 }
        } else if high_bit_depth {
            10
        } else {
            8
        };

        let mono_chrome = if seq_profile == 1 {
            false
        } else {
            br.read_flag()?
        };

        let color_description_present_flag = br.read_flag()?;
        let (color_primaries, transfer_characteristics, matrix_coefficients) =
            if color_description_present_flag {
                let primaries = br.read_bits(8)? as u8;
                let transfer = br.read_bits(8)? as u8;
                let matrix = br.read_bits(8)? as u8;
                (primaries, transfer, matrix)
            } else {
                (2, 2, 2)
            };

        let color_range;
        let mut subsampling_x = true;
        let mut subsampling_y = true;
        let mut chroma_sample_position = 0u8;

        if mono_chrome {
            color_range = br.read_flag()?;
        } else if color_description_present_flag
            && color_primaries == 1
            && transfer_characteristics == 13
            && matrix_coefficients == 0
        {
            color_range = true;
            subsampling_x = false;
            subsampling_y = false;
        } else {
            color_range = br.read_flag()?;
            match seq_profile {
                0 => {
                    subsampling_x = true;
                    subsampling_y = true;
                }
                1 => {
                    subsampling_x = false;
                    subsampling_y = false;
                }
                _ => {
                    if bit_depth == 12 {
                        subsampling_x = br.read_flag()?;
                        if subsampling_x {
                            subsampling_y = br.read_flag()?;
                        } else {
                            subsampling_y = false;
                        }
                    } else {
                        subsampling_x = true;
                        subsampling_y = false;
                    }
                }
            }

            if subsampling_x && subsampling_y {
                chroma_sample_position = br.read_bits(2)? as u8;
            }
        }

        Ok(Self {
            high_bit_depth,
            twelve_bit,
            bit_depth,
            mono_chrome,
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
            color_range,
            subsampling_x,
            subsampling_y,
            chroma_sample_position,
        })
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_len: usize,
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            bit_len: data.len() * 8,
            bit_pos: 0,
        }
    }

    fn ensure(&self, bits: usize) -> Result<()> {
        if self.bit_pos + bits > self.bit_len {
            Err(anyhow!("not enough bits"))
        } else {
            Ok(())
        }
    }

    fn read_bits(&mut self, count: usize) -> Result<u64> {
        if count == 0 {
            return Ok(0);
        }
        self.ensure(count)?;
        let mut value = 0u64;
        for _ in 0..count {
            let byte_index = self.bit_pos / 8;
            let bit_offset = 7 - (self.bit_pos % 8);
            let bit = (self.data[byte_index] >> bit_offset) & 1;
            value = (value << 1) | bit as u64;
            self.bit_pos += 1;
        }
        Ok(value)
    }

    fn read_flag(&mut self) -> Result<bool> {
        Ok(self.read_bits(1)? != 0)
    }

    fn skip_bits(&mut self, count: usize) -> Result<()> {
        self.ensure(count)?;
        self.bit_pos += count;
        Ok(())
    }
}

fn build_codec_string(info: &SequenceHeader) -> String {
    let profile = info.seq_profile;
    let level = info.seq_level_idx.first().copied().unwrap_or(0);
    let tier_char = if info.seq_tier.first().copied().unwrap_or(false) {
        'H'
    } else {
        'M'
    };

    let bit_depth = info.color_config.bit_depth;
    let chroma = if info.color_config.mono_chrome {
        0
    } else if info.color_config.subsampling_x && info.color_config.subsampling_y {
        1
    } else if info.color_config.subsampling_x && !info.color_config.subsampling_y {
        2
    } else {
        3
    };
    let color_range = if info.color_config.color_range { 1 } else { 0 };
    let chroma_pos = info.color_config.chroma_sample_position;

    format!(
        "av01.{}.{}{:02}.{:02}.{}.{}.{}",
        profile, tier_char, level, bit_depth, chroma, color_range, chroma_pos
    )
}

fn build_av1c_record(info: &SequenceHeader, sequence_header_with_size: &[u8]) -> Vec<u8> {
    let mut record = Vec::new();

    // marker (1) + version (7)
    record.push(0x81);

    let seq_level = info.seq_level_idx.first().copied().unwrap_or(0) & 0x1F;
    let byte1 = ((info.seq_profile & 0x07) << 5) | seq_level;
    record.push(byte1);

    let tier_bit = if info.seq_tier.first().copied().unwrap_or(false) {
        1
    } else {
        0
    };
    let color = &info.color_config;
    let high_bitdepth = if color.bit_depth > 8 { 1 } else { 0 };
    let twelve_bit = if color.bit_depth == 12 { 1 } else { 0 };
    let monochrome = if color.mono_chrome { 1 } else { 0 };
    let chroma_x = if color.subsampling_x { 1 } else { 0 };
    let chroma_y = if color.subsampling_y { 1 } else { 0 };
    let chroma_pos = color.chroma_sample_position & 0x03;

    let byte2 = (tier_bit << 7)
        | (high_bitdepth << 6)
        | (twelve_bit << 5)
        | (monochrome << 4)
        | (chroma_x << 3)
        | (chroma_y << 2)
        | chroma_pos;
    record.push(byte2);

    // reserved (3 bits) + initial_presentation_delay_present (1 bit) + delay_minus_one/reserved (4 bits)
    record.push(0);

    record.extend_from_slice(sequence_header_with_size);

    record
}

#[cfg(test)]
mod tests {
    const SHORT_OBU: &[u8] = &[
        0x0a, 0x0e, 0x00, 0x00, 0x00, 0x4a, 0xab, 0xbf, 0xc3, 0x77, 0x6b, 0xe4, 0x40, 0x40, 0x40,
        0x41,
    ];

    const RTP_PAYLOAD_SINGLE: &[u8] = &[
        0x18, 0x0a, 0x0e, 0x00, 0x00, 0x00, 0x4a, 0xab, 0xbf, 0xc3, 0x77, 0x6b, 0xe4, 0x40, 0x40,
        0x40, 0x41,
    ];

    const RTP_PAYLOAD_AGGREGATED: &[u8] = &[
        0x28, 0x10, 0x0a, 0x0e, 0x00, 0x00, 0x00, 0x4a, 0xab, 0xbf, 0xc3, 0x77, 0x6b, 0xe4, 0x40,
        0x40, 0x40, 0x41, 0x0a, 0x0e, 0x00, 0x00, 0x00, 0x4a, 0xab, 0xbf, 0xc3, 0x77, 0x6b, 0xe4,
        0x40, 0x40, 0x40, 0x41,
    ];

    fn make_packet(payload: &'static [u8], marker: bool, seq: u16) -> Packet {
        let mut pkt = Packet::default();
        pkt.header.marker = marker;
        pkt.header.payload_type = 96;
        pkt.header.sequence_number = seq;
        pkt.payload = Bytes::from_static(payload);
        pkt
    }

    #[test]
    fn rtp_parser_reassembles_single_packet() {
        let mut parser = Av1RtpParser::new();
        let pkt = make_packet(RTP_PAYLOAD_SINGLE, true, 17645);

        let result = parser.push_packet(&pkt).expect("push packet");
        assert!(result.is_some(), "parser should output frame at marker");

        let expected = {
            let obus = vec![SHORT_OBU.to_vec()];
            pack_temporal_unit(&obus).to_vec()
        };

        assert_eq!(result.unwrap().to_vec(), expected);
    }

    #[test]
    fn rtp_parser_handles_aggregated_obus() {
        let mut parser = Av1RtpParser::new();
        let pkt = make_packet(RTP_PAYLOAD_AGGREGATED, true, 20010);

        let result = parser.push_packet(&pkt).expect("push packet");
        assert!(result.is_some(), "parser should output frame at marker");

        let expected = {
            let obus = vec![SHORT_OBU.to_vec(), SHORT_OBU.to_vec()];
            pack_temporal_unit(&obus).to_vec()
        };

        assert_eq!(result.unwrap().to_vec(), expected);
    }
    use super::{
        Av1RtpParser, ColorConfig, SequenceHeader, build_av1c_record, pack_temporal_unit,
        read_leb128,
    };
    use bytes::Bytes;
    use webrtc::rtp::packet::Packet;

    #[test]
    fn leb128_decoding_examples() {
        let vectors = [
            (&[0x00u8][..], 0usize),
            (&[0x80, 0x01][..], 128usize),
            (&[0xFF, 0x01][..], 255usize),
        ];

        for (bytes, expected) in vectors {
            let (value, consumed) = read_leb128(bytes).expect("valid leb128");
            assert_eq!(value, expected);
            assert_eq!(consumed, bytes.len());
        }
    }

    #[test]
    fn av1c_record_includes_sequence_header() {
        let info = SequenceHeader {
            seq_profile: 0,
            seq_level_idx: vec![8],
            seq_tier: vec![false],
            max_frame_width_minus1: 1919,
            max_frame_height_minus1: 1079,
            color_config: ColorConfig {
                high_bit_depth: false,
                twelve_bit: false,
                bit_depth: 8,
                mono_chrome: false,
                color_primaries: 2,
                transfer_characteristics: 2,
                matrix_coefficients: 2,
                color_range: false,
                subsampling_x: true,
                subsampling_y: true,
                chroma_sample_position: 0,
            },
        };

        let sequence_header_with_size = vec![0x82, 0x01, 0xAA];
        let record = build_av1c_record(&info, &sequence_header_with_size);

        assert_eq!(record[0], 0x81); // marker + version
        assert_eq!(record[1] & 0x1F, 8); // level index
        assert_eq!(record[2] & 0x80, 0); // tier 0
        assert_eq!(record[2] & 0x40, 0); // high_bitdepth = 0
        assert_eq!(&record[4..], &sequence_header_with_size[..]);
    }
}
