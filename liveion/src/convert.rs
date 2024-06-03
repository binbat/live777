use live777_http::event::NodeMetaData;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;

use crate::config::Config;

impl From<crate::forward::message::Layer> for live777_http::response::Layer {
    fn from(value: crate::forward::message::Layer) -> Self {
        live777_http::response::Layer {
            encoding_id: value.encoding_id,
        }
    }
}

impl From<crate::forward::message::ForwardInfo> for live777_http::response::StreamInfo {
    fn from(value: crate::forward::message::ForwardInfo) -> Self {
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

impl From<crate::forward::message::SessionInfo> for live777_http::response::SessionInfo {
    fn from(value: crate::forward::message::SessionInfo) -> Self {
        live777_http::response::SessionInfo {
            id: value.id,
            create_time: value.create_time,
            connect_state: convert_connect_state(value.connect_state),
            reforward: value.reforward.map(|reforward| reforward.into()),
        }
    }
}

impl From<crate::forward::message::ReforwardInfo> for live777_http::response::ReforwardInfo {
    fn from(value: crate::forward::message::ReforwardInfo) -> Self {
        live777_http::response::ReforwardInfo {
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
            pub_max: value.stream_info.pub_max.0,
            sub_max: value.stream_info.sub_max.0,
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
