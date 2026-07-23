//! Typed stream-lifecycle events.
//!
//! All events travel on a single `broadcast` bus owned by
//! [`crate::stream::manager::Manager`]. Consumers:
//!
//! - SSE `/api/sse/streams` and net4mqtt — snapshot consumers that treat every
//!   event as a "something changed" ping and re-send a full snapshot.
//! - recorder — reacts to `StreamCreated`/`StreamDeleted` for auto start/stop.
//! - hook — runs user scripts on `StreamCreated`/`StreamDeleted`. It cannot
//!   reconcile after lag (a missed script cannot be rerun), so its
//!   dispatcher only forwards events into an internal queue and never
//!   awaits a script while holding the bus receiver.
//! - the manager's event logger — emits one canonical debug line per event
//!   with its full payload.
//!
//! Consumers MUST tolerate `broadcast::error::RecvError::Lagged`: snapshot
//! consumers re-sync by re-snapshotting, and the recorder reconciles its task
//! set against the manager — a `while let Ok(..)` loop exits silently on lag
//! and has bitten us before.

/// Why a stream was torn down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamDeleteReason {
    /// Admin API (`DELETE /api/streams/{stream}`) or a session-kick cascade.
    ApiDeleted,
    /// `strategy.auto_delete_whip` fired after the publisher left.
    PublishLeaveTimeout,
    /// `strategy.auto_delete_whep` fired after the last subscriber left.
    SubscribeLeaveTimeout,
    /// No publisher and no subscribers; reclaimed after the orphan grace period.
    Orphaned,
    /// Internal reset of a provisioned stream back to standby (RTSP
    /// re-ANNOUNCE, publisher-leave cascade). Always paired with an
    /// immediate `StreamCreated`; the registration itself never lapses.
    Reset,
}

/// Why a publish/subscribe session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionStopReason {
    /// The PeerConnection reached `Closed` (including the `Failed` -> close ->
    /// `Closed` cascade and stream teardown closing live sessions).
    PeerClosed,
    /// Ended via `DELETE /api/session/{stream}/{session}` — either the client
    /// gracefully hanging up (WHIP/WHEP session delete) or an admin kick; both
    /// share the endpoint and are indistinguishable at this layer.
    ApiDeleted,
    /// An on-demand source was stopped because the stream had no consumers
    /// for `on_demand_close_after_ms` (no subscribers, no RTSP pull clients).
    #[cfg_attr(not(feature = "source"), allow(dead_code))]
    IdleTimeout,
}

/// A stream-lifecycle event. `stream` is the stream name; `session` is the
/// session ID for publish/subscribe events.
#[derive(Clone, Debug)]
pub enum Event {
    /// The stream now exists in the manager (created via API, auto-create, or
    /// source startup).
    StreamCreated { stream: String },
    /// The stream was removed from the manager.
    StreamDeleted {
        stream: String,
        reason: StreamDeleteReason,
    },
    /// A WHIP/cascade publisher session was established.
    PublishStarted { stream: String, session: String },
    /// A publisher session ended.
    PublishStopped {
        stream: String,
        session: String,
        reason: SessionStopReason,
    },
    /// A WHIP/cascade subscriber session was established.
    SubscribeStarted { stream: String, session: String },
    /// A subscriber session ended.
    SubscribeStopped {
        stream: String,
        session: String,
        reason: SessionStopReason,
    },
    /// Content-free "stream state changed" ping for snapshot consumers (SSE,
    /// net4mqtt). Fired alongside every publish/subscribe transition above,
    /// plus track, connection-state, and closed-session changes. `StreamCreated`/
    /// `StreamDeleted` are emitted by the manager and carry no paired ping —
    /// snapshot consumers must treat every event as a change hint anyway.
    ForwardChanged { stream: String },
}

impl Event {
    pub fn stream(&self) -> &str {
        match self {
            Event::StreamCreated { stream }
            | Event::StreamDeleted { stream, .. }
            | Event::PublishStarted { stream, .. }
            | Event::PublishStopped { stream, .. }
            | Event::SubscribeStarted { stream, .. }
            | Event::SubscribeStopped { stream, .. }
            | Event::ForwardChanged { stream } => stream,
        }
    }
}
