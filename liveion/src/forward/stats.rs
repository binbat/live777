//! Media statistics counters shared by the forward hot paths and the
//! manager's periodic bitrate sampler (issue #252: stream in/out rates and
//! cumulative totals).
//!
//! Counting convention: bytes are the RTP wire size (header + extensions +
//! payload) of packets the forwarding loops actually handled. Outbound
//! counters therefore exclude NACK retransmissions, which the webrtc stack
//! serves internally below the forwarding loop.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Time base for sampler timestamps (milliseconds since process start —
/// only deltas between samples matter).
static TIME_BASE: LazyLock<Instant> = LazyLock::new(Instant::now);

fn now_ms() -> u64 {
    TIME_BASE.elapsed().as_millis() as u64
}

/// One sampling result: the counter deltas since the previous sample and
/// the bitrate derived from them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sample {
    pub bytes: u64,
    pub packets: u64,
    /// Bits per second over the sampling interval; 0 when the interval
    /// collapsed (two samples within the same millisecond).
    pub bitrate: u64,
}

/// Byte deltas sampled from one stream's counters on a stats tick, used
/// for the server-wide Prometheus totals.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ByteDeltas {
    /// Bytes received from the publisher side.
    pub inbound: u64,
    /// Bytes sent to all subscriber sides.
    pub outbound: u64,
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
    /// `TIME_BASE`).
    sampled_at_ms: AtomicU64,
}

impl MediaStats {
    pub(crate) fn new() -> Self {
        Self::new_at(now_ms())
    }

    fn new_at(start_ms: u64) -> Self {
        Self {
            bytes: AtomicU64::new(0),
            packets: AtomicU64::new(0),
            bitrate_bps: AtomicU64::new(0),
            sampled_bytes: AtomicU64::new(0),
            sampled_packets: AtomicU64::new(0),
            // The baseline starts at creation, so the first sample already
            // yields an average rate instead of a meaningless zero.
            sampled_at_ms: AtomicU64::new(start_ms),
        }
    }

    /// Count one forwarded packet of `bytes` bytes. Called on the RTP hot
    /// path — keep it to plain atomic adds.
    pub(crate) fn inc(&self, bytes: u64) {
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.packets.fetch_add(1, Ordering::Relaxed);
    }

    /// Compute the bitrate since the previous sample and return the
    /// counter deltas, so the caller can accumulate stream/server totals
    /// that stay monotonic even when this flow goes away.
    pub(crate) fn sample(&self) -> Sample {
        self.sample_at(now_ms())
    }

    fn sample_at(&self, now: u64) -> Sample {
        let bytes = self.bytes.load(Ordering::Relaxed);
        let packets = self.packets.load(Ordering::Relaxed);
        let last_bytes = self.sampled_bytes.swap(bytes, Ordering::Relaxed);
        let last_packets = self.sampled_packets.swap(packets, Ordering::Relaxed);
        let last_at = self.sampled_at_ms.swap(now, Ordering::Relaxed);
        let bytes_delta = bytes.saturating_sub(last_bytes);
        let packets_delta = packets.saturating_sub(last_packets);
        let dt_ms = now.saturating_sub(last_at);
        let bitrate = if dt_ms == 0 {
            0
        } else {
            (bytes_delta as u128 * 8000 / dt_ms as u128) as u64
        };
        self.bitrate_bps.store(bitrate, Ordering::Relaxed);
        Sample {
            bytes: bytes_delta,
            packets: packets_delta,
            bitrate,
        }
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
        let stats = MediaStats::new_at(1000);
        // Nothing flowed yet: zero deltas and zero rate, but the baseline
        // already moved to the first sample.
        assert_eq!(stats.sample_at(2000), Sample::default());

        stats.inc(1000);
        stats.inc(500);
        assert_eq!(
            stats.sample_at(3000),
            Sample {
                bytes: 1500,
                packets: 2,
                bitrate: 1500 * 8000 / 1000,
            }
        );
        let snap = stats.snapshot();
        assert_eq!(snap.bytes, 1500);
        assert_eq!(snap.packets, 2);
        assert_eq!(snap.bitrate, 1500 * 8000 / 1000);

        // No traffic: rate decays to zero, cumulative counters stay.
        assert_eq!(stats.sample_at(5000), Sample::default());
        let snap = stats.snapshot();
        assert_eq!(snap.bytes, 1500);
        assert_eq!(snap.bitrate, 0);
    }

    #[test]
    fn first_sample_yields_creation_average() {
        // The baseline starts at creation, so traffic before the first
        // sample still produces a rate.
        let stats = MediaStats::new_at(1000);
        stats.inc(1600);
        let sample = stats.sample_at(3000);
        assert_eq!(sample.bytes, 1600);
        assert_eq!(sample.packets, 1);
        assert_eq!(sample.bitrate, 1600 * 8000 / 2000);
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
