//! Public types for the livehal native backend crate.
//!
//! `NativeSourceParams` is defined here and owned by livehal.
//! liveion converts `SourceSpec` / URL-based config into this type and passes it to
//! `NativePipeline::new()`.

/// Parameters for creating a native capture+encode pipeline.
#[derive(Debug, Clone)]
pub struct NativeSourceParams {
    pub capture_backend: String,
    pub capture_device: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub capture_pixel_format: u32,
    pub encoder_backend: String,
    pub codec: u32,
    pub bitrate: u32,
    pub profile: String,
    pub gop: u32,
    pub payload_type: u32,
    pub clock_rate: u32,
    pub capture_prefer_dmabuf: u8,
    pub encoder_prefer_dmabuf: u8,
    /// Codec name for SDP (e.g. "H264", "H265"). Used by liveion's
    /// `get_video_codec()` when constructing RTCRtpCodecParameters.
    pub codec_name: String,
    /// Default profile-level-id for SDP (e.g. "42001f").
    pub default_profile: String,

    // --- Numeric profile / level / tier (Rust resolves; C++ and SDP use directly) ---
    /// Numeric profile_idc — 66=Baseline, 77=Main, 100=High (H.264); 1=Main (H.265).
    pub profile_idc: u32,
    /// Numeric level_idc — 40=H.264 4.0, 120=H.265 4.0.
    pub level_idc: u32,
    /// Numeric tier_flag — 0=Main tier (always 0 for H.264; 0 for H.265 MVP).
    pub tier_flag: u32,
}

/// An encoded video packet received from the C++ pipeline.
///
/// Data is copied from the FFI callback immediately — the `data` field
/// owns its bytes and is safe to use across await points.
#[derive(Debug, Clone)]
pub struct EncodedPacket {
    /// VideoCodec enum value (100=H264, 101=H265, …).
    pub codec: u32,
    /// Owned copy of the encoded frame data.
    pub data: Vec<u8>,
    /// Presentation timestamp in microseconds.
    pub pts_us: u64,
    /// Decode timestamp in microseconds.
    pub dts_us: u64,
    /// EncodedFlags bitmask (bit 0 = keyframe, bit 1 = config/SPS-PPS).
    pub flags: u32,
}
