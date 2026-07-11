use super::{CodecAdapter, TrackKind};
use anyhow::{Result, anyhow};
use bytes::{BufMut, Bytes, BytesMut};
use rtc_rtp::codec::av1::Av1Depacketizer;
use rtc_rtp::packet::Packet;
use rtc_rtp::packetizer::Depacketizer;

const TIMESCALE: u32 = 90_000;
const OBU_TYPE_SEQUENCE_HEADER: u8 = 1;
const MAX_TEMPORAL_UNIT_SIZE: usize = 3 * 1024 * 1024;

// AV1 aggregation header bit masks (matches rtc-rtp codec::av1)
const AV1_Z_MASK: u8 = 0b1000_0000;
const AV1_Y_MASK: u8 = 0b0100_0000;
const AV1_W_MASK: u8 = 0b0011_0000;
const AV1_N_MASK: u8 = 0b0000_1000;

fn format_av1_agg_header(header: u8) -> String {
    let z = (header & AV1_Z_MASK) != 0;
    let y = (header & AV1_Y_MASK) != 0;
    let w = (header & AV1_W_MASK) >> 4;
    let n = (header & AV1_N_MASK) != 0;
    format!("0x{header:02x} Z={z} Y={y} W={w} N={n}")
}

fn hex_prefix(bytes: &[u8], max: usize) -> String {
    let len = bytes.len().min(max);
    let mut s = String::with_capacity(len * 3);
    for (i, b) in bytes[..len].iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        s.push_str(&format!("{b:02x}"));
    }
    if bytes.len() > max {
        s.push_str(&format!(" ... ({} bytes total)", bytes.len()));
    }
    s
}

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
                tracing::trace!(
                    "[av1] parsed {} OBUs from temporal unit (input size: {})",
                    obus.len(),
                    frame.len()
                );

                for obu in &obus {
                    let obu_type = (obu[0] >> 3) & 0x0F;
                    tracing::trace!(
                        "[av1] OBU type: {}, size: {}, header: 0x{:02x}",
                        obu_type,
                        obu.len(),
                        obu[0]
                    );

                    if obu_type == OBU_TYPE_SEQUENCE_HEADER {
                        // Need to parse the OBU to update sequence header
                        match self.update_sequence_header_from_obu(obu) {
                            Ok(updated) => {
                                if updated {
                                    tracing::info!(
                                        "[av1] sequence header updated: {}x{}, codec: {}",
                                        self.width,
                                        self.height,
                                        self.codec_string
                                            .as_ref()
                                            .unwrap_or(&"unknown".to_string())
                                    );
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
                tracing::trace!(
                    "[av1] marshalled bitstream: {} OBUs, output size: {}",
                    obus.len(),
                    marshalled.len()
                );

                return (marshalled, is_random_access, config_updated && self.ready());
            }
            Err(err) => {
                tracing::warn!("[av1] failed to parse temporal unit: {err}");
            }
        }

        // Fallback: return the frame as-is
        tracing::trace!(
            "[av1] fallback: returning frame as-is (size: {})",
            frame.len()
        );
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
    depacketizer: Av1Depacketizer,
    accumulator: BytesMut,
    expected_seq: Option<u16>,
    last_timestamp: u32,
    /// When a timestamp change finishes a previous temporal unit, that unit is
    /// returned on the next call so the current packet can still be processed.
    pending_frame: Option<BytesMut>,
}

impl Default for Av1RtpParser {
    fn default() -> Self {
        Self::new()
    }
}

impl Av1RtpParser {
    pub fn new() -> Self {
        Self {
            depacketizer: Av1Depacketizer::new(),
            accumulator: BytesMut::new(),
            expected_seq: None,
            last_timestamp: 0,
            pending_frame: None,
        }
    }

    fn reset(&mut self) {
        self.depacketizer = Av1Depacketizer::new();
        self.accumulator.clear();
        self.expected_seq = None;
        self.last_timestamp = 0;
        self.pending_frame = None;
    }

    pub fn push_packet(&mut self, pkt: &Packet) -> Result<Option<BytesMut>> {
        // If a previous call produced a pending frame, return it first.
        if let Some(frame) = self.pending_frame.take() {
            return Ok(Some(frame));
        }

        let agg_header = pkt.payload.first().copied().unwrap_or(0);
        tracing::trace!(
            "[av1-rtp] packet seq={} ts={} marker={} len={} agg={}",
            pkt.header.sequence_number,
            pkt.header.timestamp,
            pkt.header.marker,
            pkt.payload.len(),
            format_av1_agg_header(agg_header),
        );

        // Detect sequence number gaps. AV1 depacketization is stateful, so a
        // missing packet makes the current accumulator unusable.
        if let Some(expected) = self.expected_seq
            && pkt.header.sequence_number != expected
        {
            tracing::debug!(
                "[av1-rtp] sequence gap detected: expected {}, got {}; resetting",
                expected,
                pkt.header.sequence_number
            );
            self.reset();
        }
        self.expected_seq = Some(pkt.header.sequence_number.wrapping_add(1));

        // Timestamp discontinuity indicates a new temporal unit. The AV1 RTP
        // spec says a receiver must handle the case where the marker bit is not
        // set on the last packet of a temporal unit; the boundary is also
        // indicated by the next packet having an incremented timestamp. Emit the
        // accumulated unit instead of dropping it.
        if self.last_timestamp != 0 && pkt.header.timestamp != self.last_timestamp {
            if !self.accumulator.is_empty() {
                tracing::debug!(
                    "[av1-rtp] timestamp discontinuity ({} -> {}); emitting accumulated temporal unit ({} bytes)",
                    self.last_timestamp,
                    pkt.header.timestamp,
                    self.accumulator.len()
                );
                self.pending_frame = Some(std::mem::take(&mut self.accumulator));
            }
            // Reset depacketizer/accumulator state for the new temporal unit,
            // but keep expected_seq so the current packet is processed normally.
            self.depacketizer = Av1Depacketizer::new();
            self.accumulator.clear();
        }
        self.last_timestamp = pkt.header.timestamp;

        let obus = match self.depacketizer.depacketize(&pkt.payload) {
            Ok(obus) => {
                tracing::trace!(
                    "[av1-rtp] depacketized seq={} ts={} output_len={}",
                    pkt.header.sequence_number,
                    pkt.header.timestamp,
                    obus.len()
                );
                obus
            }
            Err(e) => {
                tracing::warn!(
                    "[av1-rtp] depacketize error seq={} ts={} marker={} len={} agg={} payload={}: {e}",
                    pkt.header.sequence_number,
                    pkt.header.timestamp,
                    pkt.header.marker,
                    pkt.payload.len(),
                    format_av1_agg_header(agg_header),
                    hex_prefix(&pkt.payload, 64),
                );
                // Preserve any already-complete temporal unit before resetting.
                if self.pending_frame.is_none() && !self.accumulator.is_empty() {
                    self.pending_frame = Some(std::mem::take(&mut self.accumulator));
                }
                self.reset();
                return Err(anyhow!("AV1 depacketization failed: {e}"));
            }
        };

        if !obus.is_empty() {
            if self.accumulator.len() + obus.len() > MAX_TEMPORAL_UNIT_SIZE {
                let size = self.accumulator.len() + obus.len();
                self.reset();
                return Err(anyhow!(
                    "temporal unit size ({size}) exceeds maximum allowed ({MAX_TEMPORAL_UNIT_SIZE})"
                ));
            }
            self.accumulator.extend_from_slice(&obus);
        }

        // A temporal unit is complete when the RTP marker bit is set and the
        // last OBU does not continue into the next packet (Y flag is false).
        if pkt.header.marker && self.depacketizer.y {
            tracing::warn!(
                "[av1-rtp] marker set but last OBU continues (Y=1); malformed packet, dropping"
            );
            self.reset();
            return Ok(None);
        }

        if pkt.header.marker {
            if self.accumulator.is_empty() {
                return Ok(None);
            }

            let temporal_unit = std::mem::take(&mut self.accumulator);
            // Reset the depacketizer for the next temporal unit, but keep
            // sequence/timestamp tracking so the next packet is processed
            // normally.
            self.depacketizer = Av1Depacketizer::new();
            tracing::trace!(
                "[av1-rtp] temporal unit complete: seq={} size={}",
                pkt.header.sequence_number,
                temporal_unit.len()
            );

            if let Some(pending) = self.pending_frame.take() {
                self.pending_frame = Some(temporal_unit);
                return Ok(Some(pending));
            }
            return Ok(Some(temporal_unit));
        }

        if let Some(pending) = self.pending_frame.take() {
            return Ok(Some(pending));
        }
        Ok(None)
    }

    /// Return a pending temporal unit that was held back because of a timestamp
    /// discontinuity. This should be called when the RTP stream ends so the last
    /// complete frame is not lost. Unfinished accumulator data (no marker bit
    /// seen) is intentionally discarded to avoid writing an incomplete frame.
    pub fn flush(&mut self) -> Option<BytesMut> {
        self.pending_frame.take()
    }
}

impl crate::recorder::codec::RtpParser for Av1RtpParser {
    type Output = BytesMut;

    fn push_packet(&mut self, pkt: &Packet) -> Result<Option<Self::Output>> {
        self.push_packet(pkt)
    }

    fn flush(&mut self) -> Option<Self::Output> {
        self.flush()
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

            tracing::trace!(
                "[av1] marshalled OBU: type={}, header=0x{:02x}, size={}, total={}",
                (obu[0] >> 3) & 0x0F,
                header_with_size,
                payload_size,
                1 + size_buf.len() + payload_size
            );
        } else {
            // Already has size field, copy as-is
            buf.extend_from_slice(obu);
            tracing::trace!(
                "[av1] OBU already has size field, copied as-is: {}",
                obu.len()
            );
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
    bit_depth: u8,
    mono_chrome: bool,
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
        let bit_depth = if seq_profile == 2 && high_bit_depth {
            let twelve_bit = br.read_flag()?;
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

        let mut subsampling_x = true;
        let mut subsampling_y = true;
        let mut chroma_sample_position = 0u8;

        if mono_chrome {
            br.skip_bits(1)?; // color_range
        } else if color_description_present_flag
            && color_primaries == 1
            && transfer_characteristics == 13
            && matrix_coefficients == 0
        {
            // Implicit 4:4:4 subsampling and full range; no bits to read.
            subsampling_x = false;
            subsampling_y = false;
        } else {
            br.skip_bits(1)?; // color_range
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
            bit_depth,
            mono_chrome,
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

    // Browsers accept the minimal AV1 codec string reliably:
    // av01.<profile>.<level><tier>.<bitDepth>
    // Adding chroma subsampling / color metadata often causes
    // MediaSource.isTypeSupported to return false in practice.
    format!(
        "av01.{}.{:02}{}.{:02}",
        profile, level, tier_char, bit_depth
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

        // Upstream depacketizer preserves OBUs that already carry size fields.
        assert_eq!(result.unwrap().to_vec(), SHORT_OBU);
    }

    #[test]
    fn rtp_parser_handles_aggregated_obus() {
        let mut parser = Av1RtpParser::new();
        let pkt = make_packet(RTP_PAYLOAD_AGGREGATED, true, 20010);

        let result = parser.push_packet(&pkt).expect("push packet");
        assert!(result.is_some(), "parser should output frame at marker");

        let mut expected = Vec::with_capacity(SHORT_OBU.len() * 2);
        expected.extend_from_slice(SHORT_OBU);
        expected.extend_from_slice(SHORT_OBU);
        assert_eq!(result.unwrap().to_vec(), expected);
    }

    fn make_packet_owned(payload: Vec<u8>, marker: bool, seq: u16) -> Packet {
        let mut pkt = Packet::default();
        pkt.header.marker = marker;
        pkt.header.payload_type = 96;
        pkt.header.sequence_number = seq;
        pkt.payload = Bytes::from(payload);
        pkt
    }

    #[test]
    fn rtp_parser_emits_frame_on_timestamp_change_when_marker_missing() {
        // A Frame OBU large enough to be split into two RTP packets.
        let mut obu = vec![0x30]; // Frame OBU, no extension, no size field
        obu.extend_from_slice(&[0xAB; 200]);

        let mut payloader = Av1Payloader::default();
        let packets = payloader
            .payload(120, &Bytes::from(obu))
            .expect("packetize AV1 frame");
        assert_eq!(packets.len(), 2, "frame should be split into two packets");

        let mut parser = Av1RtpParser::new();
        let mut first = make_packet_owned(packets[0].to_vec(), false, 1);
        first.header.timestamp = 1000;
        let mut second = make_packet_owned(packets[1].to_vec(), false, 2);
        second.header.timestamp = 1000;
        // Third packet belongs to the next temporal unit and has a new timestamp.
        let mut third = make_packet(RTP_PAYLOAD_SINGLE, true, 3);
        third.header.timestamp = 2000;

        assert!(
            parser.push_packet(&first).unwrap().is_none(),
            "first fragment should not emit a frame"
        );
        assert!(
            parser.push_packet(&second).unwrap().is_none(),
            "second fragment without marker should not emit a frame"
        );

        let result = parser.push_packet(&third).unwrap();
        assert!(
            result.is_some(),
            "timestamp change should emit the accumulated temporal unit"
        );
        // The emitted frame contains the reassembled OBU with an added size field.
        assert!(result.unwrap().len() > 200);

        // The next call returns the frame from the third packet.
        let next = parser.push_packet(&Packet::default()).unwrap();
        assert!(
            next.is_some(),
            "pending frame from the third packet should be returned"
        );
    }

    #[test]
    fn rtp_parser_recovers_after_depacketize_error() {
        let mut parser = Av1RtpParser::new();

        // An invalid AV1 RTP payload (only aggregation header, no OBU data).
        let bad = make_packet(&[0x10], true, 1);
        assert!(parser.push_packet(&bad).is_err());

        // A subsequent valid packet should still be parsed.
        let good = make_packet(RTP_PAYLOAD_SINGLE, true, 2);
        let result = parser.push_packet(&good).unwrap();
        assert!(
            result.is_some(),
            "parser should recover and parse valid packet"
        );
    }

    #[test]
    fn rtp_parser_reassembles_fragmented_obu_with_size_field() {
        // OBS-style packetization: the OBU already carries a low-overhead size
        // field and is fragmented across multiple RTP packets (Z/Y flags).
        let mut obu = BytesMut::new();
        obu.put_u8(0x32); // Frame OBU, no extension, has size field
        let payload = vec![0xAB; 500];
        write_leb128(&mut obu, payload.len());
        obu.extend_from_slice(&payload);
        let obu = obu.freeze();

        let first_fragment_size = 400;
        let mut p1 = vec![0x50]; // Z=0, Y=1, W=1
        p1.extend_from_slice(&obu[..first_fragment_size]);
        let mut p2 = vec![0x90]; // Z=1, Y=0, W=1
        p2.extend_from_slice(&obu[first_fragment_size..]);

        let mut parser = Av1RtpParser::new();
        assert!(
            parser
                .push_packet(&make_packet_owned(p1, false, 1))
                .unwrap()
                .is_none(),
            "first fragment should be buffered"
        );
        let result = parser.push_packet(&make_packet_owned(p2, true, 2)).unwrap();
        assert!(
            result.is_some(),
            "reassembled OBU should be emitted at marker"
        );
        assert_eq!(result.unwrap().to_vec(), obu.to_vec());
    }

    #[test]
    fn rtp_parser_flush_drops_unfinished_temporal_unit() {
        // A fragmented frame without a marker bit is buffered in the accumulator.
        // flush() intentionally discards unfinished accumulator data so that an
        // incomplete temporal unit is not written to the recording.
        let mut obu = vec![0x30]; // Frame OBU, no extension, no size field
        obu.extend_from_slice(&[0xAB; 200]);

        let mut payloader = Av1Payloader::default();
        let packets = payloader
            .payload(120, &Bytes::from(obu))
            .expect("packetize AV1 frame");
        assert_eq!(packets.len(), 2, "frame should be split into two packets");

        let mut parser = Av1RtpParser::new();
        let first = make_packet_owned(packets[0].to_vec(), false, 1);
        let second = make_packet_owned(packets[1].to_vec(), false, 2);

        assert!(parser.push_packet(&first).unwrap().is_none());
        assert!(parser.push_packet(&second).unwrap().is_none());

        let flushed = parser.flush();
        assert!(
            flushed.is_none(),
            "flush should drop unfinished accumulator"
        );
    }

    use super::{
        Av1RtpParser, ColorConfig, SequenceHeader, build_av1c_record, read_leb128, write_leb128,
    };
    use bytes::BufMut;
    use bytes::{Bytes, BytesMut};
    use rtc_rtp::codec::av1::Av1Payloader;
    use rtc_rtp::packet::Packet;
    use rtc_rtp::packetizer::Payloader;

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
                bit_depth: 8,
                mono_chrome: false,
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
