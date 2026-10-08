use prometheus::{Gauge, GaugeVec, IntCounterVec, Opts, Registry, TextEncoder};
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
/// RTCP packets observed on the server's peer connections, labeled by
/// `direction` and `kind`: `to_publisher` = written to a publish peer
/// (counted by the outermost interceptor of the publish chain, so both
/// interceptor-generated and application-written packets are included);
/// `from_subscriber` = received from a subscribe peer (tapped by the
/// subscribe-quality interceptor, compiled only with the `source` feature,
/// so this direction is absent in builds without it). `kind` is the RTCP
/// packet type: `pli`, `fir`, `nack`, `rr`, `sr`, `twcc`, `remb`, or `other`.
pub static RTCP_PACKETS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    IntCounterVec::new(
        Opts::new("rtcp_packets_total", "RTCP packets by direction and kind"),
        &["direction", "kind"],
    )
    .unwrap()
});
/// Per-stream RTP media bytes: the same accounting points as
/// [`RTP_BYTES_TOTAL`] (stats-tick sampling plus final folds) with the
/// stream name as an extra label. Streams are created dynamically
/// (WHIP/WHEP auto-create), so this label is unbounded in high-churn
/// deployments; provisioned-stream deployments are fine.
pub static STREAM_RTP_BYTES_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    IntCounterVec::new(
        Opts::new(
            "stream_rtp_bytes_total",
            "Per-stream RTP media bytes transferred (wire size)",
        ),
        &["stream", "direction"],
    )
    .unwrap()
});
/// Per-stream session counts, labeled by `kind` (`publish` | `subscribe`).
/// Set from the manager's 2 s stats tick from the forward's live session
/// state rather than incremented/decremented per event, so the values
/// cannot drift. A stream's series are removed from the vec at teardown
/// (`emit_stream_deleted`) — the prometheus client never drops children on
/// its own, so without the removal deleted streams would bloat /metrics
/// forever instead of going stale.
pub static STREAM_SESSIONS: LazyLock<GaugeVec> = LazyLock::new(|| {
    GaugeVec::new(
        Opts::new(
            "stream_sessions",
            "Per-stream publish/subscribe session counts",
        ),
        &["stream", "kind"],
    )
    .unwrap()
});
/// DataChannel messages transferred, labeled by `direction`: `in` = received
/// from a client (publish or subscribe peer), `out` = written to a client.
/// Counted in the per-channel read/write loops, so `out` reflects the bus
/// fan-out (one delivery per attached peer).
pub static DATACHANNEL_MESSAGES_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    IntCounterVec::new(
        Opts::new(
            "datachannel_messages_total",
            "DataChannel messages by direction",
        ),
        &["direction"],
    )
    .unwrap()
});
/// DataChannel payload bytes transferred (message body, no SCTP/DTLS
/// framing), same accounting points as [`DATACHANNEL_MESSAGES_TOTAL`].
pub static DATACHANNEL_BYTES_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    IntCounterVec::new(
        Opts::new(
            "datachannel_bytes_total",
            "DataChannel payload bytes by direction",
        ),
        &["direction"],
    )
    .unwrap()
});
/// DataChannel messages dropped because a per-channel write loop lagged
/// behind its stream's broadcast bus (the ring capacity is bounded). Only
/// the `out` direction can drop this way.
pub static DATACHANNEL_DROPPED_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    IntCounterVec::new(
        Opts::new(
            "datachannel_dropped_total",
            "DataChannel messages dropped on broadcast-bus lag",
        ),
        &["direction"],
    )
    .unwrap()
});
/// Per-stream DataChannel payload bytes: the same accounting points as
/// [`DATACHANNEL_BYTES_TOTAL`] with the stream name as an extra label,
/// sharing [`STREAM_RTP_BYTES_TOTAL`]'s cardinality caveats and teardown
/// removal (`emit_stream_deleted`).
pub static STREAM_DATACHANNEL_BYTES_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    IntCounterVec::new(
        Opts::new(
            "stream_datachannel_bytes_total",
            "Per-stream DataChannel payload bytes",
        ),
        &["stream", "direction"],
    )
    .unwrap()
});
pub static REGISTRY: LazyLock<Registry> =
    LazyLock::new(|| Registry::new_custom(Some("live777".to_string()), None).unwrap());
pub static ENCODER: LazyLock<TextEncoder> = LazyLock::new(TextEncoder::new);
