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

pub use source_pipeline::{KeyframeHandle, NativePipeline};
pub use types::{EncodedPacket, NativeSourceParams};

/// Monotonic clock (`CLOCK_MONOTONIC`) in microseconds — the same epoch the
/// C++ pipeline uses for frame timestamps (V4L2 buffer SOF / steady_clock),
/// so downstream stages can compute a frame's age as
/// `monotonic_us() - pkt.pts_us`.
pub fn monotonic_us() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // clock_gettime(CLOCK_MONOTONIC) cannot fail.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1_000
}
