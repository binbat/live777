#include "camera.h"
#include "encoder.h"
#include <memory>
#include <string>
#include <vector>
#include <cstdio>

extern "C" {

struct BridgeContext {
    uint32_t magic = 0xDEADBEEF;
    CameraHandle camera;
    Encoder encoder;
    void (*on_frame)(const uint8_t* data, size_t size, int is_keyframe, uint64_t timestamp, void* user_data);
    void* user_data;
};

// THE STABLE CHANNEL: Data from Camera -> Encoder
static void on_camera_frame_stable(const uint8_t* data, size_t size, uint64_t timestamp, void* user_data) {
    if (!user_data) return;
    auto ctx = static_cast<BridgeContext*>(user_data);
    if (ctx->magic != 0xDEADBEEF) return;
    ctx->encoder.encode(data, size, timestamp);
}

BridgeContext* bridge_init(int width, int height, int fps, int bitrate, int camera_id, int rotation, int hflip, int vflip) {
    auto ctx = new BridgeContext();
    
    CameraParams params;
    params.width = width;
    params.height = height;
    params.fps = fps;
    params.bitrate = bitrate;
    params.camera_id = camera_id;
    params.rotation = rotation;
    params.hflip = hflip != 0;
    params.vflip = vflip != 0;

    if (!ctx->encoder.init(params)) {
        delete ctx;
        return nullptr;
    }

    ctx->camera = camera_create();
    if (!camera_init(ctx->camera, &params)) {
        camera_destroy(ctx->camera);
        delete ctx;
        return nullptr;
    }

    camera_set_callback(ctx->camera, on_camera_frame_stable, ctx);
    return ctx;
}

void bridge_set_callback(BridgeContext* ctx, void (*callback)(const uint8_t*, size_t, int, uint64_t, void*), void* user_data) {
    if (!ctx || ctx->magic != 0xDEADBEEF) return;
    ctx->on_frame = callback;
    ctx->user_data = user_data;
    
    ctx->encoder.setNALCallback([](const uint8_t* d, size_t s, int k, uint64_t ts, void* ud) {
        auto c = static_cast<BridgeContext*>(ud);
        if (c->on_frame) {
            c->on_frame(d, s, k, ts, c->user_data);
        }
    }, ctx);
}

bool bridge_start(BridgeContext* ctx) {
    if (!ctx || ctx->magic != 0xDEADBEEF) return false;
    return camera_start(ctx->camera);
}

void bridge_stop(BridgeContext* ctx) {
    if (!ctx || ctx->magic != 0xDEADBEEF) return;
    camera_stop(ctx->camera);
    ctx->encoder.stop();
}

void bridge_request_keyframe(BridgeContext* ctx) {
    if (!ctx || ctx->magic != 0xDEADBEEF) return;
    ctx->encoder.requestKeyframe();
}

const char* bridge_get_error(BridgeContext* ctx) {
    if (!ctx || ctx->magic != 0xDEADBEEF) return "Invalid Context";
    return camera_get_error(ctx->camera);
}

void bridge_free(BridgeContext* ctx) {
    if (!ctx || ctx->magic != 0xDEADBEEF) return;
    camera_destroy(ctx->camera);
    delete ctx;
}

} // extern "C"
