//! AV1 RTP repacketizer for `whipinto` sources.
//!
//! Incoming AV1 RTP packets from external tools (ffmpeg, gstreamer, etc.) may be
//! larger than the WebRTC 1200-byte MTU (e.g. ffmpeg defaults to 1472).  This
//! module depacketizes the AV1 RTP stream into OBU bitstream and repacketizes
//! it with a smaller MTU before it reaches the WebRTC subscriber path.

use anyhow::{Context, Result};
use rtc_rtp::codec::av1::Av1Payloader;
use rtc_rtp::packet::Packet;
use rtc_rtp::packetizer::Payloader;
use tracing::trace;

use super::av1_assembler::Av1Assembler;

/// Target RTP payload size for repacketized AV1 packets.
///
/// WebRTC typically requires the full UDP datagram to stay under ~1200 bytes
/// after SRTP/DTLS/ICE overhead.
const AV1_OUTGOING_MTU: usize = 1200;

#[derive(Debug)]
pub struct Av1Repacketizer {
    assembler: Av1Assembler,
    payloader: Av1Payloader,
}

impl Default for Av1Repacketizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Av1Repacketizer {
    pub fn new() -> Self {
        Self {
            assembler: Av1Assembler::new(),
            payloader: Av1Payloader::default(),
        }
    }

    /// Process one incoming AV1 RTP packet and return zero or more repacketized
    /// RTP packets whose payloads are at most [`AV1_OUTGOING_MTU`] bytes.
    ///
    /// Sequence numbers in returned packets are placeholders and are rewritten
    /// later by `VirtualPublishTrack::inject_rtp`.
    pub fn process(&mut self, packet: &Packet) -> Result<Vec<Packet>> {
        let Some(temporal_unit) = self.assembler.feed(packet)? else {
            return Ok(Vec::new());
        };

        // Repacketize the assembled temporal unit.
        let temporal_unit = temporal_unit.freeze();
        let payloads = self
            .payloader
            .payload(AV1_OUTGOING_MTU, &temporal_unit)
            .with_context(|| "AV1 repacketization failed")?;

        let total = payloads.len();
        let mut output = Vec::with_capacity(total);

        for (idx, payload) in payloads.into_iter().enumerate() {
            let mut header = packet.header.clone();
            header.marker = idx == total - 1;
            header.sequence_number = 0; // rewritten by VirtualPublishTrack
            header.padding = false;
            header.extension = false;
            header.extension_profile = 0;
            header.extensions.clear();
            header.extensions_padding = 0;

            output.push(Packet { header, payload });
        }

        trace!(
            "AV1 repacketized: seq={} timestamp={} {} packets, last_marker={}",
            packet.header.sequence_number,
            packet.header.timestamp,
            output.len(),
            output.last().map(|p| p.header.marker).unwrap_or(false)
        );

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::{Bytes, BytesMut};
    use rtc_rtp::header::Header;

    /// Build a minimal AV1 OBU: Frame OBU (type=6) without extension and without
    /// size field, followed by payload bytes.
    fn build_av1_obu(payload: &[u8]) -> Bytes {
        let mut obu = BytesMut::with_capacity(1 + payload.len());
        // OBU header: type=6 (Frame), has_size=0, extension=0 -> 0x30
        obu.extend_from_slice(&[0x30]);
        obu.extend_from_slice(payload);
        obu.freeze()
    }

    /// Build an AV1 RTP packet using the AV1 OBU aggregation format.
    /// Aggregation header: W=1, no Z, no Y, no N -> 0x10
    fn build_av1_rtp_packet(seq: u16, timestamp: u32, marker: bool, obu: &[u8]) -> Packet {
        let mut payload = BytesMut::with_capacity(1 + obu.len());
        payload.extend_from_slice(&[0x10]); // aggregation header
        payload.extend_from_slice(obu);

        Packet {
            header: Header {
                version: 2,
                marker,
                payload_type: 96,
                sequence_number: seq,
                timestamp,
                ssrc: 0x12345678,
                ..Default::default()
            },
            payload: payload.freeze(),
        }
    }

    #[test]
    fn repacketize_single_oversized_temporal_unit() {
        // Create a temporal unit larger than 1200 bytes.
        let obu = build_av1_obu(&[0xAB; 3000]);
        let packet = build_av1_rtp_packet(1, 1000, true, &obu);

        let mut repacketizer = Av1Repacketizer::new();
        let output = repacketizer
            .process(&packet)
            .expect("process should succeed");

        assert!(
            !output.is_empty(),
            "oversized AV1 temporal unit should be split"
        );

        // Every output payload must be <= 1200 bytes.
        for (i, pkt) in output.iter().enumerate() {
            assert!(
                pkt.payload.len() <= AV1_OUTGOING_MTU,
                "packet {} payload {} exceeds MTU",
                i,
                pkt.payload.len()
            );
        }

        // Only the last packet should have the marker bit set.
        assert!(output.last().unwrap().header.marker);
        for pkt in output.iter().take(output.len() - 1) {
            assert!(!pkt.header.marker);
        }

        // Timestamps and SSRC should be preserved.
        for pkt in &output {
            assert_eq!(pkt.header.timestamp, 1000);
            assert_eq!(pkt.header.ssrc, 0x12345678);
            assert_eq!(pkt.header.payload_type, 96);
        }
    }

    #[test]
    fn fragmented_temporal_unit_is_reassembled_before_repacketization() {
        // Split a large OBU across two RTP packets using the AV1 Z/Y fragmentation
        // flags with W=1 (single OBU element, no length field).
        let obu = build_av1_obu(&[0xCD; 3000]);
        let split_at = 1501; // includes the 1-byte OBU header
        let first_fragment = &obu[..split_at];
        let second_fragment = &obu[split_at..];

        // First packet: W=1, Y=1 (last OBU continues), marker=false.
        // Aggregation header: 0x50 (W=1, Y=1)
        let mut payload1 = BytesMut::with_capacity(1 + first_fragment.len());
        payload1.extend_from_slice(&[0x50]);
        payload1.extend_from_slice(first_fragment);
        let packet1 = Packet {
            header: Header {
                version: 2,
                marker: false,
                payload_type: 96,
                sequence_number: 10,
                timestamp: 2000,
                ssrc: 0x12345678,
                ..Default::default()
            },
            payload: payload1.freeze(),
        };

        // Second packet: Z=1 (continuation), Y=0, marker=true.
        // Aggregation header: 0x90 (Z=1, W=1)
        let mut payload2 = BytesMut::with_capacity(1 + second_fragment.len());
        payload2.extend_from_slice(&[0x90]);
        payload2.extend_from_slice(second_fragment);
        let packet2 = Packet {
            header: Header {
                version: 2,
                marker: true,
                payload_type: 96,
                sequence_number: 11,
                timestamp: 2000,
                ssrc: 0x12345678,
                ..Default::default()
            },
            payload: payload2.freeze(),
        };

        let mut repacketizer = Av1Repacketizer::new();
        let out1 = repacketizer
            .process(&packet1)
            .expect("process should succeed");
        assert!(
            out1.is_empty(),
            "incomplete temporal unit yields no packets"
        );

        let out2 = repacketizer
            .process(&packet2)
            .expect("process should succeed");
        assert!(!out2.is_empty(), "complete temporal unit should be split");

        for (i, pkt) in out2.iter().enumerate() {
            assert!(
                pkt.payload.len() <= AV1_OUTGOING_MTU,
                "packet {} payload {} exceeds MTU",
                i,
                pkt.payload.len()
            );
        }
        assert!(out2.last().unwrap().header.marker);
    }

    #[test]
    fn sequence_gap_resets_assembler() {
        let obu = build_av1_obu(&[0xEF; 100]);
        let packet1 = build_av1_rtp_packet(1, 1000, false, &obu);
        // Gap: seq 2 is missing. packet2 is a complete temporal unit on its own.
        let packet2 = build_av1_rtp_packet(3, 1000, true, &obu);

        let mut repacketizer = Av1Repacketizer::new();
        let out1 = repacketizer.process(&packet1).unwrap();
        assert!(out1.is_empty());

        // Gap triggers reset; packet2 is processed as a new, complete temporal unit.
        let out2 = repacketizer.process(&packet2).unwrap();
        assert!(
            !out2.is_empty(),
            "gap should reset state and allow the new complete unit to be emitted"
        );
        assert!(out2.last().unwrap().header.marker);
    }
}
