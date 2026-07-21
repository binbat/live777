//! Typed stream-lifecycle events.
//!
//! All events travel on a single `broadcast` bus owned by
//! [`crate::stream::manager::Manager`]. Consumers:
//!
//! - SSE `/api/sse/streams` and net4mqtt — snapshot consumers that treat every
//!   event as a "something changed" ping and re-send a full snapshot.
//! - recorder — reacts to `StreamUp`/`StreamDown` for auto start/stop.
//! - the manager's event logger — emits one canonical debug line per event
//!   with its full payload.
//!
//! Consumers MUST tolerate `broadcast::error::RecvError::Lagged` by continuing
//! the loop (and re-snapshotting where applicable); a `while let Ok(..)` loop
//! exits silently on lag and has bitten us before.

/// Why a stream was torn down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamDownReason {
    /// Admin API (`DELETE /api/streams/{stream}`) or a session-kick cascade.
    ApiDeleted,
    /// `strategy.auto_delete_whip` fired after the publisher left.
    PublishLeaveTimeout,
    /// `strategy.auto_delete_whep` fired after the last subscriber left.
    SubscribeLeaveTimeout,
    /// No publisher and no subscribers; reclaimed after the orphan grace period.
    Orphaned,
}

/// Why a publish/subscribe session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionDownReason {
    /// The PeerConnection reached `Closed` (including the `Failed` -> close ->
    /// `Closed` cascade and stream teardown closing live sessions).
    PeerClosed,
    /// Explicitly kicked via the admin session API.
    ApiKicked,
}

/// A stream-lifecycle event. `stream` is the stream name; `session` is the
/// session ID for publish/subscribe events.
#[derive(Clone, Debug)]
pub enum Event {
    /// The stream now exists in the manager (created via API, auto-create, or
    /// source startup).
    StreamUp { stream: String },
    /// The stream was removed from the manager.
    StreamDown {
        stream: String,
        reason: StreamDownReason,
    },
    /// A WHIP/cascade publisher session was established.
    PublishUp { stream: String, session: String },
    /// A publisher session ended.
    PublishDown {
        stream: String,
        session: String,
        reason: SessionDownReason,
    },
    /// A WHIP/cascade subscriber session was established.
    SubscribeUp { stream: String, session: String },
    /// A subscriber session ended.
    SubscribeDown {
        stream: String,
        session: String,
        reason: SessionDownReason,
    },
    /// Content-free "stream state changed" ping for snapshot consumers (SSE,
    /// net4mqtt). Fired on every finer-grained transition above plus track,
    /// connection-state, and closed-session changes.
    ForwardChanged { stream: String },
}

impl Event {
    pub fn stream(&self) -> &str {
        match self {
            Event::StreamUp { stream }
            | Event::StreamDown { stream, .. }
            | Event::PublishUp { stream, .. }
            | Event::PublishDown { stream, .. }
            | Event::SubscribeUp { stream, .. }
            | Event::SubscribeDown { stream, .. }
            | Event::ForwardChanged { stream } => stream,
        }
    }
}
