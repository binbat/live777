//! Internal C++ types for capture and encoder layers.
//!
//! These types are C++ internal — they do NOT cross the Rust FFI boundary.
//! Rust only receives `EncodedPacket` via a pure-C FFI callback (later PR).

#pragma once
#include <cstdint>

// ---------------------------------------------------------------------------
// Raw pixel formats (input side — for RawFrame)
// ---------------------------------------------------------------------------
enum class RawPixelFormat : uint32_t {
    Yuyv422 = 0,
    Nv12 = 1,
    Yuv420p = 2,
    Mjpeg = 3,
    Rgb888 = 4,
};

// ---------------------------------------------------------------------------
// Encoded video codecs (output side — for EncodedPacket, PR3)
// ---------------------------------------------------------------------------
enum class VideoCodec : uint32_t {
    H264 = 100,
    H265 = 101,
    Av1 = 102,
    Vp8 = 103,
    Vp9 = 104,
};

// ---------------------------------------------------------------------------
// Buffer kind
// ---------------------------------------------------------------------------
enum class BufferKind : uint32_t {
    Cpu = 0,
    DmaBuf = 1,
};

// ---------------------------------------------------------------------------
// Plane view — a single plane within a RawFrame
// ---------------------------------------------------------------------------
struct PlaneView {
    const uint8_t* data; // valid only within the capture callback (CPU path)
    uint32_t stride;
    uint32_t bytes;
    int dma_fd; // -1 for CPU path; DMA fd lifecycle managed internally by C++
    uint32_t offset;
};

// ---------------------------------------------------------------------------
// Capture buffer release callback (zero-copy lifetime contract)
//
// Called by the frame consumer when the capture buffer identified by
// `buffer_index` is no longer being read.  Must be exactly-once per frame
// that carried a non-null `release`.  May be invoked from a different
// thread than the capture callback, so implementations must be thread-safe
// and must tolerate calls after capture stop (as no-ops).
// ---------------------------------------------------------------------------
using CaptureBufferReleaseFn = void (*)(void* ctx, uint32_t buffer_index);

// ---------------------------------------------------------------------------
// RawFrame — output of the capture layer
//
// Lifetime: the struct (and CPU plane pointers) is valid only within the
// CaptureFrameCallback.  The consumer (encoder) must copy data it needs
// beyond the callback return — EXCEPT for the underlying capture buffer of
// a DmaBuf frame, whose reuse is governed by the release contract below.
//
// Zero-copy lifetime contract (deferred requeue):
//   - `release == nullptr`: classic behaviour.  The capture requeues the
//     V4L2 buffer as soon as the callback returns; the consumer must not
//     touch the buffer afterwards.
//   - `release != nullptr` (DmaBuf frames only): the capture does NOT
//     requeue the buffer.  Ownership transfers to the consumer, which MUST
//     call `release(release_ctx, buffer_index)` exactly once — either when
//     the hardware is done reading the buffer, or immediately on any path
//     where it does not take ownership (validation failure, copy fallback).
// ---------------------------------------------------------------------------
struct RawFrame {
    BufferKind kind;
    RawPixelFormat format;
    uint32_t width;
    uint32_t height;
    uint64_t pts_us;
    uint64_t seq;
    uint32_t plane_count; // 1–3
    PlaneView planes[3]; // only indices [0..plane_count) are valid

    // Zero-copy release contract; all zero/null when not applicable.
    uint32_t buffer_index = 0;
    CaptureBufferReleaseFn release = nullptr;
    void* release_ctx = nullptr;
};

// ---------------------------------------------------------------------------
// FrameReleaseGuard — RAII helper honouring the release contract.
//
// Releases an armed capture buffer on destruction unless disarmed.  Every
// encoder backend constructs one at submit() entry so that EVERY exit path
// (validation failure, unsupported kind, put failure) returns the buffer to
// the capture exactly once; a backend that retains the buffer asynchronously
// (rkmpp zero-copy) disarms the guard once ownership moved to its in-flight
// queue.  Backends that never retain (v4l2-m2m, rdk) simply never disarm.
// ---------------------------------------------------------------------------
struct FrameReleaseGuard {
    const RawFrame& frame;
    bool armed;
    explicit FrameReleaseGuard(const RawFrame& f)
        : frame(f), armed(f.release != nullptr) {}
    ~FrameReleaseGuard() {
        if (armed) frame.release(frame.release_ctx, frame.buffer_index);
    }
    FrameReleaseGuard(const FrameReleaseGuard&) = delete;
    FrameReleaseGuard& operator=(const FrameReleaseGuard&) = delete;
    void disarm() { armed = false; }
};
