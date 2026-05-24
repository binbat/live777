//! CaptureBackend — abstract interface for capture devices.
//!
//! Each backend (libcamera, generic V4L2, RDK X5 V4L2) implements this
//! interface to produce RawFrame objects.  The capture layer does NOT
//! create or own an Encoder.
//!
//! Usage:
//!   1. init(cfg, &err)
//!   2. start(frame_callback, &err)
//!   3. … frames arrive via callback …
//!   4. stop()

#pragma once
#include "media_types.h"
#include <functional>
#include <string>

// ---------------------------------------------------------------------------
// Capture configuration
// ---------------------------------------------------------------------------
struct CaptureConfig {
    std::string backend;   // "libcamera" or "v4l2"
    std::string device;    // "/dev/video0" or camera_id string
    uint32_t width;
    uint32_t height;
    uint32_t fps;
    RawPixelFormat pixel_format;
    bool prefer_dmabuf = false;
};

// ---------------------------------------------------------------------------
// Frame callback — invoked for every captured frame.
// The RawFrame reference is valid only during the callback.
// ---------------------------------------------------------------------------
using CaptureFrameCallback = std::function<void(const RawFrame&)>;

// ---------------------------------------------------------------------------
// Abstract capture backend
// ---------------------------------------------------------------------------
class CaptureBackend {
public:
    virtual ~CaptureBackend() = default;

    /// One-time initialisation.  Must be called before start().
    virtual bool init(const CaptureConfig& cfg, std::string* err) = 0;

    /// Begin capture.  Frames are delivered to `cb`.
    virtual bool start(CaptureFrameCallback cb, std::string* err) = 0;

    /// Stop capture and release streaming resources.
    /// Safe to call multiple times.
    virtual void stop() = 0;

    /// Returns true if the backend is currently streaming.
    virtual bool isRunning() const = 0;
};

/// Factory: creates the appropriate CaptureBackend for the given config.
/// Implemented in each platform's capture .cpp file.
std::unique_ptr<CaptureBackend> create_capture_backend(const CaptureConfig& cfg);
