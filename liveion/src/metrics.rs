use prometheus::{Gauge, Registry, TextEncoder};
use std::sync::LazyLock;

pub static STREAM: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("stream", "stream number").unwrap());
pub static PUBLISH: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("publish", "publish number").unwrap());
pub static SUBSCRIBE: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("subscribe", "subscribe number").unwrap());
pub static REFORWARD: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("reforward", "reforward number").unwrap());
pub static REGISTRY: LazyLock<Registry> =
    LazyLock::new(|| Registry::new_custom(Some("live777".to_string()), None).unwrap());
pub static ENCODER: LazyLock<TextEncoder> = LazyLock::new(TextEncoder::new);

pub fn node_metrics() -> api::event::NodeMetrics {
    api::event::NodeMetrics {
        stream: STREAM.get() as u64,
        publish: PUBLISH.get() as u64,
        subscribe: SUBSCRIBE.get() as u64,
        reforward: REFORWARD.get() as u64,
    }
}
