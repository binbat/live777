use serde::{Deserialize, Serialize};

/// Segment metadata for recording
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentMetadata {
    /// Segment start timestamp in microseconds
    pub start_ts: i64,
    /// Segment end timestamp in microseconds  
    pub end_ts: i64,
    /// Segment duration in milliseconds
    pub duration_ms: i32,
    /// Storage path relative to the stream root
    pub path: String,
    /// Whether the segment starts with a keyframe
    pub is_keyframe: bool,
}

/// Request to pull segments from Live777 nodes
#[derive(Debug, Serialize, Deserialize)]
pub struct PullSegmentsRequest {
    /// Optional stream filter
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    /// Pull segments after this timestamp (microseconds)
    /// If not provided, returns all available segments
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_ts: Option<i64>,
    /// Maximum number of segments to return (default: 100)
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    100
}

/// Response from Live777 nodes for segment pull requests
#[derive(Debug, Serialize, Deserialize)]
pub struct PullSegmentsResponse {
    /// Node alias that generated these segments
    pub node_alias: String,
    /// Stream name
    pub stream: String,
    /// List of segment metadata
    pub segments: Vec<SegmentMetadata>,
    /// Timestamp of the latest segment (for next pull)
    pub last_ts: Option<i64>,
    /// Total available segments count
    pub total_count: u32,
    /// Whether there are more segments available
    pub has_more: bool,
}

/// Timeline segment for playback API
#[derive(Debug, Serialize, Deserialize)]
pub struct TimelineSegment {
    pub id: String,
    pub start_ts: i64,
    pub end_ts: i64,
    pub duration_ms: i32,
    pub path: String,
    pub is_keyframe: bool,
    pub created_at: String,
}

/// Timeline response for playback API
#[derive(Debug, Serialize, Deserialize)]
pub struct TimelineResponse {
    pub stream: String,
    pub segments: Vec<TimelineSegment>,
    pub total_count: u64,
}

/// List of available recorded streams
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamsListResponse {
    pub streams: Vec<String>,
}
