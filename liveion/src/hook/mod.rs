pub mod convert;

use crate::forward::message::ForwardEvent;

#[derive(Clone, Debug)]
pub enum Event {
    Stream(StreamEvent),
    Forward(ForwardEvent),
}

#[derive(Clone, Debug)]
pub struct StreamEvent {
    pub r#type: StreamEventType,
    pub stream: Stream,
}

#[derive(Clone, Debug)]
pub enum StreamEventType {
    Up,
    Down,
}

#[derive(Clone, Debug)]
pub struct Stream {
    pub stream: String,
    pub session: Option<String>,
    pub publish: u64,
    pub subscribe: u64,
    pub reforward: u64,
}
