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

pub(crate) trait RePayload {
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

pub(crate) struct RePayloadVpx {
    buffer: Vec<Bytes>,
    encoder: Box<dyn Payloader + Send>,
    decoder: Box<dyn Depacketizer + Send>,
    sequence_number: u16,
    /// In order to verify that the sequence number is correct
    /// If network have some error loss some packet, We need detect it
    src_sequence_number: u16,
}

impl RePayloadVpx {
    pub fn new(mime_type: String) -> RePayloadVpx {
        RePayloadVpx {
            buffer: Vec::new(),
            decoder: match mime_type.as_str() {
                MIME_TYPE_VP8 => Box::default() as Box<vp8::Vp8Packet>,
                MIME_TYPE_VP9 => Box::default() as Box<vp9::Vp9Packet>,
                _ => Box::default() as Box<vp8::Vp8Packet>,
            },
            encoder: match mime_type.as_str() {
                MIME_TYPE_VP8 => Box::default() as Box<vp8::Vp8Payloader>,
                MIME_TYPE_VP9 => Box::default() as Box<vp9::Vp9Payloader>,
                _ => Box::default() as Box<vp8::Vp8Payloader>,
            },
            sequence_number: 0,
            src_sequence_number: 0,
        }
    }
}

impl RePayload for RePayloadVpx {
    fn payload(&mut self, packet: Packet) -> Vec<Packet> {
        // verify the sequence number is linear
        if self.src_sequence_number + 1 != packet.header.sequence_number
            && self.src_sequence_number != 0
        {
            error!(
                "Should received sequence: {}. But received sequence: {}",
                self.src_sequence_number + 1,
                packet.header.sequence_number
            );
        }
        self.src_sequence_number = packet.header.sequence_number;

        match self.decoder.depacketize(&packet.payload) {
            Ok(data) => self.buffer.push(data),
            Err(e) => error!("{}", e),
        };

        if packet.header.marker {
            let packets = match self
                .encoder
                .payload(RTP_OUTBOUND_MTU, &Bytes::from(self.buffer.concat()))
            {
                Ok(payloads) => {
                    let length = payloads.len();
                    payloads
                        .into_iter()
                        .enumerate()
                        .map(|(i, payload)| -> Packet {
                            let mut header = packet.clone().header;
                            header.sequence_number = self.sequence_number;
                            header.marker = matches!(i, x if x == length - 1);

                            self.sequence_number = self.sequence_number.wrapping_add(1);
                            Packet { header, payload }
                        })
                        .collect::<Vec<Packet>>()
                }
                Err(e) => {
                    error!("{}", e);
                    vec![]
                }
            };

            self.buffer.clear();
            return packets;
        }
        vec![]
    }
}
