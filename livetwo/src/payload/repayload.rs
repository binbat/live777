use bytes::{Bytes, BytesMut};
use rtc::peer_connection::configuration::media_engine::*;
use rtc::rtp::{
    codec::*,
    packet::Packet,
    packetizer::{Depacketizer, Payloader},
};
use tracing::{debug, error, trace, warn};

use base64::Engine;

use super::{H264Processor, H265Processor};
use crate::payload::{RTP_OUTBOUND_MTU, payload_annex_b};

const NAL_UNIT_TYPE_MASK: u8 = 0x3F;
const FU_START_BITMASK: u8 = 0x80;
const FU_END_BITMASK: u8 = 0x40;

const START_CODE_3: [u8; 3] = [0, 0, 1];
const START_CODE_4: [u8; 4] = [0, 0, 0, 1];

const H265_NAL_TYPE_FU: u8 = 49;
const H265_NAL_TYPE_AP: u8 = 48;

/// AV1 RTP aggregation-header Y bit: the last OBU in this packet continues
/// into the next packet. See draft-ietf-payload-av1-rtp/sec.
const AV1_Y_MASK: u8 = 0b0100_0000;

pub trait RePayload {
    fn payload(&mut self, packet: &Packet) -> Vec<Packet>;
    fn set_h264_params(&mut self, sps: Vec<u8>, pps: Vec<u8>);
    fn set_h265_params(&mut self, vps: Vec<u8>, sps: Vec<u8>, pps: Vec<u8>);
}

/// Extract a fmtp parameter value without lowercasing, for base64-encoded
/// values (sprop-parameter-sets, sprop-vps, ...).
fn fmtp_param_case_preserving<'a>(fmtp: &'a str, key: &str) -> Option<&'a str> {
    fmtp.split(';').find_map(|part| {
        let (param_key, value) = part.trim().split_once('=')?;
        param_key
            .trim()
            .eq_ignore_ascii_case(key)
            .then_some(value.trim())
    })
}

pub struct Forward;

impl Forward {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Forward {
    fn default() -> Self {
        Self::new()
    }
}

impl RePayload for Forward {
    fn payload(&mut self, packet: &Packet) -> Vec<Packet> {
        vec![packet.clone()]
    }

    fn set_h264_params(&mut self, _sps: Vec<u8>, _pps: Vec<u8>) {}

    fn set_h265_params(&mut self, _vps: Vec<u8>, _sps: Vec<u8>, _pps: Vec<u8>) {}
}

struct RePayloadBase {
    buffer: Vec<Bytes>,
    sequence_number: u16,
    src_sequence_number: u16,
    /// Whether `src_sequence_number` has been initialized. Sequence number 0
    /// is a valid RTP value, so we cannot use `0` as a sentinel for the first
    /// packet.
    has_baseline: bool,
    /// Cumulative count of input sequence gaps, for rate-limited logging.
    gap_count: u64,
}

impl RePayloadBase {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            sequence_number: 0,
            src_sequence_number: 0,
            has_baseline: false,
            gap_count: 0,
        }
    }

    /// Verify RTP sequence-number continuity and update the baseline.
    ///
    /// Returns `true` when the packet's sequence number is the expected
    /// successor to the previous packet (or when it is the first packet seen,
    /// which establishes the baseline). Returns `false` when a gap is
    /// detected, meaning the buffered fragments belong to an incomplete frame
    /// that will never be reconstructable; callers should discard the buffer.
    ///
    /// After a gap, the next packet re-establishes the baseline regardless of
    /// its marker bit, because the previous frame's fragments were discarded.
    fn verify_sequence_number(&mut self, packet: &Packet) -> bool {
        let continuous = if self.has_baseline {
            let expected = self.src_sequence_number.wrapping_add(1);
            expected == packet.header.sequence_number
        } else {
            // First packet: accept any sequence number and establish the baseline.
            true
        };
        if !continuous {
            // A gap is routine on UDP paths (publisher-side loss, multicast
            // delivery, a lagging broadcast): the fragments are dropped and
            // the next packet re-baselines, so this is not an error. Rate-limit
            // the logging the way the WHEP track handler's drop counter does.
            self.gap_count += 1;
            let gaps = self.gap_count;
            if gaps <= 10 || gaps.is_multiple_of(100) {
                warn!(
                    "Input sequence gap: expected {}, received {} (total gaps {})",
                    self.src_sequence_number.wrapping_add(1),
                    packet.header.sequence_number,
                    gaps
                );
            } else {
                debug!(
                    "Input sequence gap: expected {}, received {} (total gaps {})",
                    self.src_sequence_number.wrapping_add(1),
                    packet.header.sequence_number,
                    gaps
                );
            }
            // Reset the baseline so the next packet re-establishes it instead
            // of being misidentified as a second gap against the stale baseline.
            self.has_baseline = false;
        } else {
            self.has_baseline = true;
        }
        self.src_sequence_number = packet.header.sequence_number;
        continuous
    }

    fn clear_buffer(&mut self) {
        self.buffer.clear();
    }
}

pub struct RePayloadCodec {
    base: RePayloadBase,
    encoder: Box<dyn Payloader + Send>,
    decoder: Box<dyn Depacketizer + Send>,
    mime_type: String,
    h264_processor: Option<H264Processor>,
    h265_processor: Option<H265Processor>,
    frame_count: u32,
}

impl RePayloadCodec {
    pub fn new(mime_type: String) -> Self {
        let mime_lc = mime_type.to_ascii_lowercase();
        let is = |candidate: &str| mime_lc == candidate.to_ascii_lowercase();

        let decoder: Box<dyn Depacketizer + Send> = if is(MIME_TYPE_VP8) {
            Box::<vp8::Vp8Packet>::default()
        } else if is(MIME_TYPE_VP9) {
            Box::<vp9::Vp9Packet>::default()
        } else if is(MIME_TYPE_H264) {
            Box::<h264::H264Packet>::default()
        } else if is(MIME_TYPE_HEVC) {
            Box::<h265::H265Packet>::default()
        } else if is(MIME_TYPE_AV1) {
            Box::<av1::Av1Depacketizer>::default()
        } else if is(MIME_TYPE_OPUS) {
            Box::<opus::OpusPacket>::default()
        } else {
            Box::<vp8::Vp8Packet>::default()
        };

        let encoder: Box<dyn Payloader + Send> = if is(MIME_TYPE_VP8) {
            Box::<vp8::Vp8Payloader>::default()
        } else if is(MIME_TYPE_VP9) {
            Box::<vp9::Vp9Payloader>::default()
        } else if is(MIME_TYPE_H264) {
            Box::<h264::H264Payloader>::default()
        } else if is(MIME_TYPE_HEVC) {
            Box::<h265::HevcPayloader>::default()
        } else if is(MIME_TYPE_AV1) {
            Box::<av1::Av1Payloader>::default()
        } else if is(MIME_TYPE_OPUS) {
            Box::<opus::OpusPayloader>::default()
        } else {
            Box::<vp8::Vp8Payloader>::default()
        };

        let h264_processor = if is(MIME_TYPE_H264) {
            Some(H264Processor::new())
        } else {
            None
        };

        let h265_processor = if is(MIME_TYPE_HEVC) {
            Some(H265Processor::new())
        } else {
            None
        };

        Self {
            base: RePayloadBase::new(),
            decoder,
            encoder,
            mime_type,
            h264_processor,
            h265_processor,
            frame_count: 0,
        }
    }

    /// Build a codec for `mime_type`, pre-seeding H264/H265 parameter sets
    /// from an SDP fmtp line (`sprop-parameter-sets` /
    /// `sprop-vps`/`sprop-sps`/`sprop-pps`, base64). No-op for other mime
    /// types or unparseable params.
    pub fn with_sprop_params(mime_type: String, sdp_fmtp_line: &str) -> Self {
        let mut rp = Self::new(mime_type.clone());
        let b64 = base64::engine::general_purpose::STANDARD;
        if mime_type.eq_ignore_ascii_case(MIME_TYPE_H264) {
            if let Some(sprop) = fmtp_param_case_preserving(sdp_fmtp_line, "sprop-parameter-sets") {
                let parts: Vec<&str> = sprop.split(',').collect();
                if parts.len() >= 2
                    && let (Ok(sps), Ok(pps)) =
                        (b64.decode(parts[0].trim()), b64.decode(parts[1].trim()))
                {
                    rp.set_h264_params(sps, pps);
                }
            }
        } else if mime_type.eq_ignore_ascii_case(MIME_TYPE_HEVC) {
            let vps = fmtp_param_case_preserving(sdp_fmtp_line, "sprop-vps");
            let sps = fmtp_param_case_preserving(sdp_fmtp_line, "sprop-sps");
            let pps = fmtp_param_case_preserving(sdp_fmtp_line, "sprop-pps");
            if let (Some(vps), Some(sps), Some(pps)) = (vps, sps, pps)
                && let (Ok(vps), Ok(sps), Ok(pps)) = (
                    b64.decode(vps.trim()),
                    b64.decode(sps.trim()),
                    b64.decode(pps.trim()),
                )
            {
                rp.set_h265_params(vps, sps, pps);
            }
        }
        rp
    }

    fn is_av1(&self) -> bool {
        self.mime_type.eq_ignore_ascii_case(MIME_TYPE_AV1)
    }

    fn convert_h265_to_annex_b(&self, data: &[u8]) -> Vec<u8> {
        if data.is_empty() {
            return Vec::new();
        }

        let first_byte = data[0];
        let nal_type = (first_byte >> 1) & NAL_UNIT_TYPE_MASK;

        debug!("H.265 NAL type: {}", nal_type);

        match nal_type {
            0..=31 => {
                trace!("Single VCL NAL unit");
                let mut result = Vec::with_capacity(4 + data.len());
                result.extend_from_slice(&START_CODE_4);
                result.extend_from_slice(data);
                result
            }

            32..=34 => {
                trace!("Parameter set NAL unit ({})", nal_type);
                let mut result = Vec::with_capacity(4 + data.len());
                result.extend_from_slice(&START_CODE_4);
                result.extend_from_slice(data);
                result
            }

            35..=47 => {
                trace!("Non-VCL NAL unit");
                let mut result = Vec::with_capacity(4 + data.len());
                result.extend_from_slice(&START_CODE_4);
                result.extend_from_slice(data);
                result
            }

            H265_NAL_TYPE_AP if data.len() > 2 => {
                debug!("Converting AP to Annex B");
                self.convert_h265_ap_to_annex_b(&data[2..])
            }

            H265_NAL_TYPE_FU => {
                error!("FU should not reach convert_h265_to_annex_b!");
                Vec::new()
            }

            _ => {
                warn!(
                    "Unknown H.265 NAL type {}, adding start code anyway",
                    nal_type
                );
                let mut result = Vec::with_capacity(4 + data.len());
                result.extend_from_slice(&START_CODE_4);
                result.extend_from_slice(data);
                result
            }
        }
    }

    fn convert_h265_ap_to_annex_b(&self, data: &[u8]) -> Vec<u8> {
        let mut result = Vec::new();
        let mut offset = 0;
        let mut nal_count = 0;

        while offset + 2 <= data.len() {
            let nal_size = ((data[offset] as usize) << 8) | (data[offset + 1] as usize);
            offset += 2;

            if nal_size == 0 || offset + nal_size > data.len() {
                warn!("Invalid AP NAL size: {}", nal_size);
                break;
            }

            nal_count += 1;
            result.extend_from_slice(&START_CODE_4);
            result.extend_from_slice(&data[offset..offset + nal_size]);
            debug!("Converted AP NAL #{}: {} bytes", nal_count, nal_size);

            offset += nal_size;
        }

        debug!(
            "Converted {} NAL units from AP, total {} bytes",
            nal_count,
            result.len()
        );
        result
    }

    fn payload_h265(
        &mut self,
        data: &[u8],
        original_header: &rtc::rtp::header::Header,
    ) -> Vec<Packet> {
        let max_payload_size = RTP_OUTBOUND_MTU - 12;
        let payloads = payload_annex_b(data, max_payload_size);
        let total = payloads.len();

        let mut packets = Vec::with_capacity(total);
        for (i, payload) in payloads.into_iter().enumerate() {
            let mut header = original_header.clone();
            header.sequence_number = self.base.sequence_number;
            header.marker = i == total - 1;
            self.base.sequence_number = self.base.sequence_number.wrapping_add(1);

            debug!(
                "H.265 RTP packet: seq={} marker={} len={}",
                header.sequence_number,
                header.marker,
                payload.len()
            );

            packets.push(Packet { header, payload });
        }

        debug!("Generated {} H.265 RTP packets", packets.len());
        packets
    }
}

impl RePayloadCodec {
    /// Process an inbound RTP packet and return the complete encoded frame when
    /// the marker bit indicates the end of a frame.
    pub fn process(&mut self, packet: &Packet) -> Option<Bytes> {
        let continuous = self.base.verify_sequence_number(packet);
        if !continuous {
            // A gap means the buffered fragments belong to a previous frame that
            // will never be complete; drop them to avoid feeding corrupt data.
            self.base.clear_buffer();
        }

        match self.decoder.depacketize(&packet.payload) {
            Ok(data) => {
                debug!("Depacketized {} bytes", data.len());

                if data.len() >= 3 {
                    let has_start_code =
                        data.starts_with(&START_CODE_4) || data.starts_with(&START_CODE_3);

                    debug!(
                        "First 4 bytes: {:02X?}, has_start_code: {}",
                        &data[..4.min(data.len())],
                        has_start_code
                    );

                    if !has_start_code && self.h265_processor.is_some() {
                        let nal_type = (data[0] >> 1) & NAL_UNIT_TYPE_MASK;

                        if nal_type == H265_NAL_TYPE_FU && data.len() >= 3 {
                            let fu_header = data[2];
                            let start_bit = (fu_header & FU_START_BITMASK) != 0;
                            let end_bit = (fu_header & FU_END_BITMASK) != 0;
                            let fu_type = fu_header & NAL_UNIT_TYPE_MASK;

                            debug!(
                                "FU packet - S={}, E={}, type={}",
                                start_bit, end_bit, fu_type
                            );

                            if start_bit {
                                let reconstructed_byte0 = (data[0] & 0x81) | (fu_type << 1);
                                let mut result = BytesMut::with_capacity(6 + data.len() - 3);
                                result.extend_from_slice(&START_CODE_4);
                                result.extend_from_slice(&[reconstructed_byte0, data[1]]);
                                result.extend_from_slice(&data[3..]);
                                self.base.buffer.push(result.freeze());
                                debug!("FU start - added start code and NAL header");
                            } else if self.base.buffer.is_empty() {
                                // FU continuation extends a prior FU-start. An empty
                                // buffer means that start was lost (e.g. a sequence
                                // gap dropped it), so this fragment is an orphan:
                                // appending its raw bytes would assemble corrupt
                                // Annex-B. Drop it and keep skipping until the next
                                // FU-start or self-contained NAL resyncs the stream.
                                debug!(
                                    "FU continuation with empty buffer, dropping orphan fragment"
                                );
                            } else {
                                self.base.buffer.push(Bytes::copy_from_slice(&data[3..]));
                                debug!("FU continuation - added payload only");
                            }
                        } else {
                            debug!("Converting non-FU H.265 to Annex B");
                            let converted = self.convert_h265_to_annex_b(&data);
                            self.base.buffer.push(Bytes::from(converted));
                        }
                    } else {
                        self.base.buffer.push(data);
                    }
                } else {
                    self.base.buffer.push(data);
                }
            }
            Err(e) => {
                error!("Depacketize error: {}", e);
                return None;
            }
        }

        if packet.header.marker {
            // AV1: a marker bit with Y=1 means the last OBU continues into a
            // packet that will never arrive (marker ends the temporal unit).
            // The depacketizer has buffered that fragment internally, so the
            // assembled data would be truncated. Drop the unit and recreate
            // the depacketizer to clear the held-back fragment — mirrors
            // liveion's Av1Assembler, which resets and errors on this case.
            if self.is_av1() && !packet.payload.is_empty() && (packet.payload[0] & AV1_Y_MASK) != 0
            {
                warn!(
                    "AV1 RTP marker set but last OBU continues (Y=1); dropping malformed temporal unit"
                );
                self.base.clear_buffer();
                self.decoder = Box::<av1::Av1Depacketizer>::default();
                return None;
            }

            self.frame_count += 1;
            let total_len: usize = self.base.buffer.iter().map(|b| b.len()).sum();
            let mut combined = BytesMut::with_capacity(total_len);
            for chunk in &self.base.buffer {
                combined.extend_from_slice(chunk);
            }
            let combined_data = combined.freeze();

            if combined_data.is_empty() {
                warn!("Empty frame data, skipping");
                self.base.clear_buffer();
                return None;
            }
            let data_with_params = if let Some(ref mut processor) = self.h264_processor {
                if !processor.has_params() {
                    processor.extract_params(&combined_data);
                }
                let injected = processor.inject_params(&combined_data);
                // Only pay the Bytes allocation when injection actually
                // added parameter sets; otherwise reuse the original.
                if injected.len() > combined_data.len() {
                    Bytes::from(injected)
                } else {
                    combined_data.clone()
                }
            } else if let Some(ref mut processor) = self.h265_processor {
                if !processor.has_params() {
                    processor.extract_params(&combined_data);
                }
                let injected = processor.inject_params(&combined_data);
                if injected.len() > combined_data.len() {
                    Bytes::from(injected)
                } else {
                    combined_data.clone()
                }
            } else {
                combined_data.clone()
            };

            debug!(
                "Data after param injection: {} bytes (diff: {})",
                data_with_params.len(),
                data_with_params.len() as i64 - combined_data.len() as i64
            );

            let final_data = if data_with_params.len() > combined_data.len() {
                debug!("Sending frame with injected params");
                data_with_params
            } else {
                debug!("Sending frame without params (frame #{})", self.frame_count);
                combined_data
            };

            self.base.clear_buffer();
            Some(final_data)
        } else {
            None
        }
    }
}

impl RePayload for RePayloadCodec {
    fn payload(&mut self, packet: &Packet) -> Vec<Packet> {
        let final_data = match self.process(packet) {
            Some(data) => data,
            None => return vec![],
        };

        if self.h265_processor.is_some() {
            self.payload_h265(&final_data, &packet.header)
        } else {
            match self.encoder.payload(RTP_OUTBOUND_MTU, &final_data) {
                Ok(payloads) => {
                    let length = payloads.len();
                    debug!("Generated {} output packets", length);

                    payloads
                        .into_iter()
                        .enumerate()
                        .map(|(i, payload)| {
                            let mut header = packet.header.clone();
                            header.sequence_number = self.base.sequence_number;
                            header.marker = i == length - 1;
                            self.base.sequence_number = self.base.sequence_number.wrapping_add(1);
                            Packet { header, payload }
                        })
                        .collect::<Vec<Packet>>()
                }
                Err(e) => {
                    error!("Payload error: {}", e);
                    vec![]
                }
            }
        }
    }

    fn set_h264_params(&mut self, sps: Vec<u8>, pps: Vec<u8>) {
        if self.mime_type.eq_ignore_ascii_case(MIME_TYPE_H264) {
            trace!(
                "Setting H.264 params from SDP - SPS: {} bytes, PPS: {} bytes",
                sps.len(),
                pps.len()
            );
            if let Some(ref mut processor) = self.h264_processor {
                processor.set_params(sps, pps);
            }
        }
    }

    fn set_h265_params(&mut self, vps: Vec<u8>, sps: Vec<u8>, pps: Vec<u8>) {
        if self.mime_type.eq_ignore_ascii_case(MIME_TYPE_HEVC) {
            trace!(
                "Setting H.265 params - VPS: {} bytes, SPS: {} bytes, PPS: {} bytes",
                vps.len(),
                sps.len(),
                pps.len()
            );
            if let Some(ref mut processor) = self.h265_processor {
                processor.set_params(vps, sps, pps);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use bytes::Bytes;

    fn av1_packet(marker: bool, aggregation_header: u8) -> Packet {
        // Aggregation header + a minimal Frame OBU (type 6, no size field).
        let payload = Bytes::from(vec![aggregation_header, 0x30, 0x01, 0x02, 0x03]);
        Packet {
            header: rtc::rtp::header::Header {
                version: 2,
                marker,
                payload_type: 96,
                sequence_number: 1,
                timestamp: 1000,
                ssrc: 0x1234,
                ..Default::default()
            },
            payload,
        }
    }

    /// `marker=1` with `Y=0` is a normal end of temporal unit: the frame is
    /// emitted.
    #[test]
    fn av1_marker_without_y_emits_frame() {
        let mut codec = RePayloadCodec::new(MIME_TYPE_AV1.to_owned());
        // 0x10 = W=1, Y=0, Z=0, N=0
        let pkt = av1_packet(true, 0x10);
        let frame = codec.process(&pkt);
        assert!(frame.is_some(), "normal marker frame should be emitted");
    }

    /// `marker=1` with `Y=1` is malformed (last OBU continues but no packet
    /// follows): the truncated temporal unit must be dropped, not emitted.
    #[test]
    fn av1_marker_with_y_drops_malformed_unit() {
        let mut codec = RePayloadCodec::new(MIME_TYPE_AV1.to_owned());
        // 0x50 = W=1, Y=1, Z=0, N=0
        let pkt = av1_packet(true, 0x50);
        let frame = codec.process(&pkt);
        assert!(
            frame.is_none(),
            "marker + Y=1 must drop the malformed temporal unit"
        );
    }

    fn h264_packet(marker: bool, seq: u16, nal: &[u8]) -> Packet {
        Packet {
            header: rtc::rtp::header::Header {
                version: 2,
                marker,
                payload_type: 96,
                sequence_number: seq,
                timestamp: 1000,
                ssrc: 0x1234,
                ..Default::default()
            },
            payload: Bytes::from(nal.to_vec()),
        }
    }

    /// Senders that write SPS/PPS inline only on their own cadence (robot
    /// cameras, raw-RTP appliances): the repayloader must cache the first
    /// inline sets and re-inject them ahead of every later IDR, so a
    /// subscriber joining mid-GOP decodes from the next IDR instead of
    /// waiting for the sender's parameter-set cadence (live777#464).
    #[test]
    fn h264_params_learned_from_stream_injected_before_later_idr() {
        const SPS: [u8; 5] = [0x67, 0x42, 0x00, 0x1f, 0xaa];
        const PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];
        const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x21];
        const SLICE: [u8; 3] = [0x41, 0x9a, 0x22];

        let mut codec = RePayloadCodec::new(MIME_TYPE_H264.to_owned());

        // Frame 1: SPS + PPS + IDR inline — establishes the cache.
        assert!(codec.payload(&h264_packet(false, 1, &SPS)).is_empty());
        assert!(codec.payload(&h264_packet(false, 2, &PPS)).is_empty());
        let frame1 = codec.payload(&h264_packet(true, 3, &IDR));
        assert!(!frame1.is_empty(), "frame 1 must be emitted");

        // Frame 2: a plain non-IDR slice passes through untouched.
        let frame2 = codec.payload(&h264_packet(true, 4, &SLICE));
        let frame2_payload: Vec<u8> = frame2
            .iter()
            .flat_map(|p| p.payload.iter().copied())
            .collect();
        assert_eq!(frame2_payload, SLICE);

        // Frame 3: an IDR without inline parameter sets — the cached
        // SPS/PPS must be injected ahead of it.
        let frame3 = codec.payload(&h264_packet(true, 5, &IDR));
        let frame3_payload: Vec<u8> = frame3
            .iter()
            .flat_map(|p| p.payload.iter().copied())
            .collect();
        let find = |needle: &[u8]| {
            frame3_payload
                .windows(needle.len())
                .position(|w| w == needle)
        };
        let (sps_at, pps_at, idr_at) = (find(&SPS), find(&PPS), find(&IDR));
        assert!(
            sps_at.is_some() && pps_at.is_some(),
            "cached SPS/PPS must be injected: {frame3_payload:02x?}"
        );
        assert!(
            sps_at < idr_at && pps_at < idr_at,
            "parameter sets must precede the IDR: {frame3_payload:02x?}"
        );

        // Outbound sequence numbers stay continuous across the injection.
        let seqs: Vec<u16> = [&frame1, &frame2, &frame3]
            .into_iter()
            .flatten()
            .map(|p| p.header.sequence_number)
            .collect();
        let expected: Vec<u16> = (0..seqs.len() as u16).collect();
        assert_eq!(seqs, expected);
    }

    fn concatenated_payload(packets: &[Packet]) -> Vec<u8> {
        packets
            .iter()
            .flat_map(|p| p.payload.iter().copied())
            .collect()
    }

    /// A sequence gap drops the incomplete frame's buffered fragments and
    /// re-baselines: the next packet starts a fresh frame. Repeated gaps
    /// (past the rate-limited log's warn window) keep re-baselining.
    #[test]
    fn sequence_gap_drops_buffer_and_rebaselines() {
        const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x21];
        const SLICE_A: [u8; 3] = [0x41, 0x9a, 0x22];
        const SLICE_B: [u8; 3] = [0x41, 0x77, 0x77];

        let mut codec = RePayloadCodec::new(MIME_TYPE_H264.to_owned());

        // Frame fragment (no marker), then a gap (1 → 5): the buffered
        // pre-gap fragment is dropped; the gap packet re-baselines and starts
        // the next frame.
        assert!(codec.payload(&h264_packet(false, 1, &IDR)).is_empty());
        assert!(codec.payload(&h264_packet(false, 5, &SLICE_A)).is_empty());
        let out = codec.payload(&h264_packet(true, 6, &SLICE_B));
        assert!(!out.is_empty(), "the frame after the gap must be emitted");
        let payload = concatenated_payload(&out);
        assert!(
            !payload.windows(IDR.len()).any(|w| w == IDR),
            "the pre-gap fragment must not leak: {payload:02x?}"
        );
        assert!(
            payload.windows(SLICE_A.len()).any(|w| w == SLICE_A)
                && payload.windows(SLICE_B.len()).any(|w| w == SLICE_B),
            "the frame is the post-gap packets: {payload:02x?}"
        );

        // Past the rate-limited log's warn window (gaps 11+), gaps still
        // re-baseline.
        for i in 0..12u16 {
            codec.payload(&h264_packet(false, 10 + i * 2, &SLICE_A));
        }
        assert!(!codec.payload(&h264_packet(true, 100, &IDR)).is_empty());
    }

    /// A constructor seeded from the SDP fmtp's `sprop-parameter-sets`
    /// injects those parameter sets ahead of the very first IDR, so a
    /// receiver joining from the start (or mid-GOP, after the sender's own
    /// inline cadence) always has SPS/PPS available.
    #[test]
    fn with_sprop_params_seeds_h264_from_fmtp() {
        const SPS: [u8; 5] = [0x67, 0x42, 0x00, 0x1f, 0xaa];
        const PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];
        const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x21];

        let b64 = base64::engine::general_purpose::STANDARD;
        let fmtp = format!(
            "profile-level-id=42001f;packetization-mode=1;sprop-parameter-sets={},{}",
            b64.encode(SPS),
            b64.encode(PPS)
        );

        let mut codec = RePayloadCodec::with_sprop_params(MIME_TYPE_H264.to_owned(), &fmtp);
        let out = codec.payload(&h264_packet(true, 1, &IDR));
        let payload = concatenated_payload(&out);
        let find = |needle: &[u8]| payload.windows(needle.len()).position(|w| w == needle);
        let (sps_at, pps_at, idr_at) = (find(&SPS), find(&PPS), find(&IDR));
        assert!(
            sps_at.is_some() && pps_at.is_some(),
            "seeded SPS/PPS must be injected: {payload:02x?}"
        );
        assert!(sps_at < idr_at && pps_at < idr_at);
    }

    #[test]
    fn with_sprop_params_h264_ignores_unparseable_params() {
        const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x21];

        // Invalid base64.
        let mut codec = RePayloadCodec::with_sprop_params(
            MIME_TYPE_H264.to_owned(),
            "sprop-parameter-sets=!!!not-base64!!!,aM4yyA==",
        );
        let out = codec.payload(&h264_packet(true, 1, &IDR));
        assert_eq!(concatenated_payload(&out), IDR);

        // Missing the PPS segment.
        let mut codec = RePayloadCodec::with_sprop_params(
            MIME_TYPE_H264.to_owned(),
            "sprop-parameter-sets=Z0IAH5WoFAFuQA==",
        );
        let out = codec.payload(&h264_packet(true, 1, &IDR));
        assert_eq!(concatenated_payload(&out), IDR);

        // No sprop at all.
        let mut codec =
            RePayloadCodec::with_sprop_params(MIME_TYPE_H264.to_owned(), "packetization-mode=1");
        let out = codec.payload(&h264_packet(true, 1, &IDR));
        assert_eq!(concatenated_payload(&out), IDR);
    }

    #[test]
    fn with_sprop_params_seeds_h265_from_fmtp() {
        const VPS: [u8; 4] = [0x40, 0x01, 0x0c, 0x01];
        const SPS: [u8; 5] = [0x42, 0x01, 0x01, 0x01, 0x60];
        const PPS: [u8; 3] = [0x44, 0x01, 0xc0];
        // IDR_W_RADL (type 19): byte0 = (19 << 1) | F.
        const IDR: [u8; 4] = [0x26, 0x01, 0xaf, 0x08];

        let b64 = base64::engine::general_purpose::STANDARD;
        let fmtp = format!(
            "profile-id=1;sprop-vps={};sprop-sps={};sprop-pps={}",
            b64.encode(VPS),
            b64.encode(SPS),
            b64.encode(PPS)
        );

        let mut codec = RePayloadCodec::with_sprop_params(MIME_TYPE_HEVC.to_owned(), &fmtp);
        let out = codec.payload(&h264_packet(true, 1, &IDR));
        let payload = concatenated_payload(&out);
        let find = |needle: &[u8]| payload.windows(needle.len()).position(|w| w == needle);
        let (vps_at, sps_at, pps_at, idr_at) = (find(&VPS), find(&SPS), find(&PPS), find(&IDR));
        assert!(
            vps_at.is_some() && sps_at.is_some() && pps_at.is_some(),
            "seeded VPS/SPS/PPS must be injected: {payload:02x?}"
        );
        assert!(vps_at < idr_at && sps_at < idr_at && pps_at < idr_at);
    }

    #[test]
    fn with_sprop_params_is_noop_for_other_mime_types() {
        const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x21];
        // Z0IAH5WoFAFuQA== / aM4yyA== (the SPS/PPS from the SDP-source tests).
        const SPS: [u8; 9] = [0x67, 0x42, 0x00, 0x1f, 0x95, 0xa8, 0x14, 0x01, 0x6e];
        const PPS: [u8; 4] = [0x68, 0xce, 0x32, 0xc8];

        // Not an H264/H265 mime: the sprop keys are ignored, the codec
        // works as a plain repayloader and the seeded bytes never appear
        // in its output.
        let mut codec = RePayloadCodec::with_sprop_params(
            MIME_TYPE_VP8.to_owned(),
            "sprop-parameter-sets=Z0IAH5WoFAFuQA==,aM4yyA==",
        );
        let out = codec.payload(&h264_packet(true, 1, &IDR));
        let payload = concatenated_payload(&out);
        let contains = |needle: &[u8]| payload.windows(needle.len()).any(|w| w == needle);
        assert!(
            !contains(&SPS) && !contains(&PPS),
            "non-H264/H264 mime must not seed parameter sets: {payload:02x?}"
        );
    }
}
