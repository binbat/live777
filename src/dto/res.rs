use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
pub struct Layer {
    #[serde(rename = "encodingId")]
    pub encoding_id: String,
}

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

impl From<crate::forward::info::Layer> for Layer {
    fn from(value: crate::forward::info::Layer) -> Self {
        Layer {
            encoding_id: value.encoding_id,
        }
    }
}

impl From<crate::forward::info::ForwardInfo> for ForwardInfo {
    fn from(value: crate::forward::info::ForwardInfo) -> Self {
        ForwardInfo {
            id: value.id,
            create_time: value.create_time,
            publish_leave_time: value.publish_leave_time,
            publish_session_info: value.publish_session_info.map(|session| session.into()),
            subscribe_session_infos: value
                .subscribe_session_infos
                .into_iter()
                .map(|session| session.into())
                .collect(),
        }
    }
}

impl From<crate::forward::info::SessionInfo> for SessionInfo {
    fn from(value: crate::forward::info::SessionInfo) -> Self {
        SessionInfo {
            id: value.id,
            create_time: value.create_time,
            connect_state: value.connect_state,
        }
    }
}
