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
// RawFrame — output of the capture layer
//
// Lifetime: valid only within the CaptureFrameCallback.  The consumer
// (encoder) must copy data if it needs it beyond the callback return.
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
};
