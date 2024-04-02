use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
pub struct LayerRes {
    #[serde(rename = "encodingId")]
    pub encoding_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct ForwardInfoRes {
    pub id: String,
    #[serde(rename = "createTime")]
    pub create_time: i64,
    #[serde(rename = "publishLeaveTime")]
    pub publish_leave_time: i64,
    #[serde(rename = "publishSessionInfo")]
    pub publish_session_info: Option<SessionInfoRes>,
    #[serde(rename = "subscribeSessionInfos")]
    pub subscribe_session_infos: Vec<SessionInfoRes>,
}

#[derive(Serialize, Deserialize)]
pub struct SessionInfoRes {
    pub id: String,
    #[serde(rename = "createTime")]
    pub create_time: i64,
    #[serde(rename = "connectState")]
    pub connect_state: u8,
    #[serde(skip_serializing_if = "Option::is_none", rename = "reForward")]
    pub re_forward: Option<ReForwardInfo>,
}

#[derive(Serialize, Deserialize)]
pub struct ReForwardInfo {
    #[serde(rename = "whipUrl")]
    pub whip_url: String,
    #[serde(rename = "resourceUrl")]
    pub resource_url: Option<String>,
}

impl From<crate::forward::info::Layer> for LayerRes {
    fn from(value: crate::forward::info::Layer) -> Self {
        LayerRes {
            encoding_id: value.encoding_id,
        }
    }
}

impl From<crate::forward::info::ForwardInfo> for ForwardInfoRes {
    fn from(value: crate::forward::info::ForwardInfo) -> Self {
        ForwardInfoRes {
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

impl From<crate::forward::info::SessionInfo> for SessionInfoRes {
    fn from(value: crate::forward::info::SessionInfo) -> Self {
        SessionInfoRes {
            id: value.id,
            create_time: value.create_time,
            connect_state: value.connect_state,
            re_forward: value.re_forward.map(|re_forward| re_forward.into()),
        }
    }
}

impl From<crate::forward::info::ReForwardInfo> for ReForwardInfo {
    fn from(value: crate::forward::info::ReForwardInfo) -> Self {
        ReForwardInfo {
            whip_url: value.whip_url,
            resource_url: value.resource_url,
        }
    }
}
