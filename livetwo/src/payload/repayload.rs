use bytes::Bytes;
use tracing::{debug, error, trace, warn};
use webrtc::{
    api::media_engine::*,
    rtp::{
        codecs::*,
        packet::Packet,
        packetizer::{Depacketizer, Payloader},
    },
};

use super::{H264Processor, H265Processor};
use crate::payload::RTP_OUTBOUND_MTU;

const NAL_UNIT_TYPE_MASK: u8 = 0x3F;
const FU_START_BITMASK: u8 = 0x80;
const FU_END_BITMASK: u8 = 0x40;

const START_CODE_3: [u8; 3] = [0, 0, 1];
const START_CODE_4: [u8; 4] = [0, 0, 0, 1];

const H265_NAL_TYPE_FU: u8 = 49;
const H265_NAL_TYPE_AP: u8 = 48;

pub trait RePayload {
    fn payload(&mut self, packet: Packet) -> Vec<Packet>;
    fn set_h264_params(&mut self, sps: Vec<u8>, pps: Vec<u8>);
    fn set_h265_params(&mut self, vps: Vec<u8>, sps: Vec<u8>, pps: Vec<u8>);
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
    fn payload(&mut self, packet: Packet) -> Vec<Packet> {
        vec![packet]
    }

    fn set_h264_params(&mut self, _sps: Vec<u8>, _pps: Vec<u8>) {}

    fn set_h265_params(&mut self, _vps: Vec<u8>, _sps: Vec<u8>, _pps: Vec<u8>) {}
}

struct RePayloadBase {
    buffer: Vec<Bytes>,
    sequence_number: u16,
    src_sequence_number: u16,
}

impl RePayloadBase {
    fn new() -> Self {
        Self {
            buffer: Vec::new(),
            sequence_number: 0,
            src_sequence_number: 0,
        }
    }

    fn verify_sequence_number(&mut self, packet: &Packet) {
        if self.src_sequence_number.wrapping_add(1) != packet.header.sequence_number
            && self.src_sequence_number != 0
        {
            error!(
                "Should received sequence: {}. But received sequence: {}",
                self.src_sequence_number.wrapping_add(1),
                packet.header.sequence_number
            );
        }
        self.src_sequence_number = packet.header.sequence_number;
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

    fn convert_h265_to_annex_b(&self, data: &[u8]) -> Vec<u8> {
        if data.is_empty() {
            return Vec::new();
        }

        let first_byte = data[0];
        let nal_type = (first_byte >> 1) & NAL_UNIT_TYPE_MASK;

        debug!("H.265 NAL type: {}", nal_type);

        if nal_type <= 31 {
            debug!("Converting single NAL to Annex B");
            let mut result = Vec::with_capacity(4 + data.len());
            result.extend_from_slice(&START_CODE_4);
            result.extend_from_slice(data);
            return result;
        }

        if nal_type == H265_NAL_TYPE_AP && data.len() > 2 {
            debug!("Converting AP to Annex B");
            return self.convert_h265_ap_to_annex_b(&data[2..]);
        }

        if nal_type == H265_NAL_TYPE_FU {
            error!("FU should not reach convert_h265_to_annex_b!");
        }

        warn!("Unknown H.265 NAL type {}, adding start code", nal_type);
        let mut result = Vec::with_capacity(4 + data.len());
        result.extend_from_slice(&START_CODE_4);
        result.extend_from_slice(data);
        result
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

    fn payload_h265_manually(
        &mut self,
        data: &[u8],
        original_header: &webrtc::rtp::header::Header,
    ) -> Vec<Packet> {
        const MAX_PAYLOAD_SIZE: usize = RTP_OUTBOUND_MTU - 12;

        let mut packets = Vec::new();
        let mut offset = 0;

        while offset < data.len() {
            if offset + 4 > data.len() {
                break;
            }

            let start_code_len = if data[offset..].starts_with(&START_CODE_4) {
                4
            } else if data[offset..].starts_with(&START_CODE_3) {
                3
            } else {
                offset += 1;
                continue;
            };

            let nal_start = offset + start_code_len;

            let mut nal_end = nal_start + 1;
            while nal_end + 3 < data.len() {
                if data[nal_end..].starts_with(&START_CODE_4)
                    || data[nal_end..].starts_with(&START_CODE_3)
                {
                    break;
                }
                nal_end += 1;
            }
            if nal_end >= data.len() - 3 {
                nal_end = data.len();
            }

            let nal_unit = &data[nal_start..nal_end];

            if nal_unit.len() < 2 {
                offset = nal_end;
                continue;
            }

            let nal_header = &nal_unit[0..2];
            let nal_type = (nal_header[0] >> 1) & NAL_UNIT_TYPE_MASK;

            debug!(
                "H.265 NAL unit - type={}, size={} bytes",
                nal_type,
                nal_unit.len()
            );

            // If NAL unit fits in MTU, send as Single NAL Unit
            if nal_unit.len() <= MAX_PAYLOAD_SIZE {
                let mut header = original_header.clone();
                header.sequence_number = self.base.sequence_number;
                header.marker = nal_end >= data.len();
                self.base.sequence_number = self.base.sequence_number.wrapping_add(1);

                let marker = header.marker;

                packets.push(Packet {
                    header,
                    payload: Bytes::from(nal_unit.to_vec()),
                });

                debug!(
                    "H.265 Single NAL - type={}, size={}, marker={}",
                    nal_type,
                    nal_unit.len(),
                    marker
                );
            } else {
                // NAL unit too large, fragment using FU
                let fu_payload_data = &nal_unit[2..];
                let mut fu_offset = 0;
                let mut is_first = true;

                while fu_offset < fu_payload_data.len() {
                    let chunk_size = (fu_payload_data.len() - fu_offset).min(MAX_PAYLOAD_SIZE - 3);
                    let is_last = fu_offset + chunk_size >= fu_payload_data.len();

                    let mut fu_header = nal_type;
                    if is_first {
                        fu_header |= FU_START_BITMASK;
                    }
                    if is_last {
                        fu_header |= FU_END_BITMASK;
                    }

                    let mut fu_packet = Vec::with_capacity(3 + chunk_size);
                    fu_packet.push((nal_header[0] & 0x81) | (H265_NAL_TYPE_FU << 1));
                    fu_packet.push(nal_header[1]);
                    fu_packet.push(fu_header);
                    fu_packet
                        .extend_from_slice(&fu_payload_data[fu_offset..fu_offset + chunk_size]);

                    let mut header = original_header.clone();
                    header.sequence_number = self.base.sequence_number;
                    header.marker = is_last && nal_end >= data.len();
                    self.base.sequence_number = self.base.sequence_number.wrapping_add(1);

                    let marker = header.marker;

                    packets.push(Packet {
                        header,
                        payload: Bytes::from(fu_packet),
                    });

                    debug!(
                        "H.265 FU - type={}, S={}, E={}, size={}, marker={}",
                        nal_type, is_first, is_last, chunk_size, marker
                    );

                    fu_offset += chunk_size;
                    is_first = false;
                }
            }

            offset = nal_end;
        }

        debug!("Generated {} H.265 RTP packets", packets.len());
        packets
    }
}

impl RePayload for RePayloadCodec {
    fn payload(&mut self, packet: Packet) -> Vec<Packet> {
        self.base.verify_sequence_number(&packet);

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
                                let mut result = Vec::with_capacity(6 + data.len() - 3);
                                result.extend_from_slice(&START_CODE_4);
                                result.push(reconstructed_byte0);
                                result.push(data[1]);
                                result.extend_from_slice(&data[3..]);
                                self.base.buffer.push(Bytes::from(result));
                                debug!("FU start - added start code and NAL header");
                            } else {
                                self.base.buffer.push(Bytes::from(data[3..].to_vec()));
                                debug!("FU continuation - added payload only");
                            }
                        } else {
                            debug!("Converting non-FU H.265 to Annex B");
                            let converted = Bytes::from(self.convert_h265_to_annex_b(&data));
                            self.base.buffer.push(converted);
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
                return vec![];
            }
        }

        if packet.header.marker {
            self.frame_count += 1;
            let combined_data = Bytes::from(self.base.buffer.concat());

            if combined_data.is_empty() {
                warn!("Empty frame data, skipping");
                self.base.clear_buffer();
                return vec![];
            }
            let data_with_params = if let Some(ref mut processor) = self.h264_processor {
                if !processor.has_params() {
                    processor.extract_params(&combined_data);
                }
                Bytes::from(processor.inject_params(&combined_data))
            } else if let Some(ref mut processor) = self.h265_processor {
                if !processor.has_params() {
                    processor.extract_params(&combined_data);
                }
                Bytes::from(processor.inject_params(&combined_data))
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

            let packets = if self.h265_processor.is_some() {
                self.payload_h265_manually(&final_data, &packet.header)
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
                                self.base.sequence_number =
                                    self.base.sequence_number.wrapping_add(1);
                                Packet { header, payload }
                            })
                            .collect::<Vec<Packet>>()
                    }
                    Err(e) => {
                        error!("Payload error: {}", e);
                        vec![]
                    }
                }
            };

            self.base.clear_buffer();
            packets
        } else {
            vec![]
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
