//! Stream lifecycle state machine
//!
//! Tracks the full lifecycle of a stream from creation to active to failed.
//! This is complementary to the existing `StreamSourceState` which tracks
//! the connection-level state of a source.
//!
//! State transitions:
//! ```text
//! Created ──(source connected)──→ Connected ──(first data)──→ Active
//!    ↑                                │                          │
//!    │                                └──(error)──→ Failed ──────┘
//!    └────────────────────────────────────────────── (reset) ─────┘
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The high-level lifecycle state of a stream.
///
/// This is distinct from `StreamSourceState` which tracks the low-level
/// connection status. `StreamLifecycleState` represents the logical
/// readiness of the stream for consumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamLifecycleState {
    /// Empty stream (placeholder). No codec parameters known.
    /// Can be used for pre-authorization or reservation.
    Created,

    /// Source connected (WebRTC/RTP established) but no media data received yet.
    Connected,

    /// Actively receiving and forwarding media data.
    Active,

    /// Error state. Retains codec parameter information from previous active state.
    Failed,
}

impl Default for StreamLifecycleState {
    fn default() -> Self {
        Self::Created
    }
}

impl fmt::Display for StreamLifecycleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Connected => write!(f, "connected"),
            Self::Active => write!(f, "active"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

/// Policy controlling when a source daemon should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DaemonPolicy {
    /// Start when there are subscribers, stop when there are none.
    Auto,

    /// Always keep the source running regardless of subscriber count.
    Always,
}

impl Default for DaemonPolicy {
    fn default() -> Self {
        Self::Always
    }
}

impl fmt::Display for DaemonPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Always => write!(f, "always"),
        }
    }
}

/// Retained codec information from a previously active stream.
/// Used when transitioning to `Failed` state so that the stream
/// can be recreated with the same parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodecInfo {
    pub video_mime_type: Option<String>,
    pub video_clock_rate: Option<u32>,
    pub video_sdp_fmtp: Option<String>,
    pub audio_mime_type: Option<String>,
    pub audio_clock_rate: Option<u32>,
}

/// A lifecycle event emitted when the stream state changes.
#[derive(Debug, Clone, Serialize)]
pub struct LifecycleEvent {
    pub stream_id: String,
    pub old_state: StreamLifecycleState,
    pub new_state: StreamLifecycleState,
    pub timestamp: DateTime<Utc>,
    pub error: Option<String>,
}

/// Manages the lifecycle state of a single stream.
pub struct StreamLifecycle {
    stream_id: String,
    state: StreamLifecycleState,
    daemon_policy: DaemonPolicy,
    subscriber_count: usize,
    last_error: Option<String>,
    codec_info: Option<CodecInfo>,
    created_at: DateTime<Utc>,
    last_state_change: DateTime<Utc>,
}

impl StreamLifecycle {
    /// Create a new lifecycle tracker for a stream.
    pub fn new(stream_id: String, daemon_policy: DaemonPolicy) -> Self {
        let now = Utc::now();
        Self {
            stream_id,
            state: StreamLifecycleState::Created,
            daemon_policy,
            subscriber_count: 0,
            last_error: None,
            codec_info: None,
            created_at: now,
            last_state_change: now,
        }
    }

    /// Get the current lifecycle state.
    pub fn state(&self) -> StreamLifecycleState {
        self.state
    }

    /// Get the stream ID.
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// Get the daemon policy.
    pub fn daemon_policy(&self) -> DaemonPolicy {
        self.daemon_policy
    }

    /// Get the retained codec info (available in Failed state).
    pub fn codec_info(&self) -> Option<&CodecInfo> {
        self.codec_info.as_ref()
    }

    /// Get the last error message.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Get the creation timestamp.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Get the timestamp of the last state change.
    pub fn last_state_change(&self) -> DateTime<Utc> {
        self.last_state_change
    }

    /// Get the current subscriber count.
    pub fn subscriber_count(&self) -> usize {
        self.subscriber_count
    }

    /// Transition to Connected state.
    /// Valid from: Created, Failed
    pub fn on_connected(&mut self) -> Option<LifecycleEvent> {
        match self.state {
            StreamLifecycleState::Created | StreamLifecycleState::Failed => {
                let old = self.state;
                self.state = StreamLifecycleState::Connected;
                self.last_error = None;
                self.last_state_change = Utc::now();
                Some(LifecycleEvent {
                    stream_id: self.stream_id.clone(),
                    old_state: old,
                    new_state: self.state,
                    timestamp: self.last_state_change,
                    error: None,
                })
            }
            _ => None,
        }
    }

    /// Transition to Active state.
    /// Valid from: Connected
    pub fn on_active(&mut self) -> Option<LifecycleEvent> {
        if self.state == StreamLifecycleState::Connected {
            let old = self.state;
            self.state = StreamLifecycleState::Active;
            self.last_state_change = Utc::now();
            Some(LifecycleEvent {
                stream_id: self.stream_id.clone(),
                old_state: old,
                new_state: self.state,
                timestamp: self.last_state_change,
                error: None,
            })
        } else {
            None
        }
    }

    /// Transition to Failed state, retaining codec info.
    /// Valid from: Connected, Active
    pub fn on_failed(&mut self, error: String, codec_info: Option<CodecInfo>) -> Option<LifecycleEvent> {
        match self.state {
            StreamLifecycleState::Connected | StreamLifecycleState::Active => {
                let old = self.state;
                self.state = StreamLifecycleState::Failed;
                self.last_error = Some(error.clone());
                if codec_info.is_some() {
                    self.codec_info = codec_info;
                }
                self.last_state_change = Utc::now();
                Some(LifecycleEvent {
                    stream_id: self.stream_id.clone(),
                    old_state: old,
                    new_state: self.state,
                    timestamp: self.last_state_change,
                    error: Some(error),
                })
            }
            _ => None,
        }
    }

    /// Reset to Created state.
    pub fn reset(&mut self) -> Option<LifecycleEvent> {
        let old = self.state;
        self.state = StreamLifecycleState::Created;
        self.last_error = None;
        self.codec_info = None;
        self.last_state_change = Utc::now();
        Some(LifecycleEvent {
            stream_id: self.stream_id.clone(),
            old_state: old,
            new_state: self.state,
            timestamp: self.last_state_change,
            error: None,
        })
    }

    /// Update the subscriber count. Returns true if the source should be
    /// started/stopped based on daemon policy.
    pub fn update_subscribers(&mut self, count: usize) -> bool {
        let old_count = self.subscriber_count;
        self.subscriber_count = count;

        match self.daemon_policy {
            DaemonPolicy::Always => false, // Never stop
            DaemonPolicy::Auto => {
                // Signal change when crossing the zero boundary
                (old_count == 0 && count > 0) || (old_count > 0 && count == 0)
            }
        }
    }

    /// Whether the source should be running based on daemon policy and subscriber count.
    pub fn should_source_run(&self) -> bool {
        match self.daemon_policy {
            DaemonPolicy::Always => true,
            DaemonPolicy::Auto => self.subscriber_count > 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lifecycle_transitions() {
        let mut lc = StreamLifecycle::new("test-stream".to_string(), DaemonPolicy::Always);
        assert_eq!(lc.state(), StreamLifecycleState::Created);

        // Created -> Connected
        let event = lc.on_connected();
        assert!(event.is_some());
        assert_eq!(lc.state(), StreamLifecycleState::Connected);

        // Connected -> Active
        let event = lc.on_active();
        assert!(event.is_some());
        assert_eq!(lc.state(), StreamLifecycleState::Active);

        // Active -> Failed
        let event = lc.on_failed("test error".to_string(), Some(CodecInfo {
            video_mime_type: Some("video/H264".to_string()),
            video_clock_rate: Some(90000),
            video_sdp_fmtp: None,
            audio_mime_type: None,
            audio_clock_rate: None,
        }));
        assert!(event.is_some());
        assert_eq!(lc.state(), StreamLifecycleState::Failed);
        assert!(lc.codec_info().is_some());
        assert_eq!(lc.last_error(), Some("test error"));

        // Failed -> Connected (retry)
        let event = lc.on_connected();
        assert!(event.is_some());
        assert_eq!(lc.state(), StreamLifecycleState::Connected);
    }

    #[test]
    fn test_invalid_transitions() {
        let mut lc = StreamLifecycle::new("test".to_string(), DaemonPolicy::Always);

        // Created -> Active (invalid, must go through Connected)
        let event = lc.on_active();
        assert!(event.is_none());
        assert_eq!(lc.state(), StreamLifecycleState::Created);
    }

    #[test]
    fn test_daemon_policy_auto() {
        let mut lc = StreamLifecycle::new("test".to_string(), DaemonPolicy::Auto);

        assert!(!lc.should_source_run());

        let changed = lc.update_subscribers(1);
        assert!(changed);
        assert!(lc.should_source_run());

        let changed = lc.update_subscribers(2);
        assert!(!changed); // No boundary crossing
        assert!(lc.should_source_run());

        let changed = lc.update_subscribers(0);
        assert!(changed);
        assert!(!lc.should_source_run());
    }

    #[test]
    fn test_daemon_policy_always() {
        let mut lc = StreamLifecycle::new("test".to_string(), DaemonPolicy::Always);

        assert!(lc.should_source_run());

        let changed = lc.update_subscribers(0);
        assert!(!changed); // Always policy never signals
        assert!(lc.should_source_run());
    }
}
