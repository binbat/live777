//! Single dispatcher for capture and encoder factories.
//!
//! Each backend .cpp file exports a uniquely-named factory.  This file
//! provides the single generic entry points (create_capture_backend /
//! create_encoder_backend) that dispatch on cfg.backend at runtime.
//!
//! Factory references are conditionally compiled via CMake-generated
//! ENABLE_* macros so that Pi builds do not try to link RDK X5 symbols
//! and vice versa.

#include "include/capture_backend.h"
#include "include/encoder_backend.h"

// ---------------------------------------------------------------------------
// Forward declarations (one per backend — defined in the backend .cpp file)
// ---------------------------------------------------------------------------

#if ENABLE_CAPTURE_LIBCAMERA
std::unique_ptr<CaptureBackend> create_libcamera_capture_backend(const CaptureConfig& cfg);
#endif

// V4L2 capture: RDK and generic are mutually exclusive at compile time.
// Only declare the factory that actually exists in the current build.
#if ENABLE_BACKEND_RDK_X5
std::unique_ptr<CaptureBackend> create_rdk_v4l2_capture_backend(const CaptureConfig& cfg);
#elif ENABLE_CAPTURE_V4L2
std::unique_ptr<CaptureBackend> create_v4l2_capture_backend(const CaptureConfig& cfg);
#endif

#if ENABLE_ENCODER_V4L2_M2M
std::unique_ptr<EncoderBackend> create_v4l2_m2m_encoder_backend(const EncoderConfig& cfg);
#endif

#if ENABLE_ENCODER_RDK_X5
std::unique_ptr<EncoderBackend> create_rdk_x5_encoder_backend(const EncoderConfig& cfg);
#endif

// ---------------------------------------------------------------------------
// Dispatchers
// ---------------------------------------------------------------------------

std::unique_ptr<CaptureBackend> create_capture_backend(const CaptureConfig& cfg) {
#if ENABLE_CAPTURE_LIBCAMERA
    if (cfg.backend == "libcamera") return create_libcamera_capture_backend(cfg);
#endif
#if ENABLE_BACKEND_RDK_X5
    if (cfg.backend == "v4l2") return create_rdk_v4l2_capture_backend(cfg);
#elif ENABLE_CAPTURE_V4L2
    if (cfg.backend == "v4l2") return create_v4l2_capture_backend(cfg);
#endif
    return nullptr;
}

std::unique_ptr<EncoderBackend> create_encoder_backend(const EncoderConfig& cfg) {
#if ENABLE_ENCODER_V4L2_M2M
    if (cfg.backend == "v4l2-m2m") return create_v4l2_m2m_encoder_backend(cfg);
#endif
#if ENABLE_ENCODER_RDK_X5
    if (cfg.backend == "rdk") return create_rdk_x5_encoder_backend(cfg);
#endif
    return nullptr;
}
