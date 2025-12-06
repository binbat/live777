use tracing::trace;

mod nal_type {
    pub const H265_NAL_IDR_W_RADL: u8 = 19;
    pub const H265_NAL_IDR_N_LP: u8 = 20;
    pub const H265_NAL_VPS: u8 = 32;
    pub const H265_NAL_SPS: u8 = 33;
    pub const H265_NAL_PPS: u8 = 34;
}

pub struct H265Processor {
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl H265Processor {
    pub fn new() -> Self {
        Self {
            vps: None,
            sps: None,
            pps: None,
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

    pub fn is_idr_frame(&self, data: &[u8]) -> bool {
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

    pub fn inject_params(&self, data: &[u8]) -> Vec<u8> {
        if !self.is_idr_frame(data) {
            return data.to_vec();
        }

        if self.has_params_in_data(data) {
            trace!("Frame already has params, skipping injection");
            return data.to_vec();
        }

        if !self.has_params() {
            trace!("No cached H.265 VPS/SPS/PPS");
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

        trace!("Injected H.265 VPS/SPS/PPS");
        result
    }

    pub fn extract_params(&mut self, data: &[u8]) {
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

                let nal_type = (data[nal_start] >> 1) & 0x3F;

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
                    nal_type::H265_NAL_VPS => {
                        if self.vps.is_none() {
                            self.vps = Some(data[nal_start..nal_end].to_vec());
                            trace!("Extracted VPS: {} bytes", nal_end - nal_start);
                        }
                    }
                    nal_type::H265_NAL_SPS => {
                        if self.sps.is_none() {
                            self.sps = Some(data[nal_start..nal_end].to_vec());
                            trace!("Extracted SPS: {} bytes", nal_end - nal_start);
                        }
                    }
                    nal_type::H265_NAL_PPS => {
                        if self.pps.is_none() {
                            self.pps = Some(data[nal_start..nal_end].to_vec());
                            trace!("Extracted PPS: {} bytes", nal_end - nal_start);
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

    fn has_params_in_data(&self, data: &[u8]) -> bool {
        let mut has_vps = false;
        let mut has_sps = false;
        let mut has_pps = false;

        let check_len = data.len().min(2048);

        for i in 0..check_len.saturating_sub(4) {
            if data[i] == 0 && data[i + 1] == 0 {
                let nal_start = if data[i + 2] == 0 && data[i + 3] == 1 {
                    i + 4
                } else if data[i + 2] == 1 {
                    i + 3
                } else {
                    continue;
                };

                if nal_start < check_len {
                    let nal_type = (data[nal_start] >> 1) & 0x3F;
                    match nal_type {
                        nal_type::H265_NAL_VPS => has_vps = true,
                        nal_type::H265_NAL_SPS => has_sps = true,
                        nal_type::H265_NAL_PPS => has_pps = true,
                        _ => {}
                    }

                    if has_vps && has_sps && has_pps {
                        return true;
                    }
                }
            }
        }

        has_vps && has_sps && has_pps
    }
}

impl Default for H265Processor {
    fn default() -> Self {
        Self::new()
    }
}
