#[derive(Clone)]
pub struct Layer {
    pub encoding_id: String,
}

#[derive(Clone)]
pub struct ForwardInfo {
    pub id: String,
    pub create_time: i64,
    pub publish_leave_time: i64,
    pub publish_session_info: Option<SessionInfo>,
    pub subscribe_session_infos: Vec<SessionInfo>,
}
#[derive(Clone)]
pub struct SessionInfo {
    pub id: String,
    pub create_time: i64,
    pub connect_state: u8,
    pub re_forward: Option<ReForwardInfo>,
}

#[derive(Clone)]
pub struct ReForwardInfo {
    pub whip_url: String,
    pub basic: Option<String>,
    pub token: Option<String>,
    pub resource_url: Option<String>,
}
