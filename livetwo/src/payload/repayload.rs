use bytes::Bytes;
use tracing::{error, trace};
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
}

impl RePayload for RePayloadCodec {
    fn payload(&mut self, packet: Packet) -> Vec<Packet> {
        self.base.verify_sequence_number(&packet);

        match self.decoder.depacketize(&packet.payload) {
            Ok(data) => {
                if let Some(ref mut processor) = self.h264_processor
                    && !processor.has_params()
                {
                    processor.extract_params(&data);
                }
                if let Some(ref mut processor) = self.h265_processor
                    && !processor.has_params()
                {
                    processor.extract_params(&data);
                }

                self.base.buffer.push(data);
            }
            Err(e) => {
                error!("Depacketize error: {}", e);
            }
        }

        if packet.header.marker {
            self.frame_count += 1;

            let combined_data = Bytes::from(self.base.buffer.concat());

            let data_with_params = if let Some(ref processor) = self.h264_processor {
                Bytes::from(processor.inject_params(&combined_data))
            } else if let Some(ref processor) = self.h265_processor {
                Bytes::from(processor.inject_params(&combined_data))
            } else {
                combined_data
            };

            let packets = match self.encoder.payload(RTP_OUTBOUND_MTU, &data_with_params) {
                Ok(payloads) => {
                    let length = payloads.len();
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
