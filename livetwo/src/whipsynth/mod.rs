use std::time::Duration;

pub mod loadtest;
pub mod packetizer;
pub mod publisher;
pub mod source;

pub use loadtest::{LoadtestConfig, LoadtestStats, run_loadtest};
pub use packetizer::{Packetizer, PacketizerConfig};
pub use publisher::{Publisher, PublisherConfig};
pub use source::SourceHandle;

use crate::source::{AudioCodec, VideoCodec};

/// Runtime statistics for a WHIP publisher session.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub nack_count: u64,
    pub pli_count: u64,
    pub connected_duration: Duration,
}

/// Helper to build a `PublisherConfig` from CLI-like arguments.
#[allow(clippy::too_many_arguments)]
pub fn publisher_config(
    whip_url: String,
    token: Option<String>,
    video_codec: VideoCodec,
    audio_codec: Option<AudioCodec>,
    width: u32,
    height: u32,
    fps: u32,
    duration: Option<Duration>,
) -> PublisherConfig {
    PublisherConfig {
        whip_url,
        token,
        video_codec,
        audio_codec,
        width,
        height,
        fps,
        duration,
    }
}
