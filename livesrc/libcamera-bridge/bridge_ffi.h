#ifndef BRIDGE_FFI_H
#define BRIDGE_FFI_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

// Opaque handle for the bridge
typedef struct BridgeContext BridgeContext;

// Callback type for H.264 NAL units
typedef void (*NALCallbackFFI)(const uint8_t* data, size_t size, int is_keyframe, uint64_t timestamp, void* user_data);

// Initialize the camera bridge
// Returns a handle on success, NULL on failure
BridgeContext* bridge_init(
    int width, 
    int height, 
    int fps, 
    int bitrate, 
    int camera_id,
    int rotation,
    int hflip,
    int vflip
);

// Set the NAL callback
void bridge_set_callback(BridgeContext* ctx, NALCallbackFFI callback, void* user_data);

// Start capture and encoding
bool bridge_start(BridgeContext* ctx);

// Stop capture
void bridge_stop(BridgeContext* ctx);

// Force an IDR frame (Instant Keyframe Request)
void bridge_request_keyframe(BridgeContext* ctx);

// Get last error message
const char* bridge_get_error(BridgeContext* ctx);

// Cleanup and free the bridge
void bridge_free(BridgeContext* ctx);

#ifdef __cplusplus
}
#endif

#endif // BRIDGE_FFI_H
