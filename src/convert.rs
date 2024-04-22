use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

impl From<crate::forward::info::Layer> for live777_http::response::Layer {
    fn from(value: crate::forward::info::Layer) -> Self {
        live777_http::response::Layer {
            encoding_id: value.encoding_id,
        }
    }
}

impl From<crate::forward::info::StreamInfo> for live777_http::response::StreamInfo {
    fn from(value: crate::forward::info::StreamInfo) -> Self {
        live777_http::response::StreamInfo {
            id: value.id,
            create_time: value.create_time,
            publish_leave_time: value.publish_leave_time,
            subscribe_leave_time: value.subscribe_leave_time,
            publish_session_info: value.publish_session_info.map(|session| session.into()),
            subscribe_session_infos: value
                .subscribe_session_infos
                .into_iter()
                .map(|session| session.into())
                .collect(),
        }
    }
}

impl From<crate::forward::info::SessionInfo> for live777_http::response::SessionInfo {
    fn from(value: crate::forward::info::SessionInfo) -> Self {
        live777_http::response::SessionInfo {
            id: value.id,
            create_time: value.create_time,
            connect_state: convert_connect_state(value.connect_state),
            reforward: value.reforward.map(|reforward| reforward.into()),
        }
    }
}

impl From<crate::forward::info::ReforwardInfo> for live777_http::response::ReforwardInfo {
    fn from(value: crate::forward::info::ReforwardInfo) -> Self {
        live777_http::response::ReforwardInfo {
            target_url: value.target_url,
            resource_url: value.resource_url,
        }
    }
}

fn convert_connect_state(
    connect_state: RTCPeerConnectionState,
) -> live777_http::response::RTCPeerConnectionState {
    match connect_state {
        RTCPeerConnectionState::Unspecified => {
            live777_http::response::RTCPeerConnectionState::Unspecified
        }

        RTCPeerConnectionState::New => live777_http::response::RTCPeerConnectionState::New,
        RTCPeerConnectionState::Connecting => {
            live777_http::response::RTCPeerConnectionState::Connecting
        }

        RTCPeerConnectionState::Connected => {
            live777_http::response::RTCPeerConnectionState::Connected
        }

        RTCPeerConnectionState::Disconnected => {
            live777_http::response::RTCPeerConnectionState::Disconnected
        }

        RTCPeerConnectionState::Failed => live777_http::response::RTCPeerConnectionState::Failed,

        RTCPeerConnectionState::Closed => live777_http::response::RTCPeerConnectionState::Closed,
    }
}

impl From<crate::config::StorageModel> for live777_storage::StorageModel {
    fn from(value: crate::config::StorageModel) -> Self {
        match value {
            crate::config::StorageModel::RedisStandalone { addr } => {
                live777_storage::StorageModel::RedisStandalone { addr }
            }
        }
    }
}
