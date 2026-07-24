use api::response::Codec;
use webrtc::peer_connection::RTCPeerConnectionState;

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
            codecs: value
                .codecs
                .into_iter()
                .map(|media_code| Codec {
                    kind: media_code.kind,
                    codec: media_code.codec,
                    fmtp: media_code.fmtp,
                })
                .collect(),
            // Config-derived flags are backfilled by the manager, which owns
            // the stream config; the forward itself doesn't know them.
            provisioned: false,
            on_demand: false,
            stats: value.stats,
        }
    }
}

impl From<crate::forward::message::SessionInfo> for api::response::Session {
    fn from(value: crate::forward::message::SessionInfo) -> Self {
        api::response::Session {
            id: value.id,
            created_at: value.create_at,
            leave_at: value.leave_at,
            state: convert_connect_state(value.state),
            cascade: value.cascade.map(|reforward| {
                #[cfg(feature = "cascade")]
                {
                    reforward.into()
                }
                #[cfg(not(feature = "cascade"))]
                {
                    let _ = reforward;
                    api::response::CascadeInfo {
                        source_url: None,
                        target_url: None,
                        session_url: None,
                    }
                }
            }),
            has_data_channel: value.has_data_channel,
            stats: value.stats,
        }
    }
}

#[cfg(feature = "cascade")]
impl From<crate::forward::message::CascadeInfo> for api::response::CascadeInfo {
    fn from(value: crate::forward::message::CascadeInfo) -> Self {
        api::response::CascadeInfo {
            target_url: value.target_url,
            session_url: value.session_url,
            source_url: value.source_url,
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
