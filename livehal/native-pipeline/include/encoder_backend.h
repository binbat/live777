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
#include <memory>
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
    std::string backend;  // "v4l2-m2m", "rdk", or "rkmpp"
    VideoCodec codec;     // H264 or H265
    RawPixelFormat input_format = RawPixelFormat::Yuv420p;
    uint32_t width;
    uint32_t height;
    uint32_t fps;
    uint32_t bitrate;     // bits per second
    std::string profile;  // e.g. "42001f"
    uint32_t profile_idc = 0;  // 66=baseline,77=main,100=high (H.264); 1=main (H.265)
    uint32_t level_idc = 0;    // 40=H.264 4.0, 120=H.265 4.0
    uint32_t tier_flag = 0;    // 0=main tier (H.265 only)
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
    /// The frame struct and CPU plane pointers must be valid for the
    /// duration of the call.  If the frame carries a release callback
    /// (DmaBuf zero-copy), the implementation MUST invoke it exactly once:
    /// immediately on any path that does not retain the buffer (including
    /// every error return), or later once the hardware finished reading
    /// it.  stop() must release all still-held frames.
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

/// Platform-specific encoder factories — defined in backend .cpp files.
/// Callers should use create_encoder_backend() (the dispatcher in backend_factory.cpp).
std::shared_ptr<EncoderBackend> create_v4l2_m2m_encoder_backend(const EncoderConfig& cfg);
std::shared_ptr<EncoderBackend> create_rdk_x5_encoder_backend(const EncoderConfig& cfg);
std::shared_ptr<EncoderBackend> create_rkmpp_encoder_backend(const EncoderConfig& cfg);

/// Dispatcher: selects the right backend based on cfg.backend.
/// Defined exactly once in src/pipeline/backend_factory.cpp.
std::shared_ptr<EncoderBackend> create_encoder_backend(const EncoderConfig& cfg);
