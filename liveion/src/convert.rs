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

impl From<crate::forward::message::ForwardInfo> for api::response::Stream {
    fn from(value: crate::forward::message::ForwardInfo) -> Self {
        api::response::Stream {
            id: value.id,
            created_at: value.create_at,
            publish: api::response::PubSub {
                leave_at: value.publish_leave_at,
                sessions: match value.publish_session_info.map(|session| session.into()) {
                    Some(session) => vec![session],
                    None => vec![],
                },
            },
            subscribe: api::response::PubSub {
                leave_at: value.subscribe_leave_at,
                sessions: value
                    .subscribe_session_infos
                    .into_iter()
                    .map(|session| session.into())
                    .collect(),
            },
        }
    }
}

impl From<crate::forward::message::SessionInfo> for api::response::Session {
    fn from(value: crate::forward::message::SessionInfo) -> Self {
        api::response::Session {
            id: value.id,
            created_at: value.create_at,
            state: convert_connect_state(value.state),
            cascade: value.cascade.map(|reforward| reforward.into()),
        }
    }
}

impl From<crate::forward::message::CascadeInfo> for api::response::CascadeInfo {
    fn from(value: crate::forward::message::CascadeInfo) -> Self {
        api::response::CascadeInfo {
            target_url: value.target_url,
            session_url: value.session_url,
            source_url: value.source_url,
        }
    }
}

impl From<Config> for NodeMetaData {
    fn from(value: Config) -> Self {
        Self {
            authorization: value.auth.to_authorizations().first().cloned(),
        }
    }
}

fn convert_connect_state(state: RTCPeerConnectionState) -> api::response::RTCPeerConnectionState {
    match state {
        RTCPeerConnectionState::Unspecified | RTCPeerConnectionState::New => {
            api::response::RTCPeerConnectionState::New
        }
        RTCPeerConnectionState::Connecting => api::response::RTCPeerConnectionState::Connecting,
        RTCPeerConnectionState::Connected => api::response::RTCPeerConnectionState::Connected,
        RTCPeerConnectionState::Disconnected => api::response::RTCPeerConnectionState::Disconnected,
        RTCPeerConnectionState::Failed => api::response::RTCPeerConnectionState::Failed,
        RTCPeerConnectionState::Closed => api::response::RTCPeerConnectionState::Closed,
    }
}
