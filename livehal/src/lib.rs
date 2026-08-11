//! livehal — Hardware Abstraction Layer / native capture + encoder backend crate.
//!
//! Provides C++ native pipeline (libcamera, V4L2, RDK X5) through a safe
//! Rust wrapper.  Outputs [`EncodedPacket`] frames via an mpsc channel.
//!
//! This crate does **not** handle RTP, WHEP, source management, or any
//! real-time transport — those responsibilities belong to `liveion`.

mod native_ffi; // crate-private
pub mod source_pipeline;
pub mod types;

pub use source_pipeline::{BitrateHandle, KeyframeHandle, NativePipeline};
pub use types::{EncodedPacket, NativeSourceParams};

/// Monotonic clock (`CLOCK_MONOTONIC`) in microseconds — the same epoch the
/// C++ pipeline uses for frame timestamps (V4L2 buffer SOF / steady_clock),
/// so downstream stages can compute a frame's age as
/// `monotonic_us() - pkt.pts_us`.
///
/// The epoch contract only matters on Linux, where the native pipeline runs;
/// other platforms get a plain monotonic microsecond clock so the crate still
/// compiles and downstream deltas stay consistent.
#[cfg(unix)]
pub fn monotonic_us() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // clock_gettime(CLOCK_MONOTONIC) cannot fail.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1_000
}

/// Portable fallback for non-unix platforms (no `libc::clock_gettime` there):
/// an arbitrary monotonic epoch is fine — only deltas are used.
#[cfg(not(unix))]
pub fn monotonic_us() -> u64 {
    use std::sync::LazyLock;
    use std::time::Instant;

    static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
    EPOCH.elapsed().as_micros() as u64
}
