use api::event::NodeMetaData;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

use crate::config::Config;

impl From<crate::forward::message::Layer> for api::response::Layer {
    fn from(value: crate::forward::message::Layer) -> Self {
        api::response::Layer {
            encoding_id: value.encoding_id,
        }
    }
}

impl From<crate::forward::message::ForwardInfo> for api::response::StreamInfo {
    fn from(value: crate::forward::message::ForwardInfo) -> Self {
        api::response::StreamInfo {
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

impl From<crate::forward::message::SessionInfo> for api::response::SessionInfo {
    fn from(value: crate::forward::message::SessionInfo) -> Self {
        api::response::SessionInfo {
            id: value.id,
            create_time: value.create_time,
            connect_state: convert_connect_state(value.connect_state),
            reforward: value.reforward.map(|reforward| reforward.into()),
        }
    }
}

impl From<crate::forward::message::ReforwardInfo> for api::response::ReforwardInfo {
    fn from(value: crate::forward::message::ReforwardInfo) -> Self {
        api::response::ReforwardInfo {
            target_url: value.target_url,
            resource_url: value.resource_url,
        }
    }
}

impl From<Config> for NodeMetaData {
    fn from(value: Config) -> Self {
        Self {
            authorization: value.auth.to_authorizations().first().cloned(),
            admin_authorization: value.admin_auth.to_authorizations().first().cloned(),
        }
    }
}

fn convert_connect_state(
    connect_state: RTCPeerConnectionState,
) -> api::response::RTCPeerConnectionState {
    match connect_state {
        RTCPeerConnectionState::Unspecified => api::response::RTCPeerConnectionState::New,

        RTCPeerConnectionState::New => api::response::RTCPeerConnectionState::New,
        RTCPeerConnectionState::Connecting => api::response::RTCPeerConnectionState::Connecting,

        RTCPeerConnectionState::Connected => api::response::RTCPeerConnectionState::Connected,

        RTCPeerConnectionState::Disconnected => api::response::RTCPeerConnectionState::Disconnected,

        RTCPeerConnectionState::Failed => api::response::RTCPeerConnectionState::Failed,

        RTCPeerConnectionState::Closed => api::response::RTCPeerConnectionState::Closed,
    }
}
