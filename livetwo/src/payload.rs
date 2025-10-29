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

mod nal_type {
    pub const NAL_SLICE_IDR: u8 = 5;
    pub const NAL_SPS: u8 = 7;
    pub const NAL_PPS: u8 = 8;

    pub const H265_NAL_IDR_W_RADL: u8 = 19;
    pub const H265_NAL_IDR_N_LP: u8 = 20;
    pub const H265_NAL_VPS: u8 = 32;
    pub const H265_NAL_SPS: u8 = 33;
    pub const H265_NAL_PPS: u8 = 34;
}
pub trait RePayload {
    fn payload(&mut self, packet: Packet) -> Vec<Packet>;
    fn set_h264_params(&mut self, sps: Vec<u8>, pps: Vec<u8>);
    fn set_h265_params(&mut self, vps: Vec<u8>, sps: Vec<u8>, pps: Vec<u8>);
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

    fn set_h265_params(&mut self, _vps: Vec<u8>, _sps: Vec<u8>, _pps: Vec<u8>) {}
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
    vps: Option<Vec<u8>>,
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
        } else if is(MIME_TYPE_HEVC) {
            Box::default() as Box<h265::H265Packet>
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
        } else if is(MIME_TYPE_HEVC) {
            Box::default() as Box<h265::HevcPayloader>
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
            vps: None,
            frame_count: 0,
        }
    }

    fn is_idr_frame(&self, data: &[u8]) -> bool {
        if self.mime_type.eq_ignore_ascii_case(MIME_TYPE_H264) {
            return self.is_h264_idr_frame(data);
        } else if self.mime_type.eq_ignore_ascii_case(MIME_TYPE_HEVC) {
            return self.is_h265_idr_frame(data);
        }
        false
    }

    fn is_h264_idr_frame(&self, data: &[u8]) -> bool {
        if !self.mime_type.eq_ignore_ascii_case(MIME_TYPE_H264) {
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
                    if nal_type == nal_type::NAL_SLICE_IDR {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn is_h265_idr_frame(&self, data: &[u8]) -> bool {
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
                    let nal_type = (data[nal_start] >> 1) & 0x3F;
                    if nal_type == nal_type::H265_NAL_IDR_W_RADL
                        || nal_type == nal_type::H265_NAL_IDR_N_LP
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn inject_params(&mut self, data: &[u8]) -> Vec<u8> {
        if !self.is_idr_frame(data) {
            return data.to_vec();
        }

        if self.has_params(data) {
            trace!(
                "Frame {} already has params, skipping injection",
                self.frame_count
            );
            return data.to_vec();
        }

        if self.mime_type.eq_ignore_ascii_case(MIME_TYPE_H264) {
            return self.inject_h264_params(data);
        } else if self.mime_type.eq_ignore_ascii_case(MIME_TYPE_HEVC) {
            return self.inject_h265_params(data);
        }

        data.to_vec()
    }

    fn inject_h264_params(&self, data: &[u8]) -> Vec<u8> {
        if self.sps.is_none() || self.pps.is_none() {
            trace!("No cached H.264 SPS/PPS for frame {}", self.frame_count);
            return data.to_vec();
        }

        let mut result = Vec::new();
        result.extend_from_slice(&[0, 0, 0, 1]);
        result.extend_from_slice(self.sps.as_ref().unwrap());
        result.extend_from_slice(&[0, 0, 0, 1]);
        result.extend_from_slice(self.pps.as_ref().unwrap());
        result.extend_from_slice(data);

        trace!("Injected H.264 SPS/PPS at frame {}", self.frame_count);
        result
    }

    fn inject_h265_params(&self, data: &[u8]) -> Vec<u8> {
        if self.vps.is_none() || self.sps.is_none() || self.pps.is_none() {
            trace!("No cached H.265 VPS/SPS/PPS for frame {}", self.frame_count);
            return data.to_vec();
        }

        let mut result = Vec::new();
        result.extend_from_slice(&[0, 0, 0, 1]);
        result.extend_from_slice(self.vps.as_ref().unwrap());
        result.extend_from_slice(&[0, 0, 0, 1]);
        result.extend_from_slice(self.sps.as_ref().unwrap());
        result.extend_from_slice(&[0, 0, 0, 1]);
        result.extend_from_slice(self.pps.as_ref().unwrap());
        result.extend_from_slice(data);

        trace!("Injected H.265 VPS/SPS/PPS at frame {}", self.frame_count);
        result
    }

    fn has_params(&self, data: &[u8]) -> bool {
        if self.mime_type.eq_ignore_ascii_case(MIME_TYPE_H264) {
            return self.has_h264_params(data);
        } else if self.mime_type.eq_ignore_ascii_case(MIME_TYPE_HEVC) {
            return self.has_h265_params(data);
        }
        false
    }

    fn has_h264_params(&self, data: &[u8]) -> bool {
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
                    if nal_type == nal_type::NAL_SPS {
                        has_sps = true;
                    } else if nal_type == nal_type::NAL_PPS {
                        has_pps = true;
                    }
                }
            }
        }

        has_sps && has_pps
    }

    fn has_h265_params(&self, data: &[u8]) -> bool {
        let mut has_vps = false;
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
                    let nal_type = (data[nal_start] >> 1) & 0x3F;
                    if nal_type == nal_type::H265_NAL_VPS {
                        has_vps = true;
                    } else if nal_type == nal_type::H265_NAL_SPS {
                        has_sps = true;
                    } else if nal_type == nal_type::H265_NAL_PPS {
                        has_pps = true;
                    }
                }
            }
        }

        has_vps && has_sps && has_pps
    }

    fn extract_params(&mut self, data: &[u8]) {
        if !self.mime_type.eq_ignore_ascii_case(MIME_TYPE_H264) {
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
                    nal_type::NAL_SPS => {
                        if self.sps.is_none() {
                            self.sps = Some(data[nal_start..nal_end].to_vec());
                            trace!("Extracted SPS from stream: {} bytes", nal_end - nal_start);
                        }
                    }
                    nal_type::NAL_PPS => {
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
        if self.mime_type.eq_ignore_ascii_case(MIME_TYPE_H264) {
            trace!(
                "Setting H.264 params from SDP - SPS: {} bytes, PPS: {} bytes",
                sps.len(),
                pps.len()
            );
            self.sps = Some(sps);
            self.pps = Some(pps);
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
            self.vps = Some(vps);
            self.sps = Some(sps);
            self.pps = Some(pps);
        }
    }
}
