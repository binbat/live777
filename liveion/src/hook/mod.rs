#[cfg(feature = "webhook")]
pub mod convert;

#[cfg(feature = "webhook")]
pub mod webhook;

#[cfg(feature = "webhook")]
use async_trait::async_trait;

#[cfg(feature = "webhook")]
use tokio::sync::broadcast;

use std::fmt::Debug;

use crate::forward::message::ForwardEvent;

#[derive(Clone, Debug)]
pub enum Event {
    Node(NodeEvent),
    Stream(StreamEvent),
    Forward(ForwardEvent),
}

#[derive(Clone, Debug)]
pub enum NodeEvent {
    Up,
    KeepAlive,
    Down,
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

#[cfg(feature = "webhook")]
#[async_trait]
pub trait EventHook: Debug {
    async fn hook(&self, mut event_receiver: broadcast::Receiver<Event>);
}
