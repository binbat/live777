use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EventBody {
    pub metrics: NodeMetrics,
    pub event: Event,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Event {
    Stream {
        r#type: StreamEventType,
        stream: Stream,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub enum StreamEventType {
    StreamUp,
    StreamDown,
    PublishUp,
    PublishDown,
    SubscribeUp,
    SubscribeDown,
    ReforwardUp,
    ReforwardDown,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Stream {
    pub stream: String,
    pub session: Option<String>,
    pub publish: u64,
    pub subscribe: u64,
    pub reforward: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NodeMetrics {
    pub stream: u64,
    pub publish: u64,
    pub subscribe: u64,
    pub reforward: u64,
}
