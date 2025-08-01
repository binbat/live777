use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SegmentMetadata {
    pub start_ts: i64,
    pub end_ts: i64,
    pub duration_ms: i32,
    pub path: String,
    pub is_keyframe: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SegmentReportRequest {
    pub node_alias: String,
    pub stream: String,
    pub segments: Vec<SegmentMetadata>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SegmentReportResponse {
    pub success: bool,
    pub message: String,
    pub processed_count: usize,
}

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

#[derive(Debug, Serialize, Deserialize)]
pub struct TimelineResponse {
    pub stream: String,
    pub segments: Vec<TimelineSegment>,
    pub total_count: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StreamsListResponse {
    pub streams: Vec<String>,
}