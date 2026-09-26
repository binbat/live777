#include "include/capture_backend.h"
#include <libcamera/libcamera.h>
#include <sys/mman.h>
#include <unistd.h>
#include <vector>
#include <map>
#include <string>
#include <memory>
#include <algorithm>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <chrono>
#include <mutex>
#include <atomic>
#include <condition_variable>

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
    // Index of this buffer in allocation order; the zero-copy release
    // contract identifies the buffer by it.
    uint32_t buffer_index = 0;
    // Single dma-buf fd shared by all planes (different offsets), -1 when
    // the buffer is not exportable.
    int dma_fd = -1;
    // Single mmap of the whole FrameBuffer (libcamera planes live in one
    // dma-buf with different offsets).  base_addr may be nullptr if the
    // buffer has zero length.
    void* base_addr = nullptr;
    size_t mapped_size = 0;
    // Contiguous CPU copy of all planes for the callback.
    std::vector<uint8_t> contiguous;
    size_t total_size = 0;
};

static std::mutex g_instance_mutex;
static std::map<Request*, std::weak_ptr<class PiCameraImpl>> g_request_to_instance;

class PiCameraImpl : public CaptureBackend, public std::enable_shared_from_this<PiCameraImpl> {
public:
    std::atomic<bool> destroying_{false};

    std::unique_ptr<CameraManager> cameraManager;
    std::shared_ptr<libcamera::Camera> camera;
    Stream* videoStream = nullptr;
    std::vector<std::unique_ptr<Request>> requests;
    std::atomic<bool> running{false};
    int target_fps = 30;
    int width_ = 0;
    int height_ = 0;
    std::atomic<uint64_t> seq_{0};
    CaptureFrameCallback capture_cb_;
    mutable std::mutex state_mutex_;
    std::unique_ptr<FrameBufferAllocator> allocator_;

    std::mutex registry_mutex_;
    std::map<Request*, MappedRequest> registry_;

    // Zero-copy (prefer_dmabuf): requests handed to the consumer via an
    // armed RawFrame::release stay out of the capture queue until
    // release_request() requeues them.  Indexed by buffer_index.
    bool prefer_dmabuf_ = false;
    std::mutex release_mutex_;
    std::vector<Request*> held_requests_;

    // Per-instance timestamp base so multiple capture backends do not share
    // the same epoch. Initialised in start().
    std::chrono::steady_clock::time_point start_time_{};

    // Serialises on_request_completed with stop()/release_resources().
    // callbacks_in_flight_ tracks how many completion callbacks are currently
    // executing; stop() waits for it to reach zero before returning so that
    // callers can safely release resources.  User code is invoked without
    // holding completion_mutex_, avoiding deadlocks when the frame callback
    // performs work that contends on the same locks.
    std::mutex completion_mutex_;
    std::condition_variable completion_cv_;
    std::atomic<size_t> callbacks_in_flight_{0};

    static std::shared_ptr<PiCameraImpl> create() {
        return std::shared_ptr<PiCameraImpl>(new PiCameraImpl());
    }

    ~PiCameraImpl() override {
        destroying_.store(true);
        stop();
        release_resources();
    }

    void release_resources();
    void on_request_completed(Request* request);
    // Deferred requeue of a request whose buffer the consumer (encoder)
    // held via the zero-copy release contract; a no-op once stopped.
    static void release_trampoline(void* ctx, uint32_t buffer_index);
    void release_request(uint32_t buffer_index);

    // --- CaptureBackend overrides ---
    bool init(const CaptureConfig& cfg, std::string* err) override;
    bool start(CaptureFrameCallback cb, std::string* err) override;
    void stop() override;
    bool isRunning() const override;
    // libcamera frames are always converted to single-plane YUV420P.
    RawPixelFormat outputFormat() const override {
        return RawPixelFormat::Yuv420p;
    }
    bool emitsDmabufFrames() const override { return prefer_dmabuf_; }

private:
    PiCameraImpl() = default;
};

// RAII helper that increments callbacks_in_flight_ while a completion
// callback runs and notifies stop() when it finishes.
class CallbackInFlightGuard {
public:
    explicit CallbackInFlightGuard(PiCameraImpl* impl) : impl_(impl) {
        impl_->callbacks_in_flight_.fetch_add(1, std::memory_order_acq_rel);
    }
    ~CallbackInFlightGuard() {
        if (impl_->callbacks_in_flight_.fetch_sub(1, std::memory_order_acq_rel) == 1) {
            impl_->completion_cv_.notify_all();
        }
    }
    CallbackInFlightGuard(const CallbackInFlightGuard&) = delete;
    CallbackInFlightGuard& operator=(const CallbackInFlightGuard&) = delete;
private:
    PiCameraImpl* impl_;
};

static void request_completed_slot(Request* request) {
    if (!request) return;

    std::shared_ptr<PiCameraImpl> impl;
    {
        std::lock_guard<std::mutex> lock(g_instance_mutex);
        auto it = g_request_to_instance.find(request);
        if (it != g_request_to_instance.end()) {
            impl = it->second.lock();
        }
    }

    if (!impl || impl->destroying_.load()) return;

    // Mark the callback in-flight for the entire duration of the slot,
    // including the early return path.  This ensures stop()/the destructor
    // waits until we are done touching PiCameraImpl state before releasing
    // resources.
    CallbackInFlightGuard guard(impl.get());
    impl->on_request_completed(request);
}

void PiCameraImpl::release_resources() {
    std::lock_guard<std::mutex> lock(completion_mutex_);

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
    {
        std::lock_guard<std::mutex> lock(release_mutex_);
        held_requests_.clear();
    }

    // Unmap all buffer mappings.
    {
        std::lock_guard<std::mutex> lock(registry_mutex_);
        for (auto& [req, mapped] : registry_) {
            (void)req;
            if (mapped.base_addr && mapped.base_addr != MAP_FAILED) {
                munmap(mapped.base_addr, mapped.mapped_size);
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
    if (destroying_.load()) return;

    MappedRequest* mapped = nullptr;
    {
        std::lock_guard<std::mutex> lock(registry_mutex_);
        auto it = registry_.find(request);
        if (it == registry_.end()) {
            fprintf(stderr, "[CameraInternal] Registry MISS for req=%p\n", request);
            return;
        }
        mapped = &it->second;
    }

    const auto& planes = mapped->framebuffer->planes();
    const size_t plane_rows[3] = {
        static_cast<size_t>(height_),
        static_cast<size_t>(height_ / 2),
        static_cast<size_t>(height_ / 2),
    };
    const size_t plane_widths[3] = {
        static_cast<size_t>(width_),
        static_cast<size_t>(width_ / 2),
        static_cast<size_t>(width_ / 2),
    };

    CaptureFrameCallback cb;
    {
        std::lock_guard<std::mutex> lock(state_mutex_);
        cb = capture_cb_;
    }

    // Absolute steady_clock (CLOCK_MONOTONIC epoch on Linux) — the same
    // clock domain the V4L2 backends stamp buffer SOF in, so downstream
    // [latency] stages measure true frame age on every capture backend.
    auto now = std::chrono::steady_clock::now();
    uint64_t timestamp =
        std::chrono::duration_cast<std::chrono::microseconds>(
            now.time_since_epoch()).count();

    // Zero-copy hand-off: deliver the dma-buf planes as-is (no staging
    // copy) and do NOT requeue the request — release_request() requeues it
    // once the consumer (encoder) is done reading the buffer
    // (media_types.h deferred-requeue contract).  Requests cancelled by
    // camera->stop() carry no valid frame and must skip the hand-off.
    if (cb && prefer_dmabuf_ && planes.size() == 3 && mapped->dma_fd >= 0
        && mapped->base_addr && mapped->base_addr != MAP_FAILED
        && request->status() == Request::RequestComplete) {
        RawFrame f{};
        f.kind = BufferKind::DmaBuf;
        f.format = RawPixelFormat::Yuv420p;
        f.width = static_cast<uint32_t>(width_);
        f.height = static_cast<uint32_t>(height_);
        f.pts_us = timestamp;
        f.seq = ++seq_;
        f.plane_count = 3;
        for (size_t i = 0; i < 3; ++i) {
            const auto& plane = planes[i];
            const size_t rows = plane_rows[i];
            const uint32_t stride =
                rows > 0 ? static_cast<uint32_t>(plane.length / rows) : 0;
            f.planes[i] = {
                static_cast<const uint8_t*>(mapped->base_addr) + plane.offset,
                stride,
                plane.length,
                mapped->dma_fd,
                plane.offset,
            };
        }
        f.buffer_index = mapped->buffer_index;
        f.release = &PiCameraImpl::release_trampoline;
        f.release_ctx = this;
        cb(f);
        return;
    }

    // Copy each plane into the contiguous staging buffer, compacting away
    // row padding: the ISP may stride-align plane rows (e.g. 1296-wide
    // frames come back with a padded pitch), while the frame contract
    // downstream is a single tightly packed I420 blob with stride == width.
    // The staging buffer is not allocated in zero-copy mode; requests that
    // land here without one (cancelled by camera->stop()) carry no frame
    // and skip straight to the requeue logic below.
    if (!mapped->contiguous.empty()) {
    uint8_t* dst = mapped->contiguous.data();
    size_t offset = 0;
    bool padded = false;
    for (size_t i = 0; i < planes.size(); ++i) {
        const auto& plane = planes[i];
        const size_t rows = plane_rows[std::min(i, static_cast<size_t>(2))];
        const size_t row_bytes = plane_widths[std::min(i, static_cast<size_t>(2))];
        if (mapped->base_addr && mapped->base_addr != MAP_FAILED) {
            const uint8_t* src =
                static_cast<const uint8_t*>(mapped->base_addr) + plane.offset;
            if (plane.length == rows * row_bytes) {
                // Already tightly packed — one copy for the whole plane.
                memcpy(dst + offset, src, plane.length);
                offset += plane.length;
            } else {
                // Padded rows: copy row by row, dropping the pitch.
                padded = true;
                const size_t src_stride = plane.length / rows;
                for (size_t row = 0; row < rows; ++row) {
                    memcpy(dst + offset, src + row * src_stride, row_bytes);
                    offset += row_bytes;
                }
            }
        } else {
            offset += rows * row_bytes;
        }
    }
    if (padded && seq_ == 0) {
        fprintf(stderr,
                "[CameraInternal] %dx%d: compacting stride-padded planes"
                " (plane lengths: %zu planes, first=%u)\n",
                width_, height_, planes.size(),
                planes.empty() ? 0u : planes[0].length);
    }

    // Deliver the frame pointing straight at the staging buffer — no second
    // full-frame copy.  This is safe because the request has NOT been
    // requeued yet: the ISP cannot overwrite the dma-buf, and no later
    // completion of this request can touch `contiguous`, for the whole
    // duration of the callback.  stop() waits for in-flight callbacks
    // before resources are released, so the staging buffer outlives cb.
    if (cb) {
        RawFrame f{};
        f.kind = BufferKind::Cpu;
        f.format = RawPixelFormat::Yuv420p;
        f.width = static_cast<uint32_t>(width_);
        f.height = static_cast<uint32_t>(height_);
        f.pts_us = timestamp;
        f.seq = ++seq_;
        f.plane_count = 1;
        f.planes[0] = {
            mapped->contiguous.data(),
            static_cast<uint32_t>(width_),
            static_cast<uint32_t>(offset),
            -1,
            0,
        };
        cb(f);
    }
    }

    // Requeue only after the callback is done with the frame.  Re-read the
    // state: stop() may have run while the callback executed (it blocks on
    // callbacks_in_flight_, so `camera` is still alive here).
    bool should_queue = false;
    {
        std::lock_guard<std::mutex> lock(state_mutex_);
        should_queue = running.load() && camera != nullptr;
    }
    request->reuse(Request::ReuseFlag::ReuseBuffers);
    if (should_queue) {
        if (camera->queueRequest(request) < 0) {
            fprintf(stderr, "[CameraInternal] queueRequest failed for req=%p\n", request);
        }
    }
}

void PiCameraImpl::release_trampoline(void* ctx, uint32_t buffer_index) {
    static_cast<PiCameraImpl*>(ctx)->release_request(buffer_index);
}

// Deferred requeue for a request whose dma-buf the consumer held.  Called
// by the encoder (from any thread) once the hardware finished reading the
// buffer (or on its error/stop paths).  Races with stop() are fine: after
// camera->stop() a late release is a no-op.
void PiCameraImpl::release_request(uint32_t buffer_index) {
    std::shared_ptr<libcamera::Camera> cam;
    Request* request = nullptr;
    {
        std::lock_guard<std::mutex> lock(release_mutex_);
        if (!running.load() || buffer_index >= held_requests_.size()) return;
        request = held_requests_[buffer_index];
        cam = camera;
    }
    if (!request || !cam) return;
    request->reuse(Request::ReuseFlag::ReuseBuffers);
    if (cam->queueRequest(request) < 0) {
        fprintf(stderr,
                "[CameraInternal] deferred queueRequest failed for req=%p\n",
                request);
    }
}

// ---------------------------------------------------------------------------
// CaptureBackend implementation
// ---------------------------------------------------------------------------
bool PiCameraImpl::init(const CaptureConfig& cfg, std::string* err) {
    width_ = static_cast<int>(cfg.width);
    height_ = static_cast<int>(cfg.height);
    target_fps = static_cast<int>(cfg.fps);
    prefer_dmabuf_ = cfg.prefer_dmabuf;

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
    // Zero-copy needs headroom: the encoder may hold up to 8 buffers in
    // flight via the deferred-requeue contract, and the ISP must always
    // have free requests to keep streaming.
    sc.bufferCount = prefer_dmabuf_ ? 12 : 8;

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

    if (prefer_dmabuf_) {
        // Zero-copy requires every buffer to be a single dma-buf holding
        // exactly the three Y/U/V planes at different offsets (true for
        // the RPi pipeline handler).
        for (const auto& buffer : allocator_->buffers(videoStream)) {
            const auto& planes = buffer->planes();
            const int fd = planes.empty() ? -1 : planes[0].fd.get();
            bool exportable = planes.size() == 3 && fd >= 0;
            for (const auto& plane : planes) {
                if (plane.fd.get() != fd) {
                    exportable = false;
                    break;
                }
            }
            if (!exportable) {
                fprintf(stderr,
                        "[CameraInternal] buffers are not single-fd 3-plane "
                        "dma-bufs — zero-copy disabled, CPU path in use\n");
                prefer_dmabuf_ = false;
                break;
            }
        }
    }

    uint32_t buffer_index = 0;
    for (const auto& buffer : allocator_->buffers(videoStream)) {
        FrameBuffer* framebuffer = buffer.get();
        MappedRequest mapped;
        mapped.framebuffer = framebuffer;
        mapped.buffer_index = buffer_index;
        mapped.total_size = 0;

        // Compute total buffer size and mmap the whole dma-buf once.
        // libcamera planes share one fd with different offsets; mmap offset
        // must be page aligned, so we map from offset 0 and access planes by
        // base_addr + plane.offset.
        size_t total_length = 0;
        for (const auto& plane : framebuffer->planes()) {
            total_length = std::max(total_length,
                                    static_cast<size_t>(plane.offset + plane.length));
            mapped.total_size += plane.length;
        }

        const auto& planes = framebuffer->planes();
        int fd = planes.empty() ? -1 : planes[0].fd.get();
        if (fd < 0 || total_length == 0) {
            if (err) *err = "invalid camera buffer fd or zero-length buffer";
            release_resources();
            return false;
        }
        mapped.dma_fd = fd;

        void* addr = mmap(nullptr, total_length, PROT_READ, MAP_SHARED, fd, 0);
        if (addr == MAP_FAILED) {
            if (err) *err = "mmap failed for camera buffer";
            release_resources();
            return false;
        }
        mapped.base_addr = addr;
        mapped.mapped_size = total_length;
        // The CPU staging buffer is unused on the zero-copy path (the
        // init-time validation above guarantees the dmabuf frame shape).
        if (!prefer_dmabuf_) {
            mapped.contiguous.resize(mapped.total_size);
        }

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
            g_request_to_instance[mapped.request] = shared_from_this();
        }
        {
            std::lock_guard<std::mutex> lock(release_mutex_);
            held_requests_.push_back(request.get());
        }
        requests.push_back(std::move(request));
        buffer_index++;
    }
    return true;
}

bool PiCameraImpl::start(CaptureFrameCallback cb, std::string* err) {
    (void)err;
    {
        std::lock_guard<std::mutex> lock(state_mutex_);
        if (running.load()) return false;
        capture_cb_ = std::move(cb);
        start_time_ = std::chrono::steady_clock::now();
    }
    camera->requestCompleted.connect(request_completed_slot);

    ControlList controls;
    if (target_fps <= 0) {
        target_fps = 30;
    }
    int64_t frame_duration = 1000000 / target_fps;
    controls.set(controls::FrameDurationLimits, {frame_duration, frame_duration});

    int ret = camera->start(&controls);
    if (ret < 0) {
        std::lock_guard<std::mutex> lock(state_mutex_);
        capture_cb_ = nullptr;
        return false;
    }

    running.store(true);
    for (auto& r : requests) {
        camera->queueRequest(r.get());
    }
    return true;
}

void PiCameraImpl::stop() {
    {
        std::lock_guard<std::mutex> lock(state_mutex_);
        if (!running.load()) return;
        running.store(false);
        capture_cb_ = nullptr;
    }
    if (camera) {
        camera->stop();
        camera->requestCompleted.disconnect(request_completed_slot);
    }
    // Wait for any in-flight completion callback to finish before returning,
    // so that callers can safely release resources afterwards.
    std::unique_lock<std::mutex> lock(completion_mutex_);
    completion_cv_.wait(lock, [&] { return callbacks_in_flight_.load() == 0; });
}

bool PiCameraImpl::isRunning() const {
    return running.load();
}

// ---------------------------------------------------------------------------
// Factory for CaptureBackend (libcamera)
// ---------------------------------------------------------------------------
std::shared_ptr<CaptureBackend> create_libcamera_capture_backend(const CaptureConfig& cfg) {
    (void)cfg;
    return PiCameraImpl::create();
}
