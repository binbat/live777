#include "bridge_ffi.h"
#include "encoder.h"
#include <cstdio>
#include <cstring>
#include <cstdlib>

extern "C" {
    #include "v4l2_capture.h"
}

// 内部结构体定义
struct V4L2BridgeContext {
    V4L2CaptureHandle capture = nullptr;
    Encoder* encoder = nullptr;
    NALCallbackFFI callback = nullptr;
    void* user_data = nullptr;
};

extern "C" {

// 1. 实现 Rust 侧调用的 V4L2 接口
V4L2BridgeContext* v4l2_bridge_init(const char* device, int width, int height, int fps, int bitrate) {
    auto* ctx = new V4L2BridgeContext();
    ctx->encoder = new Encoder();

    V4L2CaptureParams cap_params;
    cap_params.device = device;
    cap_params.width = width;
    cap_params.height = height;
    cap_params.fps = fps;
    cap_params.input_format = 0; // YUYV

    ctx->capture = v4l2cap_create();
    if (!v4l2cap_init(ctx->capture, &cap_params)) {
        fprintf(stderr, "[V4L2Bridge-RDK] Capture init failed for %s: %s\n", device, v4l2cap_get_error(ctx->capture));
        delete ctx->encoder;
        v4l2cap_destroy(ctx->capture);
        delete ctx;
        return nullptr;
    }

    CameraParams enc_params;
    enc_params.width = width;
    enc_params.height = height;
    enc_params.fps = fps;
    enc_params.bitrate = bitrate;

    if (!ctx->encoder->init(enc_params)) {
        fprintf(stderr, "[V4L2Bridge-RDK] Encoder init failed\n");
        v4l2cap_destroy(ctx->capture);
        delete ctx->encoder;
        delete ctx;
        return nullptr;
    }

    // 设置回调链
    v4l2cap_set_callback(ctx->capture, [](const uint8_t* data, size_t size, uint64_t ts_us, void* user) {
        auto* c = static_cast<V4L2BridgeContext*>(user);
        if (data && c->encoder) {
            c->encoder->encode(data, size, ts_us);
        }
    }, ctx);

    ctx->encoder->setNALCallback([](const uint8_t* data, size_t size, int is_kf, uint64_t ts, void* user) {
        auto* c = static_cast<V4L2BridgeContext*>(user);
        if (c->callback) {
            c->callback(data, size, is_kf, ts, c->user_data);
        }
    }, ctx);

    return ctx;
}

void v4l2_bridge_set_callback(V4L2BridgeContext* ctx, NALCallbackFFI callback, void* user_data) {
    if (ctx) {
        ctx->callback = callback;
        ctx->user_data = user_data;
    }
}

bool v4l2_bridge_start(V4L2BridgeContext* ctx) {
    if (!ctx) return false;
    return v4l2cap_start(ctx->capture);
}

void v4l2_bridge_stop(V4L2BridgeContext* ctx) {
    if (ctx) v4l2cap_stop(ctx->capture);
}

bool v4l2_bridge_is_running(V4L2BridgeContext* ctx) {
    return ctx && v4l2cap_is_running(ctx->capture);
}

void v4l2_bridge_request_keyframe(V4L2BridgeContext* ctx) {
    if (ctx && ctx->encoder) ctx->encoder->requestKeyframe();
}

const char* v4l2_bridge_get_error(V4L2BridgeContext* ctx) {
    return ctx ? v4l2cap_get_error(ctx->capture) : "Invalid context";
}

void v4l2_bridge_free(V4L2BridgeContext* ctx) {
    if (ctx) {
        v4l2cap_destroy(ctx->capture);
        if (ctx->encoder) delete ctx->encoder;
        delete ctx;
    }
}

// 2. 修复 bridge_ffi.h 中定义的 stubs (必须严格按照函数签名)
BridgeContext* bridge_init(int w, int h, int f, int b, int ci, int rot, int hf, int vf) { return nullptr; }
void bridge_cleanup() {}
void bridge_start_libcamera() {}
void bridge_stop_libcamera() {}
const char* bridge_get_error(BridgeContext* ctx) { return ""; }
void bridge_request_keyframe(BridgeContext* ctx) {}
void bridge_set_callback(BridgeContext* ctx, NALCallbackFFI cb, void* ud) {}
bool bridge_start(BridgeContext* ctx) { return false; }
void bridge_stop(BridgeContext* ctx) {}
void bridge_free(BridgeContext* ctx) {}

} // extern "C"
