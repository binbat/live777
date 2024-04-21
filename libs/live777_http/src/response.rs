use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
pub struct Layer {
    #[serde(rename = "encodingId")]
    pub encoding_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct StreamInfo {
    pub id: String,
    #[serde(rename = "createTime")]
    pub create_time: i64,
    #[serde(rename = "publishLeaveTime")]
    pub publish_leave_time: i64,
    #[serde(rename = "subscribeLeaveTime")]
    pub subscribe_leave_time: i64,
    #[serde(rename = "publishSessionInfo")]
    pub publish_session_info: Option<SessionInfo>,
    #[serde(rename = "subscribeSessionInfos")]
    pub subscribe_session_infos: Vec<SessionInfo>,
}

#[derive(Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    #[serde(rename = "createTime")]
    pub create_time: i64,
    #[serde(rename = "connectState")]
    pub connect_state: RTCPeerConnectionState,
    #[serde(skip_serializing_if = "Option::is_none", rename = "reforward")]
    pub reforward: Option<ReforwardInfo>,
}

#[derive(Serialize, Deserialize)]
pub struct ReforwardInfo {
    #[serde(rename = "targetUrl")]
    pub target_url: String,
    #[serde(rename = "resourceUrl")]
    pub resource_url: Option<String>,
}

/// PeerConnectionState indicates the state of the PeerConnection.
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RTCPeerConnectionState {
    #[default]
    #[serde(rename = "Unspecified")]
    Unspecified,

    /// PeerConnectionStateNew indicates that any of the ICETransports or
    /// DTLSTransports are in the "new" state and none of the transports are
    /// in the "connecting", "checking", "failed" or "disconnected" state, or
    /// all transports are in the "closed" state, or there are no transports.
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
