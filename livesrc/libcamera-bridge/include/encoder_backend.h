//! EncoderBackend — abstract interface for video encoders.
//!
//! Each backend (V4L2 M2M, RDK X5) implements this interface to consume
//! RawFrame objects from the capture layer and produce EncodedPacket
//! objects (H.264 / H.265 Annex-B bytestream chunks).
//!
//! Usage:
//!   1. init(cfg, &err)
//!   2. setCallback(cb)
//!   3. submit(frame, &err)   — one call per captured frame
//!   4. requestKeyframe()     — force an IDR on the next frame
//!   5. stop()

#pragma once
#include <cstddef>
#include "media_types.h"
#include <functional>
#include <string>

// ---------------------------------------------------------------------------
// Flags for EncodedPacket
// ---------------------------------------------------------------------------
enum EncodedFlags : uint32_t {
    EncodedKeyframe = 1u << 0, // IDR frame
    EncodedConfig = 1u << 1,   // SPS / PPS / VPS
};

// ---------------------------------------------------------------------------
// Encoder configuration
// ---------------------------------------------------------------------------
struct EncoderConfig {
    std::string backend;  // "v4l2_m2m" or "rdk_x5"
    VideoCodec codec;     // H264 or H265
    uint32_t width;
    uint32_t height;
    uint32_t fps;
    uint32_t bitrate;     // bits per second
    std::string profile;  // e.g. "42001f"
    uint32_t gop = 60;
    bool prefer_dmabuf = false;
};

// ---------------------------------------------------------------------------
// Encoded packet — output of the encoder layer.
//
// Lifetime: valid only within the EncodedPacketCallback.  The consumer
// must copy data if it needs it beyond the callback return.
// ---------------------------------------------------------------------------
struct EncodedPacket {
    VideoCodec codec;
    const uint8_t* data; // valid only during callback
    size_t size;
    uint64_t pts_us;
    uint64_t dts_us;
    uint32_t flags; // bitmask of EncodedFlags
};

// ---------------------------------------------------------------------------
// Callback type for encoded output
// ---------------------------------------------------------------------------
using EncodedPacketCallback = std::function<void(const EncodedPacket&)>;

// ---------------------------------------------------------------------------
// Abstract encoder backend
// ---------------------------------------------------------------------------
class EncoderBackend {
public:
    virtual ~EncoderBackend() = default;

    /// One-time initialisation.  Must be called before submit().
    virtual bool init(const EncoderConfig& cfg, std::string* err) = 0;

    /// Submit a raw frame for encoding.
    /// The frame data must be valid for the duration of the call.
    virtual bool submit(const RawFrame& frame, std::string* err) = 0;

    /// Request an IDR keyframe at the next opportunity.
    virtual void requestKeyframe() = 0;

    /// Stop encoding and release hardware resources.
    virtual void stop() = 0;

    /// Returns true if the backend is currently encoding.
    virtual bool isRunning() const = 0;

    /// Register the callback for encoded output packets.
    virtual void setCallback(EncodedPacketCallback cb) = 0;
};

/// Factory: creates the appropriate EncoderBackend for the given config.
/// Implemented in each platform's encoder .cpp file.
std::unique_ptr<EncoderBackend> create_encoder_backend(const EncoderConfig& cfg);
