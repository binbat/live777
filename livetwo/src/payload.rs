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

/// https://github.com/webrtc-rs/webrtc/blob/dcfefd7b48dc2bb9ecf50ea66c304f62719a6c4a/webrtc/src/track/mod.rs#L10C12-L10C49
/// https://github.com/binbat/live777/issues/1200
/// WebRTC Build-in RTP must less 1200
const RTP_OUTBOUND_MTU: usize = 1200;

pub trait RePayload {
    fn payload(&mut self, packet: Packet) -> Vec<Packet>;
    fn set_h264_params(&mut self, sps: Vec<u8>, pps: Vec<u8>);
}

pub(crate) struct Forward {}

impl Forward {
    pub fn new() -> Forward {
        Forward {}
    }
}

impl RePayload for Forward {
    fn payload(&mut self, packet: Packet) -> Vec<Packet> {
        vec![packet]
    }

    fn set_h264_params(&mut self, _sps: Vec<u8>, _pps: Vec<u8>) {}
}

pub(crate) struct RePayloadBase {
    buffer: Vec<Bytes>,
    sequence_number: u16,
    src_sequence_number: u16,
}

impl RePayloadBase {
    pub fn new() -> RePayloadBase {
        RePayloadBase {
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
                self.src_sequence_number + 1,
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
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    frame_count: u32,
}

impl RePayloadCodec {
    pub fn new(mime_type: String) -> RePayloadCodec {
        let mime_lc = mime_type.to_ascii_lowercase();
        let is = |candidate: &str| mime_lc == candidate.to_ascii_lowercase();

        let decoder: Box<dyn Depacketizer + Send> = if is(MIME_TYPE_VP8) {
            Box::default() as Box<vp8::Vp8Packet>
        } else if is(MIME_TYPE_VP9) {
            Box::default() as Box<vp9::Vp9Packet>
        } else if is(MIME_TYPE_H264) {
            Box::default() as Box<h264::H264Packet>
        } else if is(MIME_TYPE_OPUS) {
            Box::default() as Box<opus::OpusPacket>
        } else {
            Box::default() as Box<vp8::Vp8Packet>
        };

        let encoder: Box<dyn Payloader + Send> = if is(MIME_TYPE_VP8) {
            Box::default() as Box<vp8::Vp8Payloader>
        } else if is(MIME_TYPE_VP9) {
            Box::default() as Box<vp9::Vp9Payloader>
        } else if is(MIME_TYPE_H264) {
            Box::default() as Box<h264::H264Payloader>
        } else if is(MIME_TYPE_OPUS) {
            Box::default() as Box<opus::OpusPayloader>
        } else {
            Box::default() as Box<vp8::Vp8Payloader>
        };

        RePayloadCodec {
            base: RePayloadBase::new(),
            decoder,
            encoder,
            mime_type,
            sps: None,
            pps: None,
            frame_count: 0,
        }
    }

    fn idr_frame(&self, data: &[u8]) -> bool {
        if self.mime_type.to_lowercase() != "video/h264" {
            return false;
        }

        for i in 0..data.len().saturating_sub(4) {
            if data[i] == 0 && data[i + 1] == 0 {
                let nal_start = if data[i + 2] == 0 && data[i + 3] == 1 {
                    i + 4
                } else if data[i + 2] == 1 {
                    i + 3
                } else {
                    continue;
                };

                if nal_start < data.len() {
                    let nal_type = data[nal_start] & 0x1F;
                    if nal_type == 5 {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn sps_pps(&self, data: &[u8]) -> bool {
        if self.mime_type.to_lowercase() != "video/h264" {
            return false;
        }

        let mut has_sps = false;
        let mut has_pps = false;

        for i in 0..data.len().saturating_sub(4) {
            if data[i] == 0 && data[i + 1] == 0 {
                let nal_start = if data[i + 2] == 0 && data[i + 3] == 1 {
                    i + 4
                } else if data[i + 2] == 1 {
                    i + 3
                } else {
                    continue;
                };

                if nal_start < data.len() {
                    let nal_type = data[nal_start] & 0x1F;
                    if nal_type == 7 {
                        has_sps = true;
                    } else if nal_type == 8 {
                        has_pps = true;
                    }
                }
            }
        }

        has_sps && has_pps
    }

    fn extract_params(&mut self, data: &[u8]) {
        if self.mime_type.to_lowercase() != "video/h264" {
            return;
        }

        let mut i = 0;
        while i + 4 < data.len() {
            if data[i] == 0 && data[i + 1] == 0 {
                let nal_start = if data[i + 2] == 0 && data[i + 3] == 1 {
                    i + 4
                } else if data[i + 2] == 1 {
                    i + 3
                } else {
                    i += 1;
                    continue;
                };

                if nal_start >= data.len() {
                    break;
                }

                let nal_type = data[nal_start] & 0x1F;

                let mut nal_end = nal_start + 1;
                while nal_end + 3 < data.len() {
                    if (data[nal_end] == 0 && data[nal_end + 1] == 0 && data[nal_end + 2] == 1)
                        || (data[nal_end] == 0
                            && data[nal_end + 1] == 0
                            && data[nal_end + 2] == 0
                            && data[nal_end + 3] == 1)
                    {
                        break;
                    }
                    nal_end += 1;
                }
                if nal_end >= data.len() - 3 {
                    nal_end = data.len();
                }

                match nal_type {
                    7 => {
                        if self.sps.is_none() {
                            self.sps = Some(data[nal_start..nal_end].to_vec());
                            trace!("Extracted SPS from stream: {} bytes", nal_end - nal_start);
                        }
                    }
                    8 => {
                        if self.pps.is_none() {
                            self.pps = Some(data[nal_start..nal_end].to_vec());
                            trace!("Extracted PPS from stream: {} bytes", nal_end - nal_start);
                        }
                    }
                    _ => {}
                }

                i = nal_end;
            } else {
                i += 1;
            }
        }
    }

    fn inject_params(&mut self, data: &[u8]) -> Vec<u8> {
        if self.mime_type.to_lowercase() != "video/h264" {
            return data.to_vec();
        }

        let is_idr = self.idr_frame(data);

        if !is_idr {
            return data.to_vec();
        }

        if self.sps_pps(data) {
            trace!(
                "Frame {} already has SPS/PPS, skipping injection",
                self.frame_count
            );
            return data.to_vec();
        }

        if self.sps.is_none() || self.pps.is_none() {
            trace!(
                "No cached SPS/PPS for frame {}, cannot inject",
                self.frame_count
            );
            return data.to_vec();
        }

        let mut result = Vec::new();

        result.extend_from_slice(&[0, 0, 0, 1]);
        result.extend_from_slice(self.sps.as_ref().unwrap());

        result.extend_from_slice(&[0, 0, 0, 1]);
        result.extend_from_slice(self.pps.as_ref().unwrap());

        result.extend_from_slice(data);

        trace!(
            "Injected SPS/PPS at IDR frame {} (SPS: {} bytes, PPS: {} bytes, total: {} -> {})",
            self.frame_count,
            self.sps.as_ref().unwrap().len(),
            self.pps.as_ref().unwrap().len(),
            data.len(),
            result.len()
        );

        result
    }
}

impl RePayload for RePayloadCodec {
    fn payload(&mut self, packet: Packet) -> Vec<Packet> {
        self.base.verify_sequence_number(&packet);

        match self.decoder.depacketize(&packet.payload) {
            Ok(data) => {
                if self.sps.is_none() || self.pps.is_none() {
                    self.extract_params(&data);
                }
                self.base.buffer.push(data);
            }
            Err(e) => {
                error!("Depacketize error: {}", e);
            }
        };

        if packet.header.marker {
            self.frame_count += 1;

            let combined_data = Bytes::from(self.base.buffer.concat());

            let data_with_params = self.inject_params(&combined_data);

            let packets = match self
                .encoder
                .payload(RTP_OUTBOUND_MTU, &Bytes::from(data_with_params))
            {
                Ok(payloads) => {
                    let length = payloads.len();
                    payloads
                        .into_iter()
                        .enumerate()
                        .map(|(i, payload)| {
                            let mut header = packet.clone().header;
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
        if self.mime_type.to_lowercase() == "video/h264" {
            trace!(
                "Setting H.264 params from SDP - SPS: {} bytes, PPS: {} bytes",
                sps.len(),
                pps.len()
            );
            self.sps = Some(sps);
            self.pps = Some(pps);
        }
    }
}
