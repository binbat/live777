// H.264 Annex B format parser
// Finds start codes and extracts NAL units

use anyhow::{Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt};

/// H.264 NAL unit type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NalType {
    Slice = 1,
    Dpa = 2,
    Dpb = 3,
    Dpc = 4,
    Idr = 5,
    Sei = 6,
    Sps = 7,
    Pps = 8,
    Aud = 9,
    EndSequence = 10,
    EndStream = 11,
    Filler = 12,
    Unknown,
}

impl From<u8> for NalType {
    fn from(val: u8) -> Self {
        match val & 0x1F {
            1 => NalType::Slice,
            2 => NalType::Dpa,
            3 => NalType::Dpb,
            4 => NalType::Dpc,
            5 => NalType::Idr,
            6 => NalType::Sei,
            7 => NalType::Sps,
            8 => NalType::Pps,
            9 => NalType::Aud,
            10 => NalType::EndSequence,
            11 => NalType::EndStream,
            12 => NalType::Filler,
            _ => NalType::Unknown,
        }
    }
}

/// Parsed NAL unit
#[derive(Debug, Clone)]
pub struct NalUnit {
    pub nal_type: NalType,
    pub data: Vec<u8>,  // 包含 NAL header
}

impl NalUnit {
    pub fn is_keyframe(&self) -> bool {
        matches!(self.nal_type, NalType::Idr | NalType::Sps | NalType::Pps)
    }
}

/// H.264 Annex B parser
pub struct AnnexBParser<R: AsyncRead + Unpin> {
    reader: R,
    buffer: Vec<u8>,
    position: usize,
}

impl<R: AsyncRead + Unpin> AnnexBParser<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buffer: Vec::with_capacity(1024 * 1024), // 1MB buffer
            position: 0,
        }
    }

    /// 读取下一个 NAL unit
    pub async fn read_next_nal(&mut self) -> Result<Option<NalUnit>> {
        loop {
            // 查找起始码
            let start_pos = match self.find_start_code().await? {
                Some(pos) => pos,
                None => return Ok(None),  // 真正到达 EOF
            };

            // 消费起始码之前的数据
            self.buffer.drain(..start_pos);

            // 跳过起始码本身 (3 或 4 字节)
            let start_code_len = if self.buffer.len() >= 4 && &self.buffer[0..4] == &[0, 0, 0, 1] {
                4
            } else if self.buffer.len() >= 3 && &self.buffer[0..3] == &[0, 0, 1] {
                3
            } else {
                bail!("Invalid start code at position 0");
            };

            self.buffer.drain(..start_code_len);

            // 查找下一个起始码（确认 NAL 结束位置）
            let end_pos = match self.find_start_code().await? {
                Some(pos) => pos,
                None => {
                    // 到流结尾了，提取剩下的所有数据作为一个 NAL
                    let remaining = self.buffer.len();
                    if remaining == 0 { return Ok(None); }
                    remaining
                }
            };

            if end_pos == 0 {
                // 连续的起始码（空 NAL），跳过当前并继续查找下一个
                continue;
            }

            // 提取 NAL unit 数据
            let nal_data: Vec<u8> = self.buffer.drain(..end_pos).collect();
            
            // 解析 NAL type (NAL header 的第一个字节)
            let nal_type = NalType::from(nal_data[0]);

            return Ok(Some(NalUnit {
                nal_type,
                data: nal_data,
            }));
        }
    }

    /// 查找起始码（0x00 00 00 01 或 0x00 00 01）
    async fn find_start_code(&mut self) -> Result<Option<usize>> {
        let mut empty_read_count = 0;
        const MAX_INITIAL_RETRIES: u32 = 50; // Max retries for initial empty reads
        
        loop {
            // 在已有 buffer 中查找
            if let Some(pos) = self.search_start_code() {
                return Ok(Some(pos));
            }

            // 读取更多数据
            let mut chunk = vec![0u8; 8192];
            let n = self.reader.read(&mut chunk).await?;
            
            if n == 0 {
                // Empty read - could be transient or true EOF
                if self.buffer.is_empty() {
                    // No data in buffer yet - might be startup delay
                    empty_read_count += 1;
                    if empty_read_count > MAX_INITIAL_RETRIES {
                       // Truly no data after many retries
                        return Ok(None);
                    }
                    // Wait and retry
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                } else {
                    // Have some buffer data, this might be end of current NAL
                    return Ok(None);
                }
            }
            
            // Got data - reset retry counter
            empty_read_count = 0;
            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }

    /// 在 buffer 中搜索起始码
    fn search_start_code(&self) -> Option<usize> {
        if self.buffer.len() < 3 {
            return None;
        }

        for i in self.position..self.buffer.len() - 2 {
            // 检查 0x00 00 01
            if self.buffer[i] == 0 
                && self.buffer[i + 1] == 0 
                && self.buffer[i + 2] == 1 {
                return Some(i);
            }

            // 检查 0x00 00 00 01
            if i < self.buffer.len() - 3
                && self.buffer[i] == 0 
                && self.buffer[i + 1] == 0 
                && self.buffer[i + 2] == 0 
                && self.buffer[i + 3] == 1 {
                return Some(i);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn test_parse_nal() {
        // 测试数据: 两个 NAL units
        let data = vec![
            0x00, 0x00, 0x00, 0x01,  // Start code
            0x67, 0x42, 0x00, 0x1e,  // SPS NAL
            0x00, 0x00, 0x00, 0x01,  // Start code
            0x68, 0xce, 0x38, 0x80,  // PPS NAL
        ];

        let cursor = std::io::Cursor::new(data);
        let mut parser = AnnexBParser::new(BufReader::new(cursor));

        // 第一个 NAL (SPS)
        let nal1 = parser.read_next_nal().await.unwrap().unwrap();
        assert_eq!(nal1.nal_type, NalType::Sps);
        assert_eq!(nal1.data, vec![0x67, 0x42, 0x00, 0x1e]);

        // 第二个 NAL (PPS)
        let nal2 = parser.read_next_nal().await.unwrap().unwrap();
        assert_eq!(nal2.nal_type, NalType::Pps);
        assert_eq!(nal2.data, vec![0x68, 0xce, 0x38, 0x80]);
    }
}
