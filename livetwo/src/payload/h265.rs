use tracing::{debug, trace, warn};

const NAL_UNIT_TYPE_MASK: u8 = 0x3F;
const START_CODE_3: [u8; 3] = [0, 0, 1];
const START_CODE_4: [u8; 4] = [0, 0, 0, 1];

mod nal_type {
    pub const H265_NAL_IDR_W_RADL: u8 = 19;
    pub const H265_NAL_IDR_N_LP: u8 = 20;
    pub const H265_NAL_CRA_NUT: u8 = 21;
    pub const H265_NAL_VPS: u8 = 32;
    pub const H265_NAL_SPS: u8 = 33;
    pub const H265_NAL_PPS: u8 = 34;
    pub const H265_NAL_AP: u8 = 48;
    pub const H265_NAL_FU: u8 = 49;
}

pub struct H265Processor {
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    params_injected: bool,
    last_frame_hash: u64,
    last_is_idr: bool,
}

impl H265Processor {
    pub fn new() -> Self {
        Self {
            vps: None,
            sps: None,
            pps: None,
            params_injected: false,
            last_frame_hash: 0,
            last_is_idr: false,
        }
    }

    pub fn has_params(&self) -> bool {
        self.vps.is_some() && self.sps.is_some() && self.pps.is_some()
    }

    pub fn set_params(&mut self, vps: Vec<u8>, sps: Vec<u8>, pps: Vec<u8>) {
        self.vps = Some(vps);
        self.sps = Some(sps);
        self.pps = Some(pps);
    }

    pub fn is_idr_frame(&mut self, data: &[u8]) -> bool {
        if data.is_empty() {
            return false;
        }

        let hash = self.simple_hash(data);
        if hash == self.last_frame_hash {
            return self.last_is_idr;
        }

        trace!("Checking frame for IDR, size: {} bytes", data.len());

        let is_idr = if Self::has_annex_b_start_code(data) {
            trace!("Annex B format detected");
            self.is_idr_frame_annex_b(data)
        } else {
            trace!("RTP payload format detected");
            self.is_idr_frame_rtp(data)
        };

        self.last_frame_hash = hash;
        self.last_is_idr = is_idr;

        if is_idr {
            debug!("IDR/keyframe detected");
        }

        is_idr
    }

    fn simple_hash(&self, data: &[u8]) -> u64 {
        let len = data.len().min(32);
        let mut hash: u64 = 0;
        for &byte in data[..len].iter() {
            hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
        }
        hash.wrapping_add(data.len() as u64)
    }

    fn has_annex_b_start_code(data: &[u8]) -> bool {
        data.starts_with(&START_CODE_4) || data.starts_with(&START_CODE_3)
    }

    fn is_idr_frame_annex_b(&self, data: &[u8]) -> bool {
        for nal in NalIterator::new(data) {
            let nal_type = (nal.data[0] >> 1) & NAL_UNIT_TYPE_MASK;

            trace!(
                "Annex B NAL type={} ({})",
                nal_type,
                Self::nal_type_name(nal_type)
            );

            if Self::is_idr_nal_type(nal_type) {
                return true;
            }
        }
        false
    }

    fn is_idr_frame_rtp(&self, data: &[u8]) -> bool {
        if data.is_empty() {
            return false;
        }

        let nal_type = (data[0] >> 1) & NAL_UNIT_TYPE_MASK;

        trace!(
            "RTP NAL type={} ({})",
            nal_type,
            Self::nal_type_name(nal_type)
        );

        match nal_type {
            0..=47 => Self::is_idr_nal_type(nal_type),
            nal_type::H265_NAL_AP => self.check_ap_for_idr(&data[2..]),
            nal_type::H265_NAL_FU => {
                if data.len() < 3 {
                    return false;
                }
                let fu_header = data[2];
                let start_bit = (fu_header >> 7) & 1;

                if start_bit == 1 {
                    let fu_nal_type = fu_header & NAL_UNIT_TYPE_MASK;
                    trace!("FU start, NAL type={}", fu_nal_type);
                    return Self::is_idr_nal_type(fu_nal_type);
                }
                false
            }
            _ => {
                warn!("Unknown NAL type {}", nal_type);
                false
            }
        }
    }

    fn is_idr_nal_type(nal_type: u8) -> bool {
        matches!(
            nal_type,
            nal_type::H265_NAL_IDR_W_RADL
                | nal_type::H265_NAL_IDR_N_LP
                | nal_type::H265_NAL_CRA_NUT
        )
    }

    fn check_ap_for_idr(&self, data: &[u8]) -> bool {
        let mut offset = 0;

        while offset + 2 < data.len() {
            let nal_size = ((data[offset] as usize) << 8) | (data[offset + 1] as usize);
            offset += 2;

            if offset + nal_size > data.len() {
                warn!("Invalid AP NAL size: {}", nal_size);
                break;
            }

            if nal_size > 0 {
                let nal_type = (data[offset] >> 1) & NAL_UNIT_TYPE_MASK;

                trace!("AP NAL type={}, size={}", nal_type, nal_size);

                if Self::is_idr_nal_type(nal_type) {
                    return true;
                }
            }

            offset += nal_size;
        }

        false
    }

    pub fn inject_params(&mut self, data: &[u8]) -> Vec<u8> {
        let is_idr = self.is_idr_frame(data);

        if !is_idr && self.params_injected {
            trace!("Not IDR and params already injected, skipping");
            return data.to_vec();
        }

        if !self.has_params() {
            warn!("No cached params available for injection");
            return data.to_vec();
        }

        debug!(
            "Injecting VPS/SPS/PPS (is_idr={}, first_injection={})",
            is_idr, !self.params_injected
        );

        let mut result = Vec::with_capacity(
            4 + self.vps.as_ref().map_or(0, |v| v.len())
                + 4
                + self.sps.as_ref().map_or(0, |v| v.len())
                + 4
                + self.pps.as_ref().map_or(0, |v| v.len())
                + data.len(),
        );

        if let Some(ref vps) = self.vps {
            result.extend_from_slice(&START_CODE_4);
            result.extend_from_slice(vps);
            trace!("Injected VPS ({} bytes)", vps.len());
        }

        if let Some(ref sps) = self.sps {
            result.extend_from_slice(&START_CODE_4);
            result.extend_from_slice(sps);
            trace!("Injected SPS ({} bytes)", sps.len());
        }

        if let Some(ref pps) = self.pps {
            result.extend_from_slice(&START_CODE_4);
            result.extend_from_slice(pps);
            trace!("Injected PPS ({} bytes)", pps.len());
        }

        if !Self::has_annex_b_start_code(data) && !data.is_empty() {
            result.extend_from_slice(&START_CODE_4);
        }

        result.extend_from_slice(data);

        self.params_injected = true;

        result
    }

    pub fn extract_params(&mut self, data: &[u8]) {
        let mut extracted_count = 0;

        for nal in NalIterator::new(data) {
            let nal_type = (nal.data[0] >> 1) & NAL_UNIT_TYPE_MASK;

            match nal_type {
                nal_type::H265_NAL_VPS if self.vps.is_none() => {
                    self.vps = Some(nal.data.to_vec());
                    trace!("Extracted VPS: {} bytes", nal.data.len());
                    extracted_count += 1;
                }
                nal_type::H265_NAL_SPS if self.sps.is_none() => {
                    self.sps = Some(nal.data.to_vec());
                    trace!("Extracted SPS: {} bytes", nal.data.len());
                    extracted_count += 1;
                }
                nal_type::H265_NAL_PPS if self.pps.is_none() => {
                    self.pps = Some(nal.data.to_vec());
                    trace!("Extracted PPS: {} bytes", nal.data.len());
                    extracted_count += 1;
                }
                _ => {}
            }

            if self.has_params() {
                break;
            }
        }

        if extracted_count > 0 {
            debug!("Extracted {} parameter sets", extracted_count);
        }
    }

    fn nal_type_name(nal_type: u8) -> &'static str {
        match nal_type {
            0 => "TRAIL_N",
            1 => "TRAIL_R",
            2 => "TSA_N",
            3 => "TSA_R",
            4 => "STSA_N",
            5 => "STSA_R",
            6 => "RADL_N",
            7 => "RADL_R",
            8 => "RASL_N",
            9 => "RASL_R",
            16 => "BLA_W_LP",
            17 => "BLA_W_RADL",
            18 => "BLA_N_LP",
            19 => "IDR_W_RADL",
            20 => "IDR_N_LP",
            21 => "CRA_NUT",
            32 => "VPS",
            33 => "SPS",
            34 => "PPS",
            35 => "AUD",
            39 => "PREFIX_SEI",
            40 => "SUFFIX_SEI",
            48 => "AP",
            49 => "FU",
            _ => "OTHER",
        }
    }
}

impl Default for H265Processor {
    fn default() -> Self {
        Self::new()
    }
}

struct NalIterator<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> NalIterator<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn extract_nal_unit(&mut self, nal_start: usize) -> Option<NalUnit<'a>> {
        if nal_start >= self.data.len() {
            return None;
        }

        let mut nal_end = nal_start + 1;
        while nal_end + 3 < self.data.len() {
            if self.data[nal_end..].starts_with(&START_CODE_4)
                || self.data[nal_end..].starts_with(&START_CODE_3)
            {
                break;
            }
            nal_end += 1;
        }

        if nal_end >= self.data.len() - 3 {
            nal_end = self.data.len();
        }

        self.offset = nal_end;

        Some(NalUnit {
            data: &self.data[nal_start..nal_end],
        })
    }
}

impl<'a> Iterator for NalIterator<'a> {
    type Item = NalUnit<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.offset + 4 <= self.data.len() {
            let remaining = &self.data[self.offset..];

            // Check for 4-byte start code (0x00 0x00 0x00 0x01)
            if remaining.starts_with(&START_CODE_4) {
                let nal_start = self.offset + 4;
                self.offset = nal_start;
                return self.extract_nal_unit(nal_start);
            }

            // Check for 3-byte start code (0x00 0x00 0x01)
            if remaining.starts_with(&START_CODE_3) {
                let nal_start = self.offset + 3;
                self.offset = nal_start;
                return self.extract_nal_unit(nal_start);
            }

            self.offset += 1;
        }

        None
    }
}

struct NalUnit<'a> {
    data: &'a [u8],
}
