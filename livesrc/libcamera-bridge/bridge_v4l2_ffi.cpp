#include "v4l2_capture.h"
#include "encoder.h"
#include <cstdio>
#include <cstring>

extern "C" {

struct V4L2BridgeContext {
    uint32_t magic = 0xBEEFCAFE;
    V4L2CaptureHandle capture;
    Encoder encoder;
    void (*on_frame)(const uint8_t* data, size_t size, int is_keyframe, uint64_t timestamp, void* user_data);
    void* user_data;
};

// Stable channel: V4L2 Capture → Encoder
static void on_v4l2_frame(const uint8_t* data, size_t size, uint64_t timestamp, void* user_data) {
    if (!user_data) return;
    auto* ctx = static_cast<V4L2BridgeContext*>(user_data);
    if (ctx->magic != 0xBEEFCAFE) return;
    ctx->encoder.encode(data, size, timestamp);
}

V4L2BridgeContext* v4l2_bridge_init(const char* device, int width, int height, int fps, int bitrate) {
    auto* ctx = new V4L2BridgeContext();

    // Initialize the encoder (reuses the existing V4L2 M2M encoder)
    CameraParams enc_params;
    enc_params.width = width;
    enc_params.height = height;
    enc_params.fps = fps;
    enc_params.bitrate = bitrate;
    enc_params.camera_id = 0;
    enc_params.rotation = 0;
    enc_params.hflip = false;
    enc_params.vflip = false;

    if (!ctx->encoder.init(enc_params)) {
        fprintf(stderr, "[V4L2Bridge] Encoder init failed\n");
        delete ctx;
        return nullptr;
    }

    // Initialize V4L2 capture
    ctx->capture = v4l2cap_create();
    V4L2CaptureParams cap_params;
    cap_params.device = device;
    cap_params.width = width;
    cap_params.height = height;
    cap_params.fps = fps;
    cap_params.input_format = 0; // YUYV

    if (!v4l2cap_init(ctx->capture, &cap_params)) {
        fprintf(stderr, "[V4L2Bridge] Capture init failed: %s\n", v4l2cap_get_error(ctx->capture));
        v4l2cap_destroy(ctx->capture);
        delete ctx;
        return nullptr;
    }

    // Wire: Capture → Encoder
    v4l2cap_set_callback(ctx->capture, on_v4l2_frame, ctx);

    fprintf(stderr, "[V4L2Bridge] Init OK: %s %dx%d@%dfps\n", device, width, height, fps);
    return ctx;
}

void v4l2_bridge_set_callback(V4L2BridgeContext* ctx,
    void (*callback)(const uint8_t*, size_t, int, uint64_t, void*), void* user_data) {
    if (!ctx || ctx->magic != 0xBEEFCAFE) return;
    ctx->on_frame = callback;
    ctx->user_data = user_data;

    ctx->encoder.setNALCallback([](const uint8_t* d, size_t s, int k, uint64_t ts, void* ud) {
        auto* c = static_cast<V4L2BridgeContext*>(ud);
        if (c->on_frame) {
            c->on_frame(d, s, k, ts, c->user_data);
        }
    }, ctx);
}

bool v4l2_bridge_start(V4L2BridgeContext* ctx) {
    if (!ctx || ctx->magic != 0xBEEFCAFE) return false;
    return v4l2cap_start(ctx->capture);
}

void v4l2_bridge_stop(V4L2BridgeContext* ctx) {
    if (!ctx) return;
    v4l2cap_stop(ctx->capture);
}

bool v4l2_bridge_is_running(V4L2BridgeContext* ctx) {
    if (!ctx) return false;
    return v4l2cap_is_running(ctx->capture);
}

void v4l2_bridge_request_keyframe(V4L2BridgeContext* ctx) {
    if (!ctx || ctx->magic != 0xBEEFCAFE) return;
    ctx->encoder.requestKeyframe();
}

const char* v4l2_bridge_get_error(V4L2BridgeContext* ctx) {
    if (!ctx || ctx->magic != 0xBEEFCAFE) return "Invalid Context";
    return v4l2cap_get_error(ctx->capture);
}

void v4l2_bridge_free(V4L2BridgeContext* ctx) {
    if (!ctx || ctx->magic != 0xBEEFCAFE) return;
    v4l2cap_destroy(ctx->capture);
    delete ctx;
}

} // extern "C"
