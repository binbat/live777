#include "camera.h"
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

class PiCameraImpl {
public:
    uint32_t magic = 0xBCBCBCBC;
    std::unique_ptr<CameraManager> cameraManager;
    std::shared_ptr<libcamera::Camera> camera;
    Stream* videoStream = nullptr;
    std::vector<std::unique_ptr<Request>> requests;
    GlobalFrameCallback callback = nullptr;
    void* userData = nullptr;
    bool running = false;
    int target_fps = 30; // V14.9-TURBO: Store FPS
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
