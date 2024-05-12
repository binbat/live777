use live777_http::event::{NodeEventType, NodeMetaData};

use crate::forward::message;

use super::{Event, NodeEvent, Stream, StreamEventType};

impl From<NodeEvent> for NodeEventType {
    fn from(value: NodeEvent) -> Self {
        match value {
            NodeEvent::Up => NodeEventType::Up,
            NodeEvent::KeepAlive => NodeEventType::KeepAlive,
            NodeEvent::Down => NodeEventType::Down,
        }
    }
}

impl From<StreamEventType> for live777_http::event::StreamEventType {
    fn from(value: StreamEventType) -> Self {
        match value {
            StreamEventType::Up => live777_http::event::StreamEventType::StreamUp,
            StreamEventType::Down => live777_http::event::StreamEventType::StreamDown,
        }
    }
}

impl From<Stream> for live777_http::event::Stream {
    fn from(value: Stream) -> Self {
        Self {
            stream: value.stream,
            session: value.session,
            publish: value.publish,
            subscribe: value.subscribe,
            reforward: value.reforward,
        }
    }
}

impl From<message::ForwardEventType> for live777_http::event::StreamEventType {
    fn from(value: message::ForwardEventType) -> Self {
        match value {
            message::ForwardEventType::PublishUp => live777_http::event::StreamEventType::PublishUp,
            message::ForwardEventType::PublishDown => {
                live777_http::event::StreamEventType::PublishDown
            }
            message::ForwardEventType::SubscribeUp => {
                live777_http::event::StreamEventType::SubscribeUp
            }
            message::ForwardEventType::SubscribeDown => {
                live777_http::event::StreamEventType::SubscribeDown
            }
            message::ForwardEventType::ReforwardUp => {
                live777_http::event::StreamEventType::ReforwardUp
            }
            message::ForwardEventType::ReforwardDown => {
                live777_http::event::StreamEventType::ReforwardDown
            }
        }
    }
}

impl From<message::ForwardEvent> for live777_http::event::Event {
    fn from(value: message::ForwardEvent) -> Self {
        live777_http::event::Event::Stream {
            r#type: value.r#type.into(),
            stream: live777_http::event::Stream {
                stream: value.stream_info.id,
                session: Some(value.session),
                publish: if value.stream_info.publish_session_info.is_some() {
                    1
                } else {
                    0
                },
                subscribe: value.stream_info.subscribe_session_infos.len() as u64,
                reforward: value
                    .stream_info
                    .subscribe_session_infos
                    .iter()
                    .filter(|session| session.reforward.is_some())
                    .count() as u64,
            },
        }
    }
}

impl Event {
    pub fn convert_live777_http_event(self, metadata: NodeMetaData) -> live777_http::event::Event {
        match self {
            Event::Node(event) => live777_http::event::Event::Node {
                r#type: event.into(),
                metadata,
            },
            Event::Stream(stream_evnet) => live777_http::event::Event::Stream {
                r#type: stream_evnet.r#type.into(),
                stream: stream_evnet.stream.into(),
            },
            Event::Forward(forward_event) => forward_event.into(),
        }
    }
}
