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
#include <memory>
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

/// Platform-specific capture factories — defined in backend .cpp files.
/// Callers should use create_capture_backend() (the dispatcher in backend_factory.cpp).
std::unique_ptr<CaptureBackend> create_libcamera_capture_backend(const CaptureConfig& cfg);
std::unique_ptr<CaptureBackend> create_v4l2_capture_backend(const CaptureConfig& cfg);
std::unique_ptr<CaptureBackend> create_rdk_v4l2_capture_backend(const CaptureConfig& cfg);

/// Dispatcher: selects the right backend based on cfg.backend.
/// Defined exactly once in src/pipeline/backend_factory.cpp.
std::unique_ptr<CaptureBackend> create_capture_backend(const CaptureConfig& cfg);
