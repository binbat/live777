use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub version: String,
    pub git_hash: String,
    pub build_time: String,
    pub features: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub encoding_id: String,
}

/// Wrapper for continuously-updating fields that must not participate in
/// equality. Snapshot dedup (SSE `Manager::sse_handler`, the net4mqtt
/// snapshot comparison) relies on `==` to suppress unchanged snapshots, and
/// live counters would make every snapshot differ. `Live` compares always
/// equal and serializes transparently, so the parent struct keeps a plain
/// derived `PartialEq` and fields added later automatically participate in
/// dedup — no hand-written field list to drift out of sync.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(transparent)]
pub struct Live<T>(pub T);

impl<T> PartialEq for Live<T> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}
impl<T> Eq for Live<T> {}

impl<T> std::ops::Deref for Live<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Live<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub id: String,
    pub created_at: i64,
    pub publish: PubSub,
    pub subscribe: PubSub,
    pub codecs: Vec<Codec>,
    /// Declared in the server config file (`[stream.<id>]`). Provisioned
    /// streams always exist (shown even when idle) and are exempt from
    /// automatic teardown.
    #[serde(default)]
    pub provisioned: bool,
    /// This stream's configured sources start on the first subscriber and
    /// stop after the last one leaves (implies `provisioned`).
    #[serde(default)]
    pub on_demand: bool,
    /// Stream-level media statistics: `publish` is the inbound (publisher)
    /// side, `subscribe` the sum of all outbound subscriber sessions.
    #[serde(default)]
    pub stats: Live<StreamStats>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PubSub {
    pub leave_at: i64,
    pub sessions: Vec<Session>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub created_at: i64,
    pub leave_at: i64,
    pub state: RTCPeerConnectionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cascade: Option<CascadeInfo>,
    pub has_data_channel: bool,
    /// Media statistics for this session: inbound for a publish session,
    /// outbound for a subscribe session.
    #[serde(default)]
    pub stats: Live<Stats>,
}

/// Media statistics counters. `bitrate` is the rate over the last sampling
/// interval, in bits per second; `bytes`/`packets` are cumulative.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub bytes: u64,
    pub packets: u64,
    pub bitrate: u64,
}

/// Per-stream statistics: `publish` is the inbound (publisher) direction,
/// `subscribe` the aggregate of all outbound subscriber sessions.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StreamStats {
    pub publish: Stats,
    pub subscribe: Stats,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Codec {
    pub kind: String,
    pub codec: String,
    pub fmtp: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CascadeInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_url: Option<String>,
}

/// PeerConnectionState indicates the state of the PeerConnection.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RTCPeerConnectionState {
    /// PeerConnectionStateNew indicates that any of the ICETransports or
    /// DTLSTransports are in the "new" state and none of the transports are
    /// in the "connecting", "checking", "failed" or "disconnected" state, or
    /// all transports are in the "closed" state, or there are no transports.
    #[default]
    #[serde(rename = "new")]
    New,

    /// PeerConnectionStateConnecting indicates that any of the
    /// ICETransports or DTLSTransports are in the "connecting" or
    /// "checking" state and none of them is in the "failed" state.
    #[serde(rename = "connecting")]
    Connecting,

    /// PeerConnectionStateConnected indicates that all ICETransports and
    /// DTLSTransports are in the "connected", "completed" or "closed" state
    /// and at least one of them is in the "connected" or "completed" state.
    #[serde(rename = "connected")]
    Connected,

    /// PeerConnectionStateDisconnected indicates that any of the
    /// ICETransports or DTLSTransports are in the "disconnected" state
    /// and none of them are in the "failed" or "connecting" or "checking" state.
    #[serde(rename = "disconnected")]
    Disconnected,

    /// PeerConnectionStateFailed indicates that any of the ICETransports
    /// or DTLSTransports are in a "failed" state.
    #[serde(rename = "failed")]
    Failed,

    /// PeerConnectionStateClosed indicates the peer connection is closed
    /// and the isClosed member variable of PeerConnection is true.
    #[serde(rename = "closed")]
    Closed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(stats: Stats) -> Session {
        Session {
            id: "s1".to_string(),
            created_at: 1,
            leave_at: 0,
            state: RTCPeerConnectionState::Connected,
            cascade: None,
            has_data_channel: false,
            stats: Live(stats),
        }
    }

    fn stream() -> Stream {
        Stream {
            id: "live".to_string(),
            created_at: 1,
            publish: PubSub {
                leave_at: 0,
                sessions: vec![],
            },
            subscribe: PubSub {
                leave_at: 0,
                sessions: vec![],
            },
            codecs: vec![],
            provisioned: false,
            on_demand: false,
            stats: Live(StreamStats::default()),
        }
    }

    #[test]
    fn session_eq_ignores_stats() {
        let a = session(Stats::default());
        let b = session(Stats {
            bytes: 100,
            packets: 1,
            bitrate: 800,
        });
        assert_eq!(a, b);
    }

    #[test]
    fn stream_eq_ignores_stats() {
        let mut a = stream();
        let mut b = a.clone();
        b.stats.publish.bytes = 42;
        assert_eq!(a, b);
        b.publish.sessions.push(session(Stats {
            bytes: 7,
            packets: 1,
            bitrate: 56,
        }));
        a.publish.sessions.push(session(Stats::default()));
        assert_eq!(a, b);
        b.id = "other".to_string();
        assert_ne!(a, b);
    }

    #[test]
    fn derived_eq_covers_non_stats_fields() {
        // The whole point of `Live`: fields added to the structs compare
        // via the derived `PartialEq` without touching a hand-written list.
        let a = stream();
        let mut b = a.clone();
        b.provisioned = true;
        assert_ne!(a, b);
    }

    #[test]
    fn live_serializes_transparently() {
        let value = serde_json::to_value(stream()).unwrap();
        // `stats` is the plain inner object on the wire, not a wrapper.
        assert_eq!(
            value["stats"],
            serde_json::json!({
                "publish": { "bytes": 0, "packets": 0, "bitrate": 0 },
                "subscribe": { "bytes": 0, "packets": 0, "bitrate": 0 },
            })
        );
        let session_value = serde_json::to_value(session(Stats {
            bytes: 1,
            packets: 2,
            bitrate: 3,
        }))
        .unwrap();
        assert_eq!(
            session_value["stats"],
            serde_json::json!({ "bytes": 1, "packets": 2, "bitrate": 3 })
        );
    }
}
