use std::collections::HashMap;
use base64::{engine::general_purpose, Engine as _};
use bytes::{BufMut, BytesMut};


#[derive(Default, Debug, Clone)]
pub struct SdpMediaInfo {
    pub media_type: String,
    port: usize,
    protocol: String,
    fmts: Vec<u8>,
    bandwidth: Option<Bandwidth>,
    pub rtpmap: RtpMap,
    pub fmtp: Option<Fmtp>,
    pub attributes: HashMap<String, String>,
}

#[derive(Default, Debug, Clone)]
pub struct Sdp {
    pub raw_string: String,
    version: u16,
    origin: String,
    session: String,
    connection: String,
    timing: String,
    pub medias: Vec<SdpMediaInfo>,
    attributes: HashMap<String, String>,
}

impl Unmarshal for SdpMediaInfo {
    fn unmarshal(raw_data: &str) -> Option<Self> {
        let mut sdp_media = SdpMediaInfo::default();
        let parameters: Vec<&str> = raw_data.split(' ').collect();

        if let Some(para_0) = parameters.first() {
            sdp_media.media_type = para_0.to_string();
        }

        if let Some(para_1) = parameters.get(1) {
            if let Ok(port) = para_1.parse::<usize>() {
                sdp_media.port = port;
            }
        }

        if let Some(para_2) = parameters.get(2) {
            sdp_media.protocol = para_2.to_string();
        }

        let mut cur_param_idx = 3;

        while let Some(fmt_str) = parameters.get(cur_param_idx) {
            if let Ok(fmt) = fmt_str.parse::<u8>() {
                sdp_media.fmts.push(fmt);
            }
            cur_param_idx += 1;
        }

        Some(sdp_media)
    }
}


impl Unmarshal for Sdp {
    fn unmarshal(raw_data: &str) -> Option<Self> {
        let mut sdp = Sdp {
            raw_string: raw_data.to_string(),
            ..Default::default()
        };

        let lines: Vec<&str> = raw_data.split(|c| c == '\r' || c == '\n').collect();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            let kv: Vec<&str> = line.trim().splitn(2, '=').collect();
            if kv.len() < 2 {
                println!("Sdp current line : {} parse error!", line);
                continue;
            }

            match kv[0] {
                "v" => {
                    if let Ok(version) = kv[1].parse::<u16>() {
                        sdp.version = version;
                    }
                }
                "o" => {
                    sdp.origin = kv[1].to_string();
                }
                "s" => {
                    sdp.session = kv[1].to_string();
                }
                "c" => {
                    sdp.connection = kv[1].to_string();
                }
                "t" => {
                    sdp.timing = kv[1].to_string();
                }
                "m" => {
                    if let Some(sdp_media) = SdpMediaInfo::unmarshal(kv[1]) {
                        sdp.medias.push(sdp_media);
                    }
                }
                "b" => {
                    if let Some(cur_media) = sdp.medias.last_mut() {
                        cur_media.bandwidth = Some(Bandwidth::unmarshal(kv[1]).unwrap());
                    } else {
                        continue;
                    }
                }
                "a" => {
                    let attribute: Vec<&str> = kv[1].splitn(2, ':').collect();

                    let attr_name = attribute[0];
                    let attr_value = if let Some(val) = attribute.get(1) {
                        val
                    } else {
                        ""
                    };

                    if let Some(cur_media) = sdp.medias.last_mut() {
                        if attribute.len() == 2 {
                            match attr_name {
                                "rtpmap" => {
                                    if let Some(rtpmap) = RtpMap::unmarshal(attr_value) {
                                        cur_media.rtpmap = rtpmap;
                                        continue;
                                    }
                                }
                                _ => {}
                            }
                        }
                        cur_media
                            .attributes
                            .insert(attr_name.to_string(), attr_value.to_string());
                    } else {
                        sdp.attributes
                            .insert(attr_name.to_string(), attr_value.to_string());
                    }
                }
                _ => {
                    println!("not parsed: {}", line);
                }
            }
        }

        Some(sdp)
    }
}


#[derive(Debug, Clone, Default)]
pub struct Bandwidth {
    b_type: String,
    bandwidth: u16,
}

impl Unmarshal for Bandwidth {
    fn unmarshal(raw_data: &str) -> Option<Self> {
        let mut sdp_bandwidth = Bandwidth::default();

        let parameters: Vec<&str> = raw_data.split(':').collect();
        if let Some(t) = parameters.first() {
            sdp_bandwidth.b_type = t.to_string();
        }

        if let Some(bandwidth) = parameters.get(1) {
            if let Ok(bandwidth) = bandwidth.parse::<u16>() {
                sdp_bandwidth.bandwidth = bandwidth;
            }
        }

        Some(sdp_bandwidth)
    }
}


pub trait Unmarshal {
    fn unmarshal(request_data: &str) -> Option<Self>
    where
        Self: Sized;
}

pub trait Marshal {
    fn marshal(&self) -> String;
}


#[derive(Debug, Clone, Default)]
pub struct RtpMap {
    pub payload_type: u16,
    pub encoding_name: String,
    pub clock_rate: u32,
    pub encoding_param: String,
}

impl Unmarshal for RtpMap {
    fn unmarshal(raw_data: &str) -> Option<Self> {
        let mut rtpmap = RtpMap::default();

        let parts: Vec<&str> = raw_data.split(' ').collect();

        if let Some(part_0) = parts.first() {
            if let Ok(payload_type) = part_0.parse::<u16>() {
                rtpmap.payload_type = payload_type;
            }
        }

        if let Some(part_1) = parts.get(1) {
            let parameters: Vec<&str> = part_1.split('/').collect();

            if let Some(para_0) = parameters.first() {
                rtpmap.encoding_name = para_0.to_string();
            }

            if let Some(para_1) = parameters.get(1) {
                if let Ok(clock_rate) = para_1.parse::<u32>() {
                    rtpmap.clock_rate = clock_rate;
                }
            }
            if let Some(para_2) = parameters.get(2) {
                rtpmap.encoding_param = para_2.to_string();
            }
        }

        Some(rtpmap)
    }
}

impl Marshal for RtpMap {
    fn marshal(&self) -> String {
        let mut rtpmap = format!(
            "{} {}/{}",
            self.payload_type, self.encoding_name, self.clock_rate
        );
        if self.encoding_param != *"" {
            rtpmap = format!("{}/{}", rtpmap, self.encoding_param);
        }

        format!("{rtpmap}\r\n")
    }
}


#[derive(Debug, Clone, Default)]
pub struct H264Fmtp {
    pub payload_type: u16,
    packetization_mode: u8,
    profile_level_id: BytesMut,
    pub sps: BytesMut,
    pub pps: BytesMut,
}
#[derive(Debug, Clone, Default)]
pub struct H265Fmtp {
    pub payload_type: u16,
    pub vps: BytesMut,
    pub sps: BytesMut,
    pub pps: BytesMut,
}

#[derive(Debug, Clone)]
pub enum Fmtp {
    H264(H264Fmtp),
    H265(H265Fmtp),
}

impl Fmtp {
    pub fn new(codec: &str, raw_data: &str) -> Option<Fmtp> {
        match codec.to_lowercase().as_str() {
            "h264" => {
                if let Some(h264_fmtp) = H264Fmtp::unmarshal(raw_data) {
                    return Some(Fmtp::H264(h264_fmtp));
                }
            }
            "h265" => {
                if let Some(h265_fmtp) = H265Fmtp::unmarshal(raw_data) {
                    return Some(Fmtp::H265(h265_fmtp));
                }
            }
            _ => {}
        }
        None
    }

}

impl Unmarshal for H264Fmtp {
    fn unmarshal(raw_data: &str) -> Option<Self> {
        let mut h264_fmtp = H264Fmtp::default();
        let eles: Vec<&str> = raw_data.splitn(2, ' ').collect();
        if eles.len() < 2 {
            println!("H264FmtpSdp parse err: {}", raw_data);
            return None;
        }

        if let Ok(payload_type) = eles[0].parse::<u16>() {
            h264_fmtp.payload_type = payload_type;
        }

        let parameters: Vec<&str> = eles[1].split(';').collect();
        for parameter in parameters {
            let kv: Vec<&str> = parameter.trim().splitn(2, '=').collect();
            if kv.len() < 2 {
                println!("H264FmtpSdp parse key=value err: {}", parameter);
                continue;
            }
            match kv[0] {
                "packetization-mode" => {
                    if let Ok(packetization_mode) = kv[1].parse::<u8>() {
                        h264_fmtp.packetization_mode = packetization_mode;
                    }
                }
                "sprop-parameter-sets" => {
                    let spspps: Vec<&str> = kv[1].split(',').collect();
                    let sps = general_purpose::STANDARD.decode(spspps[0]).unwrap();
                    h264_fmtp.sps.put(&sps[..]);
                    let pps = general_purpose::STANDARD.decode(spspps[1]).unwrap();
                    h264_fmtp.pps.put(&pps[..]);
                }
                "profile-level-id" => {
                    h264_fmtp.profile_level_id = kv[1].into();
                }
                _ => {
                    println!("not parsed: {}", kv[0])
                }
            }
        }

        Some(h264_fmtp)
    }
}

impl Marshal for H264Fmtp {
    fn marshal(&self) -> String {
        let sps_str = general_purpose::STANDARD.encode(&self.sps);
        let pps_str = general_purpose::STANDARD.encode(&self.pps);
        let profile_level_id_str = String::from_utf8(self.profile_level_id.to_vec()).unwrap();

        let h264_fmtp = format!(
            "{} packetization-mode={}; sprop-parameter-sets={},{}; profile-level-id={}",
            self.payload_type, self.packetization_mode, sps_str, pps_str, profile_level_id_str
        );

        format!("{h264_fmtp}\r\n")
    }
}

impl Unmarshal for H265Fmtp {
    fn unmarshal(raw_data: &str) -> Option<Self> {
        let mut h265_fmtp = H265Fmtp::default();
        let eles: Vec<&str> = raw_data.splitn(2, ' ').collect();
        if eles.len() < 2 {
            println!("H265FmtpSdp parse err: {}", raw_data);
            return None;
        }

        if let Ok(payload_type) = eles[0].parse::<u16>() {
            h265_fmtp.payload_type = payload_type;
        }

        let parameters: Vec<&str> = eles[1].split(';').collect();
        for parameter in parameters {
            let kv: Vec<&str> = parameter.trim().splitn(2, '=').collect();
            if kv.len() < 2 {
                println!("H265FmtpSdp parse key=value err: {}", parameter);
                continue;
            }

            match kv[0] {
                "sprop-vps" => {
                    h265_fmtp.vps = kv[1].into();
                }
                "sprop-sps" => {
                    h265_fmtp.sps = kv[1].into();
                }
                "sprop-pps" => {
                    h265_fmtp.pps = kv[1].into();
                }
                _ => {
                    println!("not parsed: {}", kv[0])
                }
            }
        }

        Some(h265_fmtp)
    }
}

