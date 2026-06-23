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
#include <mutex>
#include <atomic>

using namespace libcamera;

// ---------------------------------------------------------------------------
// Per-instance request registry.
//
// Replaces the previous global, lock-free, magic-number registry with a
// mutex-protected map owned by each PiCameraImpl.  The static completion
// slot looks up the instance via a global instance table keyed by a unique
// id, then delegates to the instance's on_request_completed().
// ---------------------------------------------------------------------------

struct MappedRequest {
    Request* request = nullptr;
    FrameBuffer* framebuffer = nullptr;
    // Per-plane mmap results: (address, length).
    std::vector<std::pair<void*, size_t>> plane_mappings;
    // Contiguous CPU copy of all planes for the callback.
    std::vector<uint8_t> contiguous;
    size_t total_size = 0;
};

static std::mutex g_instance_mutex;
static std::map<Request*, class PiCameraImpl*> g_request_to_instance;

class PiCameraImpl : public CaptureBackend {
public:
    std::atomic<bool> destroying_{false};

    std::unique_ptr<CameraManager> cameraManager;
    std::shared_ptr<libcamera::Camera> camera;
    Stream* videoStream = nullptr;
    std::vector<std::unique_ptr<Request>> requests;
    bool running = false;
    int target_fps = 30;
    int width_ = 0;
    int height_ = 0;
    uint64_t seq_ = 0;
    CaptureFrameCallback capture_cb_;
    std::unique_ptr<FrameBufferAllocator> allocator_;

    std::mutex registry_mutex_;
    std::map<Request*, MappedRequest> registry_;

    ~PiCameraImpl() override {
        destroying_.store(true);
        stop();
        release_resources();
    }

    void release_resources();
    void on_request_completed(Request* request);

    // --- CaptureBackend overrides ---
    bool init(const CaptureConfig& cfg, std::string* err) override;
    bool start(CaptureFrameCallback cb, std::string* err) override;
    void stop() override;
    bool isRunning() const override;
};

static void request_completed_slot(Request* request) {
    if (!request) return;

    PiCameraImpl* impl = nullptr;
    {
        std::lock_guard<std::mutex> lock(g_instance_mutex);
        auto it = g_request_to_instance.find(request);
        if (it != g_request_to_instance.end()) impl = it->second;
    }

    if (!impl || impl->destroying_.load()) return;
    impl->on_request_completed(request);
}

void PiCameraImpl::release_resources() {
    if (camera) {
        camera->requestCompleted.disconnect(request_completed_slot);
    }

    // Unregister requests from the global lookup before destroying them.
    {
        std::lock_guard<std::mutex> lock(g_instance_mutex);
        for (const auto& request : requests) {
            g_request_to_instance.erase(request.get());
        }
    }

    // Drop requests before freeing buffers.
    requests.clear();

    // Unmap all per-plane mappings.
    {
        std::lock_guard<std::mutex> lock(registry_mutex_);
        for (auto& [req, mapped] : registry_) {
            (void)req;
            for (auto& mapping : mapped.plane_mappings) {
                if (mapping.first && mapping.first != MAP_FAILED) {
                    munmap(mapping.first, mapping.second);
                }
            }
        }
        registry_.clear();
    }

    if (allocator_ && videoStream) {
        allocator_->free(videoStream);
    }
    allocator_.reset();

    if (camera) {
        camera->release();
        camera.reset();
    }

    if (cameraManager) {
        cameraManager->stop();
    }
}

void PiCameraImpl::on_request_completed(Request* request) {
    MappedRequest mapped;
    {
        std::lock_guard<std::mutex> lock(registry_mutex_);
        auto it = registry_.find(request);
        if (it == registry_.end()) {
            fprintf(stderr, "[CameraInternal] Registry MISS for req=%p\n", request);
            return;
        }
        mapped = it->second;
    }

    // Copy each plane into the contiguous buffer.
    uint8_t* dst = mapped.contiguous.data();
    size_t offset = 0;
    const auto& planes = mapped.framebuffer->planes();
    for (size_t i = 0; i < planes.size() && i < mapped.plane_mappings.size(); ++i) {
        const auto& plane = planes[i];
        void* src = mapped.plane_mappings[i].first;
        if (src && src != MAP_FAILED) {
            memcpy(dst + offset, src, plane.length);
        }
        offset += plane.length;
    }

    if (capture_cb_) {
        auto now = std::chrono::steady_clock::now();
        static auto start_time = now;
        uint64_t timestamp =
            std::chrono::duration_cast<std::chrono::microseconds>(now - start_time).count();

        RawFrame f{};
        f.kind = BufferKind::Cpu;
        f.format = RawPixelFormat::Yuv420p;
        f.width = static_cast<uint32_t>(width_);
        f.height = static_cast<uint32_t>(height_);
        f.pts_us = timestamp;
        f.seq = ++seq_;
        f.plane_count = 1;
        f.planes[0] = {
            mapped.contiguous.data(),
            static_cast<uint32_t>(width_),
            static_cast<uint32_t>(mapped.total_size),
            -1,
            0,
        };
        capture_cb_(f);
    }

    // Reuse and re-queue if still running.
    request->reuse(Request::ReuseFlag::ReuseBuffers);
    if (running && camera) {
        camera->queueRequest(request);
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

    CameraConfiguration::Status validation = config->validate();
    if (validation == CameraConfiguration::Invalid) {
        if (err) *err = "Camera configuration invalid";
        return false;
    }
    if (validation == CameraConfiguration::Adjusted) {
        fprintf(stderr, "[CameraInternal] Config was adjusted to %ux%u\n",
                sc.size.width, sc.size.height);
    }
    if (camera->configure(config.get()) < 0) {
        if (err) *err = "Camera configure failed";
        return false;
    }

    // Honor the size negotiated by the camera.
    width_ = static_cast<int>(sc.size.width);
    height_ = static_cast<int>(sc.size.height);

    videoStream = sc.stream();
    allocator_ = std::make_unique<FrameBufferAllocator>(camera);
    if (allocator_->allocate(videoStream) < 0) {
        if (err) *err = "Buffer allocation failed";
        return false;
    }

    for (const auto& buffer : allocator_->buffers(videoStream)) {
        FrameBuffer* framebuffer = buffer.get();
        MappedRequest mapped;
        mapped.framebuffer = framebuffer;
        mapped.total_size = 0;

        // mmap each plane individually and prepare a contiguous copy buffer.
        for (const auto& plane : framebuffer->planes()) {
            void* addr = mmap(nullptr, plane.length, PROT_READ, MAP_SHARED,
                              plane.fd.get(), plane.offset);
            if (addr == MAP_FAILED) {
                if (err) *err = "mmap failed for camera buffer plane";
                // Unmap already mapped planes.
                for (auto& mapping : mapped.plane_mappings) {
                    if (mapping.first && mapping.first != MAP_FAILED) {
                        munmap(mapping.first, mapping.second);
                    }
                }
                release_resources();
                return false;
            }
            mapped.plane_mappings.push_back({addr, plane.length});
            mapped.total_size += plane.length;
        }
        mapped.contiguous.resize(mapped.total_size);

        std::unique_ptr<Request> request = camera->createRequest();
        if (!request) {
            if (err) *err = "createRequest failed";
            release_resources();
            return false;
        }
        if (request->addBuffer(videoStream, framebuffer) < 0) {
            if (err) *err = "addBuffer failed";
            release_resources();
            return false;
        }

        mapped.request = request.get();
        {
            std::lock_guard<std::mutex> lock(registry_mutex_);
            registry_[mapped.request] = std::move(mapped);
        }
        {
            std::lock_guard<std::mutex> lock(g_instance_mutex);
            g_request_to_instance[mapped.request] = this;
        }
        requests.push_back(std::move(request));
    }
    return true;
}

bool PiCameraImpl::start(CaptureFrameCallback cb, std::string* err) {
    (void)err;
    if (running) return false;

    capture_cb_ = std::move(cb);
    camera->requestCompleted.connect(request_completed_slot);

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
        camera->requestCompleted.disconnect(request_completed_slot);
    }
    capture_cb_ = nullptr;
}

bool PiCameraImpl::isRunning() const {
    return running;
}

// ---------------------------------------------------------------------------
// Factory for CaptureBackend (libcamera)
// ---------------------------------------------------------------------------
std::unique_ptr<CaptureBackend> create_libcamera_capture_backend(const CaptureConfig& cfg) {
    (void)cfg;
    return std::make_unique<PiCameraImpl>();
}
