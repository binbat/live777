//! Media statistics counters shared by the forward hot paths and the
//! manager's periodic bitrate sampler (issue #252: stream in/out rates and
//! cumulative totals).

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Time base for sampler timestamps (milliseconds since process start —
/// only deltas between samples matter).
static TIME_BASE: LazyLock<Instant> = LazyLock::new(Instant::now);

fn now_ms() -> u64 {
    TIME_BASE.elapsed().as_millis() as u64
}

/// Lock-free media counters for one packet flow: a publish track, a
/// subscribe session, or a whole stream.
///
/// RTP hot paths only call [`MediaStats::inc`]; the manager's stats tick
/// calls [`MediaStats::sample`] to turn byte deltas into a bitrate. Stream
/// totals additionally use [`MediaStats::add_delta`] so they stay monotonic
/// when tracks/sessions come and go.
pub(crate) struct MediaStats {
    /// Cumulative bytes (RTP wire size: header + extensions + payload).
    bytes: AtomicU64,
    /// Cumulative packets.
    packets: AtomicU64,
    /// Bits per second over the last sampling interval.
    bitrate_bps: AtomicU64,
    /// Sampler scratch: cumulative counters at the previous sample.
    sampled_bytes: AtomicU64,
    sampled_packets: AtomicU64,
    /// Sampler scratch: timestamp of the previous sample (ms since
    /// `TIME_BASE`; 0 = never sampled).
    sampled_at_ms: AtomicU64,
}

impl MediaStats {
    pub(crate) fn new() -> Self {
        Self {
            bytes: AtomicU64::new(0),
            packets: AtomicU64::new(0),
            bitrate_bps: AtomicU64::new(0),
            sampled_bytes: AtomicU64::new(0),
            sampled_packets: AtomicU64::new(0),
            sampled_at_ms: AtomicU64::new(0),
        }
    }

    /// Count one forwarded packet of `bytes` bytes. Called on the RTP hot
    /// path — keep it to plain atomic adds.
    pub(crate) fn inc(&self, bytes: u64) {
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.packets.fetch_add(1, Ordering::Relaxed);
    }

    /// Compute the bitrate since the previous sample and return the
    /// `(bytes, packets)` delta, so the caller can accumulate stream/server
    /// totals that stay monotonic even when this flow goes away.
    pub(crate) fn sample(&self) -> (u64, u64) {
        self.sample_at(now_ms())
    }

    fn sample_at(&self, now: u64) -> (u64, u64) {
        let bytes = self.bytes.load(Ordering::Relaxed);
        let packets = self.packets.load(Ordering::Relaxed);
        let last_bytes = self.sampled_bytes.swap(bytes, Ordering::Relaxed);
        let last_packets = self.sampled_packets.swap(packets, Ordering::Relaxed);
        let last_at = self.sampled_at_ms.swap(now, Ordering::Relaxed);
        let bytes_delta = bytes.saturating_sub(last_bytes);
        let packets_delta = packets.saturating_sub(last_packets);
        let dt_ms = now.saturating_sub(last_at);
        let bitrate = if last_at == 0 || dt_ms == 0 {
            0
        } else {
            (bytes_delta as u128 * 8000 / dt_ms as u128) as u64
        };
        self.bitrate_bps.store(bitrate, Ordering::Relaxed);
        (bytes_delta, packets_delta)
    }

    /// Accumulate another flow's sample delta into this counter (stream and
    /// server totals). Bitrate is maintained separately via
    /// [`MediaStats::set_bitrate`].
    pub(crate) fn add_delta(&self, bytes: u64, packets: u64) {
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.packets.fetch_add(packets, Ordering::Relaxed);
    }

    pub(crate) fn set_bitrate(&self, bitrate: u64) {
        self.bitrate_bps.store(bitrate, Ordering::Relaxed);
    }

    pub(crate) fn bitrate(&self) -> u64 {
        self.bitrate_bps.load(Ordering::Relaxed)
    }

    pub(crate) fn snapshot(&self) -> api::response::Stats {
        api::response::Stats {
            bytes: self.bytes.load(Ordering::Relaxed),
            packets: self.packets.load(Ordering::Relaxed),
            bitrate: self.bitrate_bps.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_computes_bitrate_and_deltas() {
        let stats = MediaStats::new();
        // First sample only establishes the baseline.
        assert_eq!(stats.sample_at(1000), (0, 0));
        assert_eq!(stats.snapshot().bitrate, 0);

        stats.inc(1000);
        stats.inc(500);
        assert_eq!(stats.sample_at(2000), (1500, 2));
        let snap = stats.snapshot();
        assert_eq!(snap.bytes, 1500);
        assert_eq!(snap.packets, 2);
        assert_eq!(snap.bitrate, 1500 * 8000 / 1000);

        // No traffic: rate decays to zero, cumulative counters stay.
        assert_eq!(stats.sample_at(4000), (0, 0));
        let snap = stats.snapshot();
        assert_eq!(snap.bytes, 1500);
        assert_eq!(snap.bitrate, 0);
    }

    #[test]
    fn add_delta_accumulates_totals() {
        let total = MediaStats::new();
        total.add_delta(100, 1);
        total.add_delta(50, 2);
        total.set_bitrate(1200);
        let snap = total.snapshot();
        assert_eq!(snap.bytes, 150);
        assert_eq!(snap.packets, 3);
        assert_eq!(snap.bitrate, 1200);
    }
}
