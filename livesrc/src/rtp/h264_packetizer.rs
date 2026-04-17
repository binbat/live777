// RTP H.264 Packetizer (RFC 6184)
// Converts H.264 NAL units to RTP packets

use anyhow::Result;

use super::annex_b_parser::NalUnit;

/// RTP Header structure
#[derive(Debug, Clone)]
pub struct RtpHeader {
    pub version: u8,        // Always 2
    pub padding: bool,
    pub extension: bool,
    pub marker: bool,       // Last packet of frame
    pub payload_type: u8,   // 96 for H.264
    pub sequence: u16,
    pub timestamp: u32,     // 90kHz clock
    pub ssrc: u32,          // Synchronization source
}

impl RtpHeader {
    pub fn to_bytes(&self) -> [u8; 12] {
        let mut bytes = [0u8; 12];
        
        // Byte 0: V(2), P(1), X(1), CC(4)
        bytes[0] = (self.version << 6) 
            | ((self.padding as u8) << 5) 
            | ((self.extension as u8) << 4);
        
        // Byte 1: M(1), PT(7)
        bytes[1] = ((self.marker as u8) << 7) | (self.payload_type & 0x7F);
        
        // Bytes 2-3: Sequence number
        bytes[2..4].copy_from_slice(&self.sequence.to_be_bytes());
        
        // Bytes 4-7: Timestamp
        bytes[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        
        // Bytes 8-11: SSRC
        bytes[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
        
        bytes
    }
}

/// RTP Packet
#[derive(Debug, Clone)]
pub struct RtpPacket {
    pub header: RtpHeader,
    pub payload: Vec<u8>,
}

impl RtpPacket {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(12 + self.payload.len());
        bytes.extend_from_slice(&self.header.to_bytes());
        bytes.extend_from_slice(&self.payload);
        bytes
    }
}

/// H.264 RTP Packetizer
pub struct H264Packetizer {
    mtu: usize,
    payload_type: u8,
    ssrc: u32,
    sequence: u16,
    timestamp: u32,
    clock_rate: u32,  // 90kHz for video
    cached_sps: Option<Vec<u8>>,
    cached_pps: Option<Vec<u8>>,
    sps_pps_timestamp: u32,
}

impl H264Packetizer {
    pub fn new(mtu: usize) -> Self {
        Self {
            mtu,
            payload_type: 96,  // Dynamic payload type for H.264
            ssrc: rand::random(),
            sequence: rand::random(),
            timestamp: rand::random(),
            clock_rate: 90000,  // 90kHz
            cached_sps: None,
            cached_pps: None,
            sps_pps_timestamp: 0,
        }
    }

    /// 封装单个 NAL unit 为 RTP packets
    pub fn packetize(&mut self, nal: &NalUnit) -> Result<Vec<RtpPacket>> {
        let mut packets = Vec::new();

        use crate::rtp::annex_b_parser::NalType;
        match nal.nal_type {
            NalType::Sps => {
                self.cached_sps = Some(nal.data.clone());
                self.sps_pps_timestamp = self.timestamp;
            }
            NalType::Pps => {
                self.cached_pps = Some(nal.data.clone());
                self.sps_pps_timestamp = self.timestamp;
            }
            NalType::Idr => {
                if self.sps_pps_timestamp != self.timestamp {
                    if let Some(sps) = &self.cached_sps {
                        let sps_nal = NalUnit { nal_type: NalType::Sps, data: sps.clone() };
                        packets.push(self.create_single_nal_packet(&sps_nal, false));
                    }
                    if let Some(pps) = &self.cached_pps {
                        let pps_nal = NalUnit { nal_type: NalType::Pps, data: pps.clone() };
                        packets.push(self.create_single_nal_packet(&pps_nal, false));
                    }
                    self.sps_pps_timestamp = self.timestamp;
                }
            }
            _ => {}
        }

        // RTP header 12 bytes, 需要为 payload 留空间
        let max_payload = self.mtu - 12;
        let is_vcl = matches!(nal.nal_type, NalType::Slice | NalType::Idr);

        if nal.data.len() <= max_payload {
            // Single NAL Unit Mode
            packets.push(self.create_single_nal_packet(nal, is_vcl));
        } else {
            // FU-A Fragmentation Mode
            packets.extend(self.create_fua_packets(nal, is_vcl));
        }

        Ok(packets)
    }

    /// 创建 Single NAL Unit 包
    fn create_single_nal_packet(&mut self, nal: &NalUnit, marker: bool) -> RtpPacket {
        let header = RtpHeader {
            version: 2,
            padding: false,
            extension: false,
            marker,
            payload_type: self.payload_type,
            sequence: self.sequence,
            timestamp: self.timestamp,
            ssrc: self.ssrc,
        };

        self.sequence = self.sequence.wrapping_add(1);

        RtpPacket {
            header,
            payload: nal.data.clone(),
        }
    }

    /// 创建 FU-A 分片包
    fn create_fua_packets(&mut self, nal: &NalUnit, is_vcl: bool) -> Vec<RtpPacket> {
        let nal_header = nal.data[0];
        let nal_type = nal_header & 0x1F;
        let nal_nri = nal_header & 0xE0;

        // FU indicator: F(1)|NRI(2)|Type(5) = NRI|28
        let fu_indicator = nal_nri | 28;  // 28 = FU-A

        // NAL payload (去掉 NAL header)
        let payload_data = &nal.data[1..];

        // 计算每个分片的大小（FU indicator + FU header + data）
        let max_fragment_size = self.mtu - 12 - 2;  // RTP header + FU headers

        let chunks: Vec<&[u8]> = payload_data
            .chunks(max_fragment_size)
            .collect();

        chunks
            .iter()
            .enumerate()
            .map(|(i, chunk)| {
                let start = i == 0;
                let end = i == chunks.len() - 1;

                // FU header: S(1)|E(1)|R(1)|Type(5)
                let fu_header = ((start as u8) << 7)
                    | ((end as u8) << 6)
                    | nal_type;

                // Payload: FU indicator + FU header + fragment
                let mut payload = Vec::with_capacity(2 + chunk.len());
                payload.push(fu_indicator);
                payload.push(fu_header);
                payload.extend_from_slice(chunk);

                let header = RtpHeader {
                    version: 2,
                    padding: false,
                    extension: false,
                    marker: end && is_vcl,  // Marker on last fragment if VCL
                    payload_type: self.payload_type,
                    sequence: self.sequence,
                    timestamp: self.timestamp,
                    ssrc: self.ssrc,
                };

                self.sequence = self.sequence.wrapping_add(1);

                RtpPacket { header, payload }
            })
            .collect()
    }

    /// 更新时间戳（根据帧率）
    /// 例如：30fps = 90000 / 30 = 3000 ticks per frame
    pub fn update_timestamp(&mut self, duration_ticks: u32) {
        self.timestamp = self.timestamp.wrapping_add(duration_ticks);
    }

    /// 根据帧率计算时间戳增量
    pub fn timestamp_increment(&self, fps: u32) -> u32 {
        self.clock_rate / fps
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rtp::annex_b_parser::{NalType, NalUnit};

    #[test]
    fn test_single_nal_packet() {
        let mut packetizer = H264Packetizer::new(1500);
        
        let nal = NalUnit {
            nal_type: NalType::Sps,
            data: vec![0x67, 0x42, 0x00, 0x1e, 0x95, 0xa0, 0x14],
        };

        let packets = packetizer.packetize(&nal).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].payload, nal.data);
        assert!(!packets[0].header.marker);
    }

    #[test]
    fn test_fua_fragmentation() {
        let mut packetizer = H264Packetizer::new(100);  // Small MTU
        
        // Large NAL unit
        let large_data = vec![0x65; 500];  // IDR slice header + data
        let nal = NalUnit {
            nal_type: NalType::Idr,
            data: large_data,
        };

        let packets = packetizer.packetize(&nal).unwrap();
        
        // Should be fragmented
        assert!(packets.len() > 1);
        
        // Check FU-A indicators
        for (i, packet) in packets.iter().enumerate() {
            assert_eq!(packet.payload[0] & 0x1F, 28);  // FU-A type
            
            if i == 0 {
                // First fragment: S bit set
                assert_eq!(packet.payload[1] & 0x80, 0x80);
            }
            if i == packets.len() - 1 {
                // Last fragment: E bit set, marker bit set
                assert_eq!(packet.payload[1] & 0x40, 0x40);
                assert!(packet.header.marker);
            }
        }
    }

    #[test]
    fn test_timestamp_increment() {
        let packetizer = H264Packetizer::new(1500);
        assert_eq!(packetizer.timestamp_increment(30), 3000);  // 30 fps
        assert_eq!(packetizer.timestamp_increment(25), 3600);  // 25 fps
    }
}
