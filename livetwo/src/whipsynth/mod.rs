use std::time::Duration;

pub mod loadtest;
pub mod packetizer;
pub mod publisher;
pub mod source;

pub use loadtest::{LoadtestConfig, LoadtestStats, LoadtestStatsSnapshot, run_loadtest};
pub use packetizer::{Packetizer, PacketizerConfig};
pub use publisher::{Publisher, PublisherConfig};
pub use source::SourceHandle;

/// Runtime statistics for a WHIP publisher session.
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub nack_count: u64,
    pub pli_count: u64,
    pub connected_duration: Duration,
}
