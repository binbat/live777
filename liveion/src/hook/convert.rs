use api::event::{NodeEventType, NodeMetaData};

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

impl From<StreamEventType> for api::event::StreamEventType {
    fn from(value: StreamEventType) -> Self {
        match value {
            StreamEventType::Up => api::event::StreamEventType::StreamUp,
            StreamEventType::Down => api::event::StreamEventType::StreamDown,
        }
    }
}

impl From<Stream> for api::event::Stream {
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

impl From<message::ForwardEventType> for api::event::StreamEventType {
    fn from(value: message::ForwardEventType) -> Self {
        match value {
            message::ForwardEventType::PublishUp => api::event::StreamEventType::PublishUp,
            message::ForwardEventType::PublishDown => {
                api::event::StreamEventType::PublishDown
            }
            message::ForwardEventType::SubscribeUp => {
                api::event::StreamEventType::SubscribeUp
            }
            message::ForwardEventType::SubscribeDown => {
                api::event::StreamEventType::SubscribeDown
            }
            message::ForwardEventType::ReforwardUp => {
                api::event::StreamEventType::ReforwardUp
            }
            message::ForwardEventType::ReforwardDown => {
                api::event::StreamEventType::ReforwardDown
            }
        }
    }
}

impl From<message::ForwardEvent> for api::event::Event {
    fn from(value: message::ForwardEvent) -> Self {
        api::event::Event::Stream {
            r#type: value.r#type.into(),
            stream: api::event::Stream {
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
    pub fn convert_api_event(self, metadata: NodeMetaData) -> api::event::Event {
        match self {
            Event::Node(event) => api::event::Event::Node {
                r#type: event.into(),
                metadata,
            },
            Event::Stream(stream_evnet) => api::event::Event::Stream {
                r#type: stream_evnet.r#type.into(),
                stream: stream_evnet.stream.into(),
            },
            Event::Forward(forward_event) => forward_event.into(),
        }
    }
}
