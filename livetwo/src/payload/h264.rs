use tracing::trace;

mod nal_type {
    pub const NAL_SLICE_IDR: u8 = 5;
    pub const NAL_SPS: u8 = 7;
    pub const NAL_PPS: u8 = 8;
}

pub struct H264Processor {
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl H264Processor {
    pub fn new() -> Self {
        Self {
            sps: None,
            pps: None,
        }
    }

    pub fn has_params(&self) -> bool {
        self.sps.is_some() && self.pps.is_some()
    }

    pub fn set_params(&mut self, sps: Vec<u8>, pps: Vec<u8>) {
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
                    let nal_type = data[nal_start] & 0x1F;
                    if nal_type == nal_type::NAL_SLICE_IDR {
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

        if self.sps.is_none() || self.pps.is_none() {
            trace!("No cached H.264 SPS/PPS");
            return data.to_vec();
        }

        let mut result = Vec::new();
        result.extend_from_slice(&[0, 0, 0, 1]);
        result.extend_from_slice(self.sps.as_ref().unwrap());
        result.extend_from_slice(&[0, 0, 0, 1]);
        result.extend_from_slice(self.pps.as_ref().unwrap());
        result.extend_from_slice(data);

        trace!("Injected H.264 SPS/PPS");
        result
    }

    fn has_params_in_data(&self, data: &[u8]) -> bool {
        let mut has_sps = false;
        let mut has_pps = false;

        let check_len = data.len().min(1024);

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
                    let nal_type = data[nal_start] & 0x1F;

                    match nal_type {
                        nal_type::NAL_SPS => has_sps = true,
                        nal_type::NAL_PPS => has_pps = true,
                        _ => {}
                    }

                    if has_sps && has_pps {
                        return true;
                    }
                }
            }
        }

        has_sps && has_pps
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

impl Default for H264Processor {
    fn default() -> Self {
        Self::new()
    }
}
