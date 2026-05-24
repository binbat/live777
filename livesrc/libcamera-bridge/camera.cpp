#include "camera.h"
#include "include/capture_backend.h"
#include <libcamera/libcamera.h>
#include <sys/mman.h>
#include <unistd.h>
#include <vector>
#include <map>
#include <string>
#include <memory>
#include <cstdio>
#include <cstdint>
#include <chrono>

using namespace libcamera;

// Private implementation class
struct FastMapping {
    Request* req;
    uint8_t* data;
    size_t size;
    void* impl; 
};

#define MAX_REGISTRY 16
static FastMapping g_registry[MAX_REGISTRY];
static volatile int g_registry_count = 0;

class PiCameraImpl : public CaptureBackend {
public:
    uint32_t magic = 0xBCBCBCBC;
    std::unique_ptr<CameraManager> cameraManager;
    std::shared_ptr<libcamera::Camera> camera;
    Stream* videoStream = nullptr;
    std::vector<std::unique_ptr<Request>> requests;
    GlobalFrameCallback callback = nullptr;
    void* userData = nullptr;
    bool running = false;
    int target_fps = 30;
    int width_ = 0;
    int height_ = 0;
    uint64_t seq_ = 0;
    CaptureFrameCallback capture_cb_;
    std::unique_ptr<FrameBufferAllocator> allocator_;

    // --- CaptureBackend overrides ---
    bool init(const CaptureConfig& cfg, std::string* err) override;
    bool start(CaptureFrameCallback cb, std::string* err) override;
    void stop() override;
    bool isRunning() const override;
};

// THE V14.6 "SCREAMING" SLOT
static void stable_slot_v14_6(Request* request) {
    if (!request) return;

    // SCERAM: I AM ALIVE!
    static int global_counter = 0;
    if (global_counter % 10 == 0) {
        fprintf(stderr, "[CameraInternal] SLOT TRIGGERED! req=%p, status=%d\n", request, request->status());
    }
    global_counter++;

    FastMapping* entry = nullptr;
    for (int i = 0; i < g_registry_count; ++i) {
        if (g_registry[i].req == request) {
            entry = &g_registry[i];
            break;
        }
    }

    if (entry && entry->impl) {
        PiCameraImpl* impl = static_cast<PiCameraImpl*>(entry->impl);
        if (impl->magic == 0xBCBCBCBC) {
            static auto start_time = std::chrono::steady_clock::now();
            auto now = std::chrono::steady_clock::now();
            uint64_t timestamp = std::chrono::duration_cast<std::chrono::microseconds>(now - start_time).count();

            if (impl->callback) {
                impl->callback(entry->data, entry->size, timestamp, impl->userData);
            }

            // Dispatch RawFrame via CaptureBackend callback (new path)
            if (impl->capture_cb_) {
                RawFrame f{};
                f.kind = BufferKind::Cpu;
                f.format = RawPixelFormat::Yuv420p;
                f.width = static_cast<uint32_t>(impl->width_);
                f.height = static_cast<uint32_t>(impl->height_);
                f.pts_us = timestamp;
                f.seq = ++impl->seq_;
                f.plane_count = 1;
                f.planes[0] = {
                    entry->data,
                    static_cast<uint32_t>(impl->width_),
                    static_cast<uint32_t>(entry->size),
                    -1,
                    0
                };
                impl->capture_cb_(f);
            }
        }
        
        // Reuse and RE-QUEUE
        request->reuse(Request::ReuseFlag::ReuseBuffers);
        if (impl->running) {
            impl->camera->queueRequest(request);
        }
    } else {
        fprintf(stderr, "[CameraInternal] Registry MISS for req=%p! (RegistryCount=%d)\n", request, g_registry_count);
    }
}

// ---------------------------------------------------------------------------
// CaptureBackend implementation
// ---------------------------------------------------------------------------
bool PiCameraImpl::init(const CaptureConfig& cfg, std::string* err) {
    width_ = static_cast<int>(cfg.width);
    height_ = static_cast<int>(cfg.height);
    target_fps = static_cast<int>(cfg.fps);

    cameraManager = std::make_unique<CameraManager>();
    if (cameraManager->start() != 0) {
        if (err) *err = "CameraManager failed to start";
        return false;
    }
    if (cameraManager->cameras().empty()) {
        if (err) *err = "No cameras found";
        return false;
    }
    camera = cameraManager->get(cameraManager->cameras()[0]->id());
    if (!camera || camera->acquire() != 0) {
        if (err) *err = "Failed to acquire camera";
        return false;
    }

    std::unique_ptr<CameraConfiguration> config =
        camera->generateConfiguration({StreamRole::VideoRecording});
    StreamConfiguration& sc = config->at(0);
    sc.size.width = static_cast<unsigned int>(cfg.width);
    sc.size.height = static_cast<unsigned int>(cfg.height);
    sc.pixelFormat = formats::YUV420;
    sc.bufferCount = 8;

    if (config->validate() == CameraConfiguration::Invalid) {
        fprintf(stderr, "[CameraInternal] Config was invalid, adjusted.\n");
    }
    if (camera->configure(config.get()) < 0) {
        if (err) *err = "Camera configure failed";
        return false;
    }

    videoStream = sc.stream();
    allocator_ = std::make_unique<FrameBufferAllocator>(camera);
    if (allocator_->allocate(videoStream) < 0) {
        if (err) *err = "Buffer allocation failed";
        return false;
    }

    g_registry_count = 0;
    for (const auto& buffer : allocator_->buffers(videoStream)) {
        FrameBuffer* ptr = buffer.get();
        size_t s = 0;
        for (const auto& p : ptr->planes()) s += p.length;

        void* d = mmap(NULL, s, PROT_READ, MAP_SHARED,
                       ptr->planes()[0].fd.get(), 0);

        std::unique_ptr<Request> r = camera->createRequest();
        r->addBuffer(videoStream, ptr);

        if (g_registry_count < MAX_REGISTRY) {
            g_registry[g_registry_count].req = r.get();
            g_registry[g_registry_count].data = static_cast<uint8_t*>(d);
            g_registry[g_registry_count].size = s;
            g_registry[g_registry_count].impl = this;
            g_registry_count++;
        }
        requests.push_back(std::move(r));
    }
    return true;
}

bool PiCameraImpl::start(CaptureFrameCallback cb, std::string* err) {
    (void)err;
    capture_cb_ = std::move(cb);
    camera->requestCompleted.connect(stable_slot_v14_6);

    ControlList controls;
    int64_t frame_duration = 1000000 / target_fps;
    controls.set(controls::FrameDurationLimits, {frame_duration, frame_duration});

    int ret = camera->start(&controls);
    if (ret < 0) return false;

    running = true;
    for (auto& r : requests) {
        camera->queueRequest(r.get());
    }
    return true;
}

void PiCameraImpl::stop() {
    if (!running) return;
    running = false;
    if (camera) {
        camera->stop();
        camera->requestCompleted.disconnect(stable_slot_v14_6);
    }
    capture_cb_ = nullptr;
}

bool PiCameraImpl::isRunning() const {
    return running;
}

extern "C" {

CameraHandle camera_create() {
    return new PiCameraImpl();
}

void camera_destroy(CameraHandle handle) {
    if (!handle) return;
    delete static_cast<PiCameraImpl*>(handle);
}

bool camera_init(CameraHandle handle, const CameraParams* params) {
    if (!handle || !params) return false;
    PiCameraImpl* impl = static_cast<PiCameraImpl*>(handle);
    impl->target_fps = params->fps; // V14.9-TURBO: Save the target FPS

    impl->cameraManager = std::make_unique<CameraManager>();
    if (impl->cameraManager->start() != 0) return false;
    if (impl->cameraManager->cameras().empty()) return false;
    impl->camera = impl->cameraManager->get(impl->cameraManager->cameras()[0]->id());
    if (!impl->camera || impl->camera->acquire() != 0) return false;

    std::unique_ptr<CameraConfiguration> config = impl->camera->generateConfiguration({StreamRole::VideoRecording});
    StreamConfiguration& sc = config->at(0);
    sc.size.width = params->width;
    sc.size.height = params->height;
    sc.pixelFormat = formats::YUV420;
    sc.bufferCount = 8;

    if (config->validate() == CameraConfiguration::Invalid) {
        fprintf(stderr, "[CameraInternal] Config was invalid, adjusted.\n");
    }
    fprintf(stderr, "[CameraInternal] Validated Format: %s, %dx%d\n", sc.pixelFormat.toString().c_str(), sc.size.width, sc.size.height);

    if (impl->camera->configure(config.get()) < 0) return false;

    impl->videoStream = sc.stream();
    FrameBufferAllocator* allocator = new FrameBufferAllocator(impl->camera);
    if (allocator->allocate(impl->videoStream) < 0) return false;

    g_registry_count = 0;
    for (const auto& buffer : allocator->buffers(impl->videoStream)) {
        FrameBuffer* ptr = buffer.get();
        size_t s = 0;
        for (const auto& p : ptr->planes()) s += p.length;
        
        // Map as single block - standard for Pi YUV
        void* d = mmap(NULL, s, PROT_READ, MAP_SHARED, ptr->planes()[0].fd.get(), 0);
        
        std::unique_ptr<Request> r = impl->camera->createRequest();
        r->addBuffer(impl->videoStream, ptr);
        
        if (g_registry_count < MAX_REGISTRY) {
            g_registry[g_registry_count].req = r.get();
            g_registry[g_registry_count].data = static_cast<uint8_t*>(d);
            g_registry[g_registry_count].size = s;
            g_registry[g_registry_count].impl = impl;
            fprintf(stderr, "[Registry] [%d] Request=%p, Map=%p\n", g_registry_count, r.get(), d);
            g_registry_count++;
        }
        impl->requests.push_back(std::move(r));
    }
    return true;
}

bool camera_start(CameraHandle handle) {
    if (!handle) return false;
    PiCameraImpl* impl = static_cast<PiCameraImpl*>(handle);
    impl->camera->requestCompleted.connect(stable_slot_v14_6);

    // V14.9-TURBO: Force 30 FPS by locking FrameDurationLimits
    ControlList controls;
    int64_t frame_duration = 1000000 / impl->target_fps;
    controls.set(controls::FrameDurationLimits, { frame_duration, frame_duration });
    
    int ret = impl->camera->start(&controls);
    fprintf(stderr, "[CameraInternal] Camera Start Return: %d\n", ret);
    if (ret < 0) return false;

    impl->running = true;
    for (auto& r : impl->requests) {
        int qret = impl->camera->queueRequest(r.get());
        if (qret < 0) fprintf(stderr, "[CameraInternal] Queue FAILED: %d\n", qret);
    }
    return true;
}

void camera_stop(CameraHandle handle) {
    if (!handle) return;
    PiCameraImpl* impl = static_cast<PiCameraImpl*>(handle);
    impl->running = false;
    impl->camera->stop();
    impl->camera->requestCompleted.disconnect(stable_slot_v14_6);
}

void camera_set_callback(CameraHandle handle, GlobalFrameCallback callback, void* user_data) {
    if (!handle) return;
    PiCameraImpl* impl = static_cast<PiCameraImpl*>(handle);
    impl->callback = callback;
    impl->userData = user_data;
}

const char* camera_get_error(CameraHandle handle) {
    (void)handle; return "";
}

} // extern "C"

// ---------------------------------------------------------------------------
// Factory for CaptureBackend (libcamera)
// ---------------------------------------------------------------------------
std::unique_ptr<CaptureBackend> create_capture_backend(const CaptureConfig& cfg) {
    auto impl = std::make_unique<PiCameraImpl>();
    std::string err;
    if (!impl->init(cfg, &err)) {
        fprintf(stderr, "[CameraFactory] init failed: %s\n", err.c_str());
        return nullptr;
    }
    return impl;
}
