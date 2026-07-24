use prometheus::{Gauge, IntCounter, Registry, TextEncoder};
use std::sync::LazyLock;

pub static STREAM: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("stream", "stream number").unwrap());
pub static PUBLISH: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("publish", "publish number").unwrap());
pub static SUBSCRIBE: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("subscribe", "subscribe number").unwrap());
pub static REFORWARD: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("reforward", "reforward number").unwrap());
/// Server-wide RTP bytes received from publishers (cumulative).
pub static BYTES_IN_TOTAL: LazyLock<IntCounter> = LazyLock::new(|| {
    IntCounter::new("bytes_in_total", "RTP bytes received from publishers").unwrap()
});
/// Server-wide RTP bytes sent to subscribers (cumulative).
pub static BYTES_OUT_TOTAL: LazyLock<IntCounter> =
    LazyLock::new(|| IntCounter::new("bytes_out_total", "RTP bytes sent to subscribers").unwrap());
pub static REGISTRY: LazyLock<Registry> =
    LazyLock::new(|| Registry::new_custom(Some("live777".to_string()), None).unwrap());
pub static ENCODER: LazyLock<TextEncoder> = LazyLock::new(TextEncoder::new);
