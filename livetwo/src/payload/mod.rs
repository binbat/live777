mod h264;
mod h265;
mod repayload;

pub use repayload::{Forward, RePayload, RePayloadCodec};

pub(crate) use h264::H264Processor;
pub(crate) use h265::H265Processor;

/// RTP outbound MTU
/// https://github.com/webrtc-rs/webrtc/blob/dcfefd7b48dc2bb9ecf50ea66c304f62719a6c4a/webrtc/src/track/mod.rs#L10C12-L10C49
/// https://github.com/binbat/live777/issues/1200
pub const RTP_OUTBOUND_MTU: usize = 1200;
