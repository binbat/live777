use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

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
}
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id: String,
    pub create_at: i64,
    pub state: RTCPeerConnectionState,
    pub cascade: Option<CascadeInfo>,
}

#[derive(Clone, Debug)]
pub struct Codec {
    pub kind: String,
    pub codec: String,
    pub fmtp: String,
}

#[derive(Clone, Debug)]
pub struct CascadeInfo {
    pub source_url: Option<String>,
    pub target_url: Option<String>,
    pub token: Option<String>,
    pub session_url: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ForwardEvent {
    pub r#type: ForwardEventType,
    pub session: String,
    pub stream_info: ForwardInfo,
}

#[derive(Clone, Debug)]
pub enum ForwardEventType {
    PublishUp,
    PublishDown,
    SubscribeUp,
    SubscribeDown,
    ReforwardUp,
    ReforwardDown,
}
