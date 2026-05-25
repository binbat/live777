//! Pure C ABI for SourcePipeline.
//!
//! This header defines the FFI boundary between C++ and Rust.
//! All structs use ONLY C ABI-safe types:
//!   const char*, uint32_t, uint8_t, enum values.
//! No std::string, std::function, unique_ptr, or C++ classes are exposed.
//!
//! Data flow (C++ internal):
//!   CaptureBackend → RawFrame → EncoderBackend → EncodedPacket
//!
//! Only EncodedPacket crosses the FFI boundary to Rust.
//! RawFrame is C++ internal and does NOT cross this boundary.

#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// ---------------------------------------------------------------------------
// Pure C FFI config structs (no std::string, no C++ types)
// ---------------------------------------------------------------------------

typedef struct {
    const char* backend;     // "libcamera" or "v4l2"
    const char* device;      // "/dev/video0" or camera_id
    uint32_t width;
    uint32_t height;
    uint32_t fps;
    uint32_t pixel_format;   // RawPixelFormat enum value
    uint8_t prefer_dmabuf;   // 0 = false, 1 = true
} CaptureConfigFFI;

typedef struct {
    const char* backend;     // "v4l2_m2m" or "rdk_x5"
    uint32_t codec;          // VideoCodec enum value
    uint32_t width;
    uint32_t height;
    uint32_t fps;
    uint32_t bitrate;
    const char* profile;     // "42001f"
    uint32_t gop;
    uint8_t prefer_dmabuf;   // 0 = false, 1 = true
} EncoderConfigFFI;

typedef struct {
    CaptureConfigFFI capture;
    EncoderConfigFFI encoder;
    uint32_t payload_type;
    uint32_t clock_rate;
} SourcePipelineConfigFFI;

// ---------------------------------------------------------------------------
// EncodedPacket FFI — the only data crossing to Rust
//
// Lifetime: data pointer is valid ONLY during the on_packet callback.
// Rust must copy the data before the callback returns.
// ---------------------------------------------------------------------------

typedef struct {
    uint32_t codec;          // VideoCodec enum value
    const uint8_t* data;     // valid only during callback
    size_t size;
    uint64_t pts_us;
    uint64_t dts_us;
    uint32_t flags;          // EncodedFlags bitmask
} EncodedPacketFFI;

typedef void (*EncodedPacketCallbackFFI)(const EncodedPacketFFI* packet,
                                         void* user_data);

typedef struct {
    EncodedPacketCallbackFFI on_packet;
    void* user_data;
} SourcePipelineHooksFFI;

// ---------------------------------------------------------------------------
// Opaque handle
// ---------------------------------------------------------------------------

typedef struct SourcePipelineHandle SourcePipelineHandle;

// ---------------------------------------------------------------------------
// C API
// ---------------------------------------------------------------------------

SourcePipelineHandle* source_pipeline_create(
    const SourcePipelineConfigFFI* cfg,
    const SourcePipelineHooksFFI* hooks,
    char* errbuf,
    size_t errbuf_len);

bool source_pipeline_start(SourcePipelineHandle* h);
void source_pipeline_stop(SourcePipelineHandle* h);
bool source_pipeline_is_running(SourcePipelineHandle* h);
void source_pipeline_request_keyframe(SourcePipelineHandle* h);
void source_pipeline_free(SourcePipelineHandle* h);

#ifdef __cplusplus
}
#endif
