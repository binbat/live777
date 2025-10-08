use super::{CodecAdapter, TrackKind};
use anyhow::{Result, anyhow};
use bytes::{Bytes, BytesMut};
use webrtc::rtp::packet::Packet;
use webrtc::rtp::{codecs::vp9::Vp9Packet, packetizer::Depacketizer};

/// Minimal VP9 adapter. For fMP4 we carry raw frame bytes into samples.
pub struct Vp9Adapter {
    timescale: u32,
    width: u32,
    height: u32,
    profile: u8,
    bit_depth: u8,
    chroma_subsampling: u8,
    color_range: bool,
    colour_primaries: u8,
    transfer_characteristics: u8,
    matrix_coefficients: u8,
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
            profile: 0,
            bit_depth: 8,
            chroma_subsampling: 1,
            color_range: false,
            colour_primaries: 2,
            transfer_characteristics: 2,
            matrix_coefficients: 2,
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
        self.width > 0 && self.height > 0
    }
    fn convert_frame(&mut self, frame: &Bytes) -> (Vec<u8>, bool, bool) {
        let payload = frame.as_ref();

        let mut is_key = false;
        let mut cfg_updated = false;

        if let Some(header) = Vp9FrameHeader::parse(payload) {
            is_key = header.is_keyframe();
            let metadata_updated = self.apply_header(&header);
            if let Some(size) = header.frame_size {
                cfg_updated = self.update_dimensions(size.width(), size.height());
            }
            cfg_updated |= metadata_updated;
        }

        (payload.to_vec(), is_key, cfg_updated)
    }
    fn codec_config(&self) -> Option<Vec<Vec<u8>>> {
        Some(vec![])
    }
    fn codec_string(&self) -> Option<String> {
        Some(format!(
            "vp09.{:02}.{:02}.{:02}.{:02}.{:02}.{:02}.{:02}.{:02}",
            self.profile,
            10, // level 1
            self.bit_depth,
            self.chroma_subsampling,
            self.colour_primaries,
            self.transfer_characteristics,
            self.matrix_coefficients,
            u8::from(self.color_range)
        ))
    }
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
}

impl Vp9Adapter {
    fn apply_header(&mut self, header: &Vp9FrameHeader) -> bool {
        let mut updated = false;

        if self.profile != header.profile {
            self.profile = header.profile;
            updated = true;
        }

        if let Some(color) = header.color_config {
            if self.bit_depth != color.bit_depth {
                self.bit_depth = color.bit_depth;
                updated = true;
            }

            let chroma = color.chroma_sampling();
            if self.chroma_subsampling != chroma {
                self.chroma_subsampling = chroma;
                updated = true;
            }

            if self.color_range != color.color_range {
                self.color_range = color.color_range;
                updated = true;
            }
        }

        updated
    }

    fn update_dimensions(&mut self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }

        let was_ready = self.ready();
        let dims_changed = self.width != width || self.height != height;

        self.width = width;
        self.height = height;

        (!was_ready && self.ready()) || (was_ready && dims_changed)
    }
}

#[derive(Debug, Clone, Copy)]
struct Vp9FrameSize {
    frame_width_minus_one: u16,
    frame_height_minus_one: u16,
}

impl Vp9FrameSize {
    fn width(self) -> u32 {
        self.frame_width_minus_one as u32 + 1
    }

    fn height(self) -> u32 {
        self.frame_height_minus_one as u32 + 1
    }
}

#[derive(Debug, Clone, Copy)]
struct Vp9ColorConfig {
    _ten_or_twelve_bit: bool,
    bit_depth: u8,
    _color_space: u8,
    color_range: bool,
    subsampling_x: bool,
    subsampling_y: bool,
}

#[derive(Debug, Clone, Copy)]
struct Vp9FrameHeader {
    profile: u8,
    show_existing_frame: bool,
    _frame_to_show_map_idx: Option<u8>,
    non_key_frame: bool,
    _show_frame: bool,
    _error_resilient_mode: bool,
    color_config: Option<Vp9ColorConfig>,
    frame_size: Option<Vp9FrameSize>,
}

impl Vp9FrameHeader {
    fn parse(data: &[u8]) -> Option<Self> {
        let mut reader = BitReader::new(data);

        reader.ensure(4).ok()?;
        let frame_marker = reader.read_bits(2)?;
        if frame_marker != 2 {
            return None;
        }

        let profile_low_bit = reader.read_bits(1)? as u8;
        let profile_high_bit = reader.read_bits(1)? as u8;
        let profile = (profile_high_bit << 1) | profile_low_bit;

        if profile == 3 {
            reader.ensure(1).ok()?;
            reader.skip_bits(1)?;
        }

        let show_existing_frame = reader.read_flag()?;
        let mut frame_to_show_map_idx = None;
        if show_existing_frame {
            frame_to_show_map_idx = Some(reader.read_bits(3)? as u8);
            return Some(Self {
                profile,
                show_existing_frame,
                _frame_to_show_map_idx: frame_to_show_map_idx,
                non_key_frame: true,
                _show_frame: true,
                _error_resilient_mode: false,
                color_config: None,
                frame_size: None,
            });
        }

        reader.ensure(3).ok()?;
        let non_key_frame = reader.read_flag()?;
        let show_frame = reader.read_flag()?;
        let error_resilient_mode = reader.read_flag()?;

        let mut color_config = None;
        let mut frame_size = None;

        if !non_key_frame {
            reader.ensure(24).ok()?;
            let sync0 = reader.read_bits(8)? as u8;
            let sync1 = reader.read_bits(8)? as u8;
            let sync2 = reader.read_bits(8)? as u8;
            if sync0 != 0x49 || sync1 != 0x83 || sync2 != 0x42 {
                return None;
            }

            color_config = Some(Vp9ColorConfig::parse(profile, &mut reader)?);
            frame_size = Some(Vp9FrameSize::parse(&mut reader)?);
        }

        Some(Self {
            profile,
            show_existing_frame,
            _frame_to_show_map_idx: frame_to_show_map_idx,
            non_key_frame,
            _show_frame: show_frame,
            _error_resilient_mode: error_resilient_mode,
            color_config,
            frame_size,
        })
    }

    fn is_keyframe(&self) -> bool {
        !self.show_existing_frame && !self.non_key_frame
    }
}

impl Vp9ColorConfig {
    fn parse(profile: u8, reader: &mut BitReader<'_>) -> Option<Self> {
        let mut ten_or_twelve_bit = false;
        let bit_depth = if profile >= 2 {
            ten_or_twelve_bit = reader.read_flag()?;
            if ten_or_twelve_bit { 12 } else { 10 }
        } else {
            8
        };

        let color_space = reader.read_bits(3)? as u8;
        let mut subsampling_x = true;
        let mut subsampling_y = true;

        let color_range = if color_space != 7 {
            let cr = reader.read_flag()?;
            if profile == 1 || profile == 3 {
                subsampling_x = reader.read_flag()?;
                subsampling_y = reader.read_flag()?;
                reader.skip_bits(1)?;
            }
            cr
        } else {
            if profile == 1 || profile == 3 {
                subsampling_x = false;
                subsampling_y = false;
                reader.skip_bits(1)?;
            }
            true
        };

        Some(Self {
            _ten_or_twelve_bit: ten_or_twelve_bit,
            bit_depth,
            _color_space: color_space,
            color_range,
            subsampling_x,
            subsampling_y,
        })
    }

    fn chroma_sampling(self) -> u8 {
        match (self.subsampling_x, self.subsampling_y) {
            (false, false) => 3,
            (true, false) => 2,
            _ => 1,
        }
    }
}

impl Vp9FrameSize {
    fn parse(reader: &mut BitReader<'_>) -> Option<Self> {
        reader.ensure(32).ok()?;
        let frame_width_minus_one = reader.read_bits(16)? as u16;
        let frame_height_minus_one = reader.read_bits(16)? as u16;
        Some(Self {
            frame_width_minus_one,
            frame_height_minus_one,
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

    fn ensure(&self, bits: usize) -> Result<(), ()> {
        if bits <= self.remaining() {
            Ok(())
        } else {
            Err(())
        }
    }

    fn remaining(&self) -> usize {
        self.bit_len.saturating_sub(self.bit_pos)
    }

    fn read_bits(&mut self, bits: usize) -> Option<u64> {
        if bits == 0 {
            return Some(0);
        }
        if self.ensure(bits).is_err() {
            return None;
        }

        let mut value = 0u64;
        let mut bits_to_read = bits;

        while bits_to_read > 0 {
            let byte_index = self.bit_pos / 8;
            let bit_offset = self.bit_pos % 8;
            let available = 8 - bit_offset;
            let take = available.min(bits_to_read);
            let shift = available - take;
            let mask = ((1u16 << take) - 1) as u8;
            let byte = self.data[byte_index];
            let extracted = ((byte >> shift) & mask) as u64;

            value = (value << take) | extracted;
            self.bit_pos += take;
            bits_to_read -= take;
        }

        Some(value)
    }

    fn read_flag(&mut self) -> Option<bool> {
        self.read_bits(1).map(|v| v != 0)
    }

    fn skip_bits(&mut self, bits: usize) -> Option<()> {
        self.read_bits(bits).map(|_| ())
    }
}

/// Assemble WebRTC RTP (VP9) packets into a complete VP9 frame.
const MAX_FRAME_SIZE: usize = 2 * 1024 * 1024;

pub struct Vp9RtpParser {
    depacketizer: Vp9Packet,
    fragments: Vec<Bytes>,
    fragments_size: usize,
    fragment_next_seq: Option<u16>,
    first_packet_received: bool,
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
            fragments: Vec::new(),
            fragments_size: 0,
            fragment_next_seq: None,
            first_packet_received: false,
        }
    }

    pub fn push_packet(&mut self, pkt: &Packet) -> Result<Option<BytesMut>> {
        let payload = match self.depacketizer.depacketize(&pkt.payload) {
            Ok(payload) => payload,
            Err(err) => {
                self.reset_fragments();
                return Err(anyhow!(err));
            }
        };

        let is_begin = self.depacketizer.b;
        let is_end = self.depacketizer.e;

        if is_begin {
            self.reset_fragments();
            self.first_packet_received = true;

            if !is_end {
                self.fragments_size = payload.len();
                self.fragments.push(payload.clone());
                self.fragment_next_seq = Some(pkt.header.sequence_number.wrapping_add(1));
                return Ok(None);
            }

            return Ok(Some(BytesMut::from(payload.as_ref())));
        }

        if self.fragments_size == 0 {
            if !self.first_packet_received {
                return Ok(None);
            }

            return Ok(None);
        }

        if let Some(expected) = self.fragment_next_seq
            && pkt.header.sequence_number != expected
        {
            self.reset_fragments();
            return Ok(None);
        }

        self.fragment_next_seq = Some(pkt.header.sequence_number.wrapping_add(1));
        self.fragments_size += payload.len();

        if self.fragments_size > MAX_FRAME_SIZE {
            self.reset_fragments();
            return Ok(None);
        }

        self.fragments.push(payload.clone());

        if is_end {
            let mut out = BytesMut::with_capacity(self.fragments_size);
            for fragment in self.fragments.drain(..) {
                out.extend_from_slice(fragment.as_ref());
            }
            self.fragments_size = 0;
            self.fragment_next_seq = None;
            return Ok(Some(out));
        }

        Ok(None)
    }
}

impl Vp9RtpParser {
    fn reset_fragments(&mut self) {
        self.fragments.clear();
        self.fragments_size = 0;
        self.fragment_next_seq = None;
    }
}

impl crate::recorder::codec::RtpParser for Vp9RtpParser {
    type Output = BytesMut;
    fn push_packet(&mut self, pkt: &Packet) -> Result<Option<Self::Output>> {
        Vp9RtpParser::push_packet(self, pkt)
    }
}
