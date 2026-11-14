//! Allwinner AWIspApi lifecycle controller (A733 / sun60iw2 VIN capture).
//!
//! See include/sunxi_isp.h. The closed-source libAWIspApi.so server thread
//! performs ALL private subdev ioctls, media enumeration and the 3A event
//! loop. We only drive its lifecycle around VIDIOC_STREAMON/STREAMOFF. We
//! deliberately do NOT touch media-ctl links — the device tree preconfigures
//! them, and forcing S_FMT on subdevs breaks the default negotiation
//! (isp0 width error).
//!
//! AWIspApi is a pure table of C function pointers reached through
//! CreateAWIspApi()/DestroyAWIspApi(). We load it with dlopen instead of
//! linking it, so:
//!   - the native-pipeline builds on any Linux without the vendor SDK headers;
//!   - the SDK is only required at runtime on the device, where a missing
//!     libAWIspApi.so is reported as a clear error rather than a link failure;
//!   - the ABI is stable (a flat vtable of function pointers, no C++ types).

#include "include/sunxi_isp.h"

#include <dlfcn.h>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

namespace {

// Mirrors /usr/include/AWIspApi.h (Allwinner ISP602 SDK). Kept as a private
// declaration so this file has no compile-time dependency on the SDK headers.
// Field order MUST match the vendor header exactly.
struct AwIspApi {
    int (*ispApiInit)(void);
    int (*ispSetAfRegions)(int, int, int, int, int);
    int (*ispSetSceneMode)(int, int);
    int (*ispSetAeRegions)(int, int, int, int, int);
    int (*ispGetIspId)(int);
    int (*ispStart)(int);
    int (*ispSetIspLargeImage)(int);
    int (*ispStop)(int);
    int (*ispWaitToExit)(int);
    int (*ispApiUnInit)(void);
    int (*ispSetFpsRanage)(int, int);
    int (*ispGetIspGain)(int);
    int (*ispGetIspExp)(int, unsigned int*, unsigned int*);
    int (*ispGetInfoLength)(int, int*, int*);
    int (*ispGet3AParameters)(int, void*);
    int (*ispGetDebugMessage)(int, void*);
};

using CreateFn = AwIspApi* (*)(void);
using DestroyFn = void (*)(AwIspApi*);

// "/dev/video0" -> 0, "/dev/video8" -> 8 (matches the vendor demo's
// parseVideoId and awIspGetIspId's video->isp mapping).
int parse_video_id(const std::string& device) {
    const size_t pos = device.rfind("video");
    if (pos == std::string::npos) {
        return -1;
    }
    try {
        return std::stoi(device.substr(pos + 5));
    } catch (...) {
        return -1;
    }
}

}  // namespace

struct SunxiIspController::Impl {
    void* handle = nullptr;
    AwIspApi* api = nullptr;
    DestroyFn destroy = nullptr;
    int isp_id = -1;
    bool api_initialized = false;  // ispApiInit() succeeded
    bool started = false;          // ispStart() succeeded
};

SunxiIspController::SunxiIspController() : impl_(std::make_unique<Impl>()) {}

SunxiIspController::~SunxiIspController() { stop(); }

bool SunxiIspController::init(const std::string& device, std::string* err) {
    const int video_id = parse_video_id(device);
    if (video_id < 0) {
        if (err) {
            *err = "sunxi ISP: cannot parse video id from '" + device + "'";
        }
        return false;
    }

    impl_->handle = dlopen("libAWIspApi.so", RTLD_NOW | RTLD_LOCAL);
    if (!impl_->handle) {
        if (err) {
            *err = std::string("sunxi ISP: dlopen(libAWIspApi.so) failed: ") + dlerror();
        }
        return false;
    }
    dlerror();  // clear
    auto create = reinterpret_cast<CreateFn>(dlsym(impl_->handle, "CreateAWIspApi"));
    impl_->destroy = reinterpret_cast<DestroyFn>(dlsym(impl_->handle, "DestroyAWIspApi"));
    const char* dlsym_err = dlerror();
    if (dlsym_err || !create || !impl_->destroy) {
        if (err) {
            *err = std::string("sunxi ISP: dlsym(CreateAWIspApi/DestroyAWIspApi) failed: ")
                   + (dlsym_err ? dlsym_err : "null symbol");
        }
        dlclose(impl_->handle);
        impl_->handle = nullptr;
        return false;
    }

    impl_->api = create();
    if (!impl_->api) {
        if (err) *err = "sunxi ISP: CreateAWIspApi() returned null";
        stop();
        return false;
    }
    if (impl_->api->ispApiInit() < 0) {
        if (err) *err = "sunxi ISP: ispApiInit() failed";
        stop();
        return false;
    }
    impl_->api_initialized = true;
    impl_->isp_id = impl_->api->ispGetIspId(video_id);
    if (impl_->isp_id < 0) {
        if (err) {
            *err = "sunxi ISP: ispGetIspId(video" + std::to_string(video_id)
                   + ") failed (no ISP for this device?)";
        }
        stop();
        return false;
    }

    std::fprintf(stderr, "[SunxiISP] AWIspApi ready: video%d --> isp%d\n",
                 video_id, impl_->isp_id);
    return true;
}

bool SunxiIspController::start(uint32_t fps, std::string* err) {
    if (!impl_->api || impl_->isp_id < 0) {
        if (err) *err = "sunxi ISP: start() called before successful init()";
        return false;
    }
    if (impl_->api->ispStart(impl_->isp_id) < 0) {
        if (err) *err = "sunxi ISP: ispStart() failed";
        return false;
    }
    impl_->started = true;
    std::fprintf(stderr, "[SunxiISP] ispStart(isp%d) ok (before STREAMON)\n",
                 impl_->isp_id);
    // Tell the ISP 3A loop the configured operating rate. Best-effort: the
    // sensor mode table still governs the delivered rate, but without this
    // the AE/AWB convergence assumes the driver default.
    if (fps != 0 && impl_->api->ispSetFpsRanage
        && impl_->api->ispSetFpsRanage(impl_->isp_id, static_cast<int>(fps)) < 0) {
        std::fprintf(stderr, "[SunxiISP] WARNING: ispSetFpsRanage(%u) failed\n", fps);
    }
    return true;
}

void SunxiIspController::stop() {
    if (!impl_) {
        return;
    }
    if (impl_->api) {
        if (impl_->started) {
            impl_->api->ispStop(impl_->isp_id);
            impl_->api->ispWaitToExit(impl_->isp_id);
            impl_->started = false;
        }
        // Un-init only if ispApiInit() actually succeeded: init() may have
        // bailed out earlier (e.g. ispGetIspId failure) with the api alive.
        if (impl_->api_initialized) {
            impl_->api->ispApiUnInit();
            impl_->api_initialized = false;
        }
        if (impl_->destroy) {
            impl_->destroy(impl_->api);
        }
        impl_->api = nullptr;
    }
    if (impl_->handle) {
        dlclose(impl_->handle);
        impl_->handle = nullptr;
    }
    impl_->isp_id = -1;
    impl_->started = false;
}
