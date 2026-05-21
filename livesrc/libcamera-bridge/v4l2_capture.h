#ifndef V4L2_CAPTURE_H
#define V4L2_CAPTURE_H

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>

struct V4L2CaptureParams {
    const char* device;     // e.g. "/dev/video2"
    int width;
    int height;
    int fps;
    int input_format;       // 0 = YUYV (auto-convert to YUV420P), 1 = MJPEG
};

// Callback: delivers YUV420P frames ready for the encoder (CPU path)
typedef void (*V4L2FrameCallback)(const uint8_t* data, size_t size, uint64_t timestamp_us, void* user_data);

// Callback: delivers DMA-BUF file descriptor (Zero-copy path for RDK)
typedef void (*V4L2FDFrameCallback)(int dma_fd, size_t size, uint64_t timestamp_us, void* user_data);

typedef void* V4L2CaptureHandle;

#ifdef __cplusplus
extern "C" {
#endif

V4L2CaptureHandle v4l2cap_create();
void v4l2cap_destroy(V4L2CaptureHandle handle);
bool v4l2cap_init(V4L2CaptureHandle handle, const V4L2CaptureParams* params);
bool v4l2cap_start(V4L2CaptureHandle handle);
void v4l2cap_stop(V4L2CaptureHandle handle);
void v4l2cap_set_callback(V4L2CaptureHandle handle, V4L2FrameCallback callback, void* user_data);
void v4l2cap_set_fd_callback(V4L2CaptureHandle handle, V4L2FDFrameCallback callback, void* user_data);
bool v4l2cap_is_running(V4L2CaptureHandle handle);
const char* v4l2cap_get_error(V4L2CaptureHandle handle);

#ifdef __cplusplus
}
#endif

#endif // V4L2_CAPTURE_H
