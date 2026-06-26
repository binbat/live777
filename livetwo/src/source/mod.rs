#[cfg(feature = "rsmpeg")]
pub mod frame_gen;
#[cfg(feature = "rsmpeg")]
pub mod rsmpeg_gen;

#[cfg(feature = "rsmpeg")]
pub use frame_gen::{EncodedFrame, FrameGenerator, FrameGeneratorConfig, MediaFrame};
#[cfg(feature = "rsmpeg")]
pub use rsmpeg_gen::{AudioCodec, GeneratorConfig, VideoCodec, extract_h265_sprop, generate_sdp};
