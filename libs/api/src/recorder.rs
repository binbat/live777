use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Recording session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSession {
    /// Session UUID (optional for backward compatibility)
    pub id: Option<String>,
    /// Stream name
    pub stream: String,
    /// Recording start timestamp (microseconds since epoch)
    pub start_ts: i64,
    /// Recording end timestamp (microseconds since epoch, None if still recording)
    pub end_ts: Option<i64>,
    /// Duration in milliseconds (None if still recording)
    pub duration_ms: Option<i32>,
    /// Path to the MPD manifest file
    pub mpd_path: String,
    /// Recording status
    pub status: RecordingStatus,
}

/// Recording status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecordingStatus {
    /// Recording is currently active
    Active,
    /// Recording completed successfully
    Completed,
    /// Recording failed or was interrupted
    Failed,
}

impl std::fmt::Display for RecordingStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordingStatus::Active => write!(f, "Active"),
            RecordingStatus::Completed => write!(f, "Completed"),
            RecordingStatus::Failed => write!(f, "Failed"),
        }
    }
}

impl FromStr for RecordingStatus {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Active" => Ok(RecordingStatus::Active),
            "Completed" => Ok(RecordingStatus::Completed),
            "Failed" => Ok(RecordingStatus::Failed),
            _ => Err(()),
        }
    }
}

/// Request to pull recording sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRecordingsRequest {
    /// Stream name filter (None for all streams)
    pub stream: Option<String>,
    /// Only get sessions updated since this timestamp
    pub since_ts: Option<i64>,
    /// Maximum number of sessions to return
    pub limit: u32,
}

/// Response containing recording sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRecordingsResponse {
    /// Recording sessions
    pub sessions: Vec<RecordingSession>,
    /// Timestamp of the newest session (for next pull)
    pub last_ts: Option<i64>,
}

/// Response containing recording sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingSessionResponse {
    /// Recording sessions
    pub sessions: Vec<RecordingSession>,
    /// Total count
    pub total_count: u64,
}

/// List of available recorded streams
#[derive(Debug, Serialize, Deserialize)]
pub struct StreamsListResponse {
    pub streams: Vec<String>,
}

/// Request to pull segments from Live777 node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullSegmentsRequest {
    /// Stream name filter (None for all streams)
    pub stream: Option<String>,
    /// Only get segments created since this timestamp
    pub since_ts: Option<i64>,
    /// Maximum number of segments to return
    pub limit: u32,
}

/// Segment information from Live777 node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SegmentInfo {
    pub node_alias: String,
    pub stream: String,
    pub start_ts: i64,
    pub end_ts: i64,
    pub duration_ms: i32,
    pub path: String,
    pub is_keyframe: bool,
    pub created_at: i64, // timestamp when segment was created
}

/// Response containing segments from Live777 node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullSegmentsResponse {
    /// Segments
    pub segments: Vec<SegmentInfo>,
    /// Timestamp of the newest segment (for next pull)
    pub last_ts: Option<i64>,
}
