use webrtc::peer_connection::RTCPeerConnectionState;

#[derive(Clone, Debug)]
pub struct Layer {
    pub encoding_id: String,
}

#[derive(Clone, Debug)]
pub struct ForwardInfo {
    pub id: String,
    pub create_at: i64,
    pub publish_leave_at: i64,
    pub subscribe_leave_at: i64,
    pub publish_session_info: Option<SessionInfo>,
    pub subscribe_session_infos: Vec<SessionInfo>,
    pub codecs: Vec<Codec>,
    pub has_virtual_publisher: bool,
    /// Stream-level media statistics: `publish` is the inbound (publisher)
    /// direction, `subscribe` the aggregate of all outbound subscribers.
    /// Cumulative counters are monotonic across republishes and subscriber
    /// churn; the bitrate is the current sampled rate.
    pub stats: api::response::StreamStats,
}
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id: String,
    pub create_at: i64,
    pub leave_at: i64,
    pub state: RTCPeerConnectionState,
    pub cascade: Option<CascadeInfo>,
    pub has_data_channel: bool,
    /// Media counters for this session: inbound for the publisher, outbound
    /// for a subscriber.
    pub stats: api::response::Stats,
}

#[derive(Clone, Debug)]
pub struct Codec {
    pub kind: String,
    pub codec: String,
    pub fmtp: String,
    pub payload_type: u8,
    pub clock_rate: u32,
    pub channels: u16,
}

#[derive(Clone, Debug)]
pub struct CascadeInfo {
    pub source_url: Option<String>,
    pub target_url: Option<String>,
    pub token: Option<String>,
    pub session_url: Option<String>,
}
