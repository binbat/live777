pub mod req;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct ForwardInfo {
    pub id: String,
    #[serde(rename = "createTime")]
    pub create_time: i64,
    #[serde(rename = "publishLeaveTime")]
    pub publish_leave_time: i64,
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
    pub connect_state: u8,
}
