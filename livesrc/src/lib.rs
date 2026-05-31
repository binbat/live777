//! livesrc — native capture / encoder backend crate.
//!
//! Provides C++ native pipeline (libcamera, V4L2, RDK X5) through a safe
//! Rust wrapper.  Outputs [`EncodedPacket`] frames via an mpsc channel.
//!
//! This crate does **not** handle RTP, WHEP, source management, or any
//! real-time transport — those responsibilities belong to `liveion`.

pub mod types;
mod native_ffi; // crate-private
pub mod source_pipeline;

pub use types::{EncodedPacket, NativeSourceParams};
pub use source_pipeline::{KeyframeHandle, NativePipeline};
