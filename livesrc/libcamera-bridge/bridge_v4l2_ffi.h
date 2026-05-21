#ifndef BRIDGE_V4L2_FFI_H
#define BRIDGE_V4L2_FFI_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque handle for the V4L2 bridge
typedef struct V4L2BridgeContext V4L2BridgeContext;

// Callback type: same signature as the libcamera bridge for Rust compatibility
typedef void (*V4L2NALCallbackFFI)(const uint8_t* data, size_t size, int is_keyframe, uint64_t timestamp, void* user_data);

// Initialize the V4L2 bridge (USB camera + hardware encoder)
// device: V4L2 device path, e.g. "/dev/video2"
V4L2BridgeContext* v4l2_bridge_init(
    const char* device,
    int width,
    int height,
    int fps,
    int bitrate
);

void v4l2_bridge_set_callback(V4L2BridgeContext* ctx, V4L2NALCallbackFFI callback, void* user_data);
bool v4l2_bridge_start(V4L2BridgeContext* ctx);
void v4l2_bridge_stop(V4L2BridgeContext* ctx);
bool v4l2_bridge_is_running(V4L2BridgeContext* ctx);
void v4l2_bridge_request_keyframe(V4L2BridgeContext* ctx);
const char* v4l2_bridge_get_error(V4L2BridgeContext* ctx);
void v4l2_bridge_free(V4L2BridgeContext* ctx);

#ifdef __cplusplus
}
#endif

#endif // BRIDGE_V4L2_FFI_H
