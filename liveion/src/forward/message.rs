use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

#[derive(Clone, Debug)]
pub struct Layer {
    pub encoding_id: String,
}

#[derive(Clone, Debug)]
pub struct ForwardInfo {
    pub id: String,
    pub create_time: i64,
    pub publish_leave_time: i64,
    pub subscribe_leave_time: i64,
    pub publish_session_info: Option<SessionInfo>,
    pub subscribe_session_infos: Vec<SessionInfo>,
}
#[derive(Clone, Debug)]
pub struct SessionInfo {
    pub id: String,
    pub create_time: i64,
    pub connect_state: RTCPeerConnectionState,
    pub cascade: Option<CascadeInfo>,
}

#[derive(Clone, Debug)]
pub struct CascadeInfo {
    pub src: Option<String>,
    pub dst: Option<String>,
    pub token: Option<String>,
    pub resource: Option<String>,
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
