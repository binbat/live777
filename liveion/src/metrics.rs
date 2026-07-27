use prometheus::{Gauge, IntCounterVec, Opts, Registry, TextEncoder};
use std::sync::LazyLock;

pub static STREAM: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("stream", "stream number").unwrap());
pub static PUBLISH: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("publish", "publish number").unwrap());
pub static SUBSCRIBE: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("subscribe", "subscribe number").unwrap());
pub static REFORWARD: LazyLock<Gauge> =
    LazyLock::new(|| Gauge::new("reforward", "reforward number").unwrap());
/// Server-wide RTP media bytes transferred (cumulative, wire size), labeled
/// by `direction`: `in` = received from publishers, `out` = sent to
/// subscribers.
pub static RTP_BYTES_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    IntCounterVec::new(
        Opts::new("rtp_bytes_total", "RTP media bytes transferred (wire size)"),
        &["direction"],
    )
    .unwrap()
});
pub static REGISTRY: LazyLock<Registry> =
    LazyLock::new(|| Registry::new_custom(Some("live777".to_string()), None).unwrap());
pub static ENCODER: LazyLock<TextEncoder> = LazyLock::new(TextEncoder::new);
