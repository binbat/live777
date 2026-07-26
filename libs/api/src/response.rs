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
    /// Snapshot consumers that dedup on equality must be aware these
    /// counters change continuously (both SSE and the net4mqtt xdata
    /// channel dedup on the serialized payload instead of `PartialEq`).
    #[serde(default)]
    pub stats: StreamStats,
    /// Scope of `stats`. `node` is one liveion node's media work;
    /// `clusterNodeWork` is liveman's sum of the node-level work for the
    /// same stream across nodes, including cascade hops.
    #[serde(default)]
    pub stats_scope: StatsScope,
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
    /// outbound for a subscribe session. Changes continuously; see the
    /// equality note on [`Stream::stats`].
    #[serde(default)]
    pub stats: Stats,
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

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StatsScope {
    /// Statistics from one liveion node.
    #[default]
    Node,
    /// Sum of node-level work across a liveman cluster. Internal cascade
    /// hops are intentionally counted as work on the relay nodes.
    ClusterNodeWork,
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
            stats,
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
            stats: StreamStats::default(),
            stats_scope: StatsScope::Node,
        }
    }

    #[test]
    fn eq_covers_all_fields_including_stats() {
        // `PartialEq` is honest: every field participates, stats included.
        // Snapshot dedup must therefore compare serialized payloads, never
        // `==` — see the doc on `Stream::stats`.
        let a = stream();
        let mut b = a.clone();
        b.stats.publish.bytes = 42;
        assert_ne!(a, b);
        let mut b = a.clone();
        b.stats_scope = StatsScope::ClusterNodeWork;
        assert_ne!(a, b);

        let a = session(Stats::default());
        let b = session(Stats {
            bytes: 100,
            packets: 1,
            bitrate: 800,
        });
        assert_ne!(a, b);

        let a = stream();
        let mut b = a.clone();
        b.provisioned = true;
        assert_ne!(a, b);
        b.provisioned = false;
        assert_eq!(a, b);
    }

    #[test]
    fn stats_serialize_as_plain_objects() {
        let value = serde_json::to_value(stream()).unwrap();
        assert_eq!(
            value["stats"],
            serde_json::json!({
                "publish": { "bytes": 0, "packets": 0, "bitrate": 0 },
                "subscribe": { "bytes": 0, "packets": 0, "bitrate": 0 },
            })
        );
        assert_eq!(value["statsScope"], serde_json::json!("node"));
        let mut cluster_stream = stream();
        cluster_stream.stats_scope = StatsScope::ClusterNodeWork;
        let value = serde_json::to_value(cluster_stream).unwrap();
        assert_eq!(value["statsScope"], serde_json::json!("clusterNodeWork"));

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
