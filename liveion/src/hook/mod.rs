use crate::forward::message::ForwardEvent;

#[derive(Clone, Debug)]
pub enum Event {
    Stream(StreamEvent),
    Forward(ForwardEvent),
}

#[derive(Clone, Debug)]
pub struct StreamEvent {
    // Only the recorder feature consumes the event type.
    #[cfg_attr(not(feature = "recorder"), allow(dead_code))]
    pub r#type: StreamEventType,
    pub stream: Stream,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEventType {
    Up,
    Down,
}

#[derive(Clone, Debug)]
pub struct Stream {
    pub stream: String,
}
