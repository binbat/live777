use bytes::Bytes;
use tracing::error;
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
}

impl RePayloadCodec {
    pub fn new(mime_type: String) -> RePayloadCodec {
        RePayloadCodec {
            base: RePayloadBase::new(),
            decoder: match mime_type.as_str() {
                MIME_TYPE_VP8 => Box::default() as Box<vp8::Vp8Packet>,
                MIME_TYPE_VP9 => Box::default() as Box<vp9::Vp9Packet>,
                MIME_TYPE_H264 => Box::default() as Box<h264::H264Packet>,
                MIME_TYPE_OPUS => Box::default() as Box<opus::OpusPacket>,
                _ => Box::default() as Box<vp8::Vp8Packet>,
            },
            encoder: match mime_type.as_str() {
                MIME_TYPE_VP8 => Box::default() as Box<vp8::Vp8Payloader>,
                MIME_TYPE_VP9 => Box::default() as Box<vp9::Vp9Payloader>,
                MIME_TYPE_H264 => Box::default() as Box<h264::H264Payloader>,
                MIME_TYPE_OPUS => Box::default() as Box<opus::OpusPayloader>,
                _ => Box::default() as Box<vp8::Vp8Payloader>,
            },
        }
    }
}

impl RePayload for RePayloadCodec {
    fn payload(&mut self, packet: Packet) -> Vec<Packet> {
        self.base.verify_sequence_number(&packet);

        match self.decoder.depacketize(&packet.payload) {
            Ok(data) => self.base.buffer.push(data),
            Err(e) => error!("{}", e),
        };

        if packet.header.marker {
            let packets = match self
                .encoder
                .payload(RTP_OUTBOUND_MTU, &Bytes::from(self.base.buffer.concat()))
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
                    error!("{}", e);
                    vec![]
                }
            };
            self.base.clear_buffer();
            packets
        } else {
            vec![]
        }
    }
}
