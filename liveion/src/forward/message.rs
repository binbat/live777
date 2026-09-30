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

impl Codec {
    /// Payload type advertised in a plain-RTP SDP (and stamped into
    /// forwarded packets) for this codec.
    ///
    /// When a WHIP-published stream is described before the first RTP packet
    /// arrives, the negotiated payload type is still 0 (not yet detected).
    /// Default to 96 (dynamic PT range) for video and use the static PT
    /// defined in RFC 3551 for well-known audio codecs so the SDP remains
    /// valid.
    #[allow(dead_code)]
    pub(crate) fn sdp_payload_type(&self) -> u8 {
        if self.payload_type != 0 {
            self.payload_type
        } else {
            match self.codec.as_str() {
                "pcma" => 8,
                "pcmu" => 0,
                "g722" => 9,
                _ => 96,
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct CascadeInfo {
    pub source_url: Option<String>,
    pub target_url: Option<String>,
    pub token: Option<String>,
    pub session_url: Option<String>,
}
