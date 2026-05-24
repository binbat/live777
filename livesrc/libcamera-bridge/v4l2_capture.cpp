#include "v4l2_capture.h"
#include "include/capture_backend.h"
#include <cerrno>
#include <fcntl.h>
#include <sys/select.h>
#include <sys/time.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <linux/videodev2.h>
#include <cstring>
#include <cstdio>
#include <string>
#include <vector>
#include <thread>
#include <atomic>
#include <chrono>
#include <utility>

struct V4L2CaptureImpl : public CaptureBackend {
    int fd = -1;
    int width = 0;
    int height = 0;
    int fps = 0;
    bool use_mjpeg = false;
    std::atomic<bool> running{false};

    V4L2FrameCallback callback = nullptr;
    void* user_data = nullptr;
    std::string error_msg;
    std::atomic<bool> thread_alive{false};

    CaptureFrameCallback capture_cb_;
    uint64_t seq_ = 0;

    struct Buffer {
        void* start;
        size_t length;
    };
    std::vector<Buffer> buffers;

    // Pre-allocated YUV420P conversion buffer
    std::vector<uint8_t> yuv420_buf;

    std::thread capture_thread;

    void capture_loop();
    bool is_healthy() const { return running.load() && thread_alive.load(); }
    void yuyv_to_yuv420p(const uint8_t* src, uint8_t* dst, int w, int h);

    // --- CaptureBackend overrides ---
    bool init(const CaptureConfig& cfg, std::string* err) override;
    bool start(CaptureFrameCallback cb, std::string* err) override;
    void stop() override;
    bool isRunning() const override;
};

// --- YUYV to YUV420P conversion ---
// YUYV: 2 pixels = 4 bytes [Y0 U0 Y1 V0]
// YUV420P: Y plane (w*h) + U plane (w*h/4) + V plane (w*h/4)
void V4L2CaptureImpl::yuyv_to_yuv420p(const uint8_t* src, uint8_t* dst, int w, int h) {
    uint8_t* y_plane = dst;
    uint8_t* u_plane = dst + w * h;
    uint8_t* v_plane = dst + w * h + (w * h / 4);

    for (int row = 0; row < h; row++) {
        const uint8_t* row_src = src + row * w * 2;
        uint8_t* y_row = y_plane + row * w;

        for (int col = 0; col < w; col += 2) {
            int idx = col * 2;
            y_row[col]     = row_src[idx + 0]; // Y0
            y_row[col + 1] = row_src[idx + 2]; // Y1

            // Subsample U and V: every 2x2 block shares one U and V
            if (row % 2 == 0) {
                int uv_col = col / 2;
                int uv_row = row / 2;
                u_plane[uv_row * (w / 2) + uv_col] = row_src[idx + 1]; // U
                v_plane[uv_row * (w / 2) + uv_col] = row_src[idx + 3]; // V
            }
        }
    }
}

void V4L2CaptureImpl::capture_loop() {
    fprintf(stderr, "[V4L2Capture] Capture thread started\n");
    thread_alive.store(true);

    while (running.load()) {
        fd_set fds;
        FD_ZERO(&fds);
        FD_SET(fd, &fds);

        struct timeval tv;
        tv.tv_sec = 2;
        tv.tv_usec = 0;

        int r = select(fd + 1, &fds, NULL, NULL, &tv);
        if (r <= 0) continue;

        struct v4l2_buffer buf = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;

        if (ioctl(fd, VIDIOC_DQBUF, &buf) < 0) {
            if (errno == EAGAIN) continue;
            fprintf(stderr, "[V4L2Capture] DQBUF failed: %s\n", strerror(errno));
            break;
        }

        uint64_t ts_us = (uint64_t)buf.timestamp.tv_sec * 1000000 + buf.timestamp.tv_usec;

        // Convert YUYV → YUV420P when either callback needs it
        bool need_yuv420 = !use_mjpeg && (callback || capture_cb_);
        if (need_yuv420) {
            yuyv_to_yuv420p(
                static_cast<uint8_t*>(buffers[buf.index].start),
                yuv420_buf.data(), width, height
            );
        }

        // Legacy callback
        if (callback) {
            if (!use_mjpeg) {
                callback(yuv420_buf.data(), yuv420_buf.size(), ts_us, user_data);
            } else {
                callback(
                    static_cast<uint8_t*>(buffers[buf.index].start),
                    buf.bytesused, ts_us, user_data
                );
            }
        }

        // Dispatch to CaptureBackend callback (new path)
        if (capture_cb_) {
            RawFrame f{};
            f.kind = BufferKind::Cpu;
            f.format = use_mjpeg ? RawPixelFormat::Mjpeg : RawPixelFormat::Yuv420p;
            f.width = static_cast<uint32_t>(width);
            f.height = static_cast<uint32_t>(height);
            f.pts_us = ts_us;
            f.seq = ++seq_;
            f.plane_count = 1;
            if (!use_mjpeg) {
                f.planes[0] = {
                    yuv420_buf.data(),
                    static_cast<uint32_t>(width),
                    yuv420_buf.size(),
                    -1,
                    0
                };
            } else {
                f.planes[0] = {
                    static_cast<const uint8_t*>(buffers[buf.index].start),
                    static_cast<uint32_t>(width),
                    buf.bytesused,
                    -1,
                    0
                };
            }
            capture_cb_(f);
        }

        // Re-queue buffer
        if (ioctl(fd, VIDIOC_QBUF, &buf) < 0) {
            fprintf(stderr, "[V4L2Capture] QBUF failed: %s\n", strerror(errno));
        }
    }

    thread_alive.store(false);
    fprintf(stderr, "[V4L2Capture] Capture thread stopped\n");
}

// ---------------------------------------------------------------------------
// CaptureBackend implementation
// ---------------------------------------------------------------------------
bool V4L2CaptureImpl::init(const CaptureConfig& cfg, std::string* err) {
    width = static_cast<int>(cfg.width);
    height = static_cast<int>(cfg.height);
    fps = static_cast<int>(cfg.fps);

    const char* dev = cfg.device.c_str();
    fd = ::open(dev, O_RDWR);
    if (fd < 0) {
        if (err) *err = std::string("Failed to open ") + dev + ": " + strerror(errno);
        return false;
    }

    struct v4l2_capability cap = {};
    if (ioctl(fd, VIDIOC_QUERYCAP, &cap) < 0) {
        if (err) *err = std::string("QUERYCAP failed: ") + strerror(errno);
        close(fd); fd = -1; return false;
    }

    uint32_t caps = (cap.capabilities & V4L2_CAP_DEVICE_CAPS) ? cap.device_caps : cap.capabilities;
    if (!(caps & (V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_VIDEO_CAPTURE_MPLANE))) {
        if (err) *err = "Device does not support video capture";
        close(fd); fd = -1; return false;
    }

    struct v4l2_format fmt = {};
    fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    if (ioctl(fd, VIDIOC_G_FMT, &fmt) < 0) {
        fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    }
    fmt.fmt.pix.width = static_cast<__u32>(cfg.width);
    fmt.fmt.pix.height = static_cast<__u32>(cfg.height);
    // NOTE: cfg.pixel_format is intentionally ignored for now.
    // This backend always requests YUYV and converts to YUV420P internally.
    // Pixel format selection from CaptureConfig will be added in a later cleanup.
    fmt.fmt.pix.pixelformat = V4L2_PIX_FMT_YUYV;
    fmt.fmt.pix.field = V4L2_FIELD_ANY;

    if (ioctl(fd, VIDIOC_S_FMT, &fmt) < 0) {
        if (err) *err = std::string("S_FMT failed: ") + strerror(errno);
        close(fd); fd = -1; return false;
    }

    width = fmt.fmt.pix.width;
    height = fmt.fmt.pix.height;

    struct v4l2_streamparm parm = {};
    parm.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    parm.parm.capture.timeperframe.numerator = 1;
    parm.parm.capture.timeperframe.denominator = static_cast<__u32>(fps);
    ioctl(fd, VIDIOC_S_PARM, &parm);

    yuv420_buf.resize(static_cast<size_t>(width) * static_cast<size_t>(height) * 3 / 2);

    struct v4l2_requestbuffers req = {};
    req.count = 16;
    req.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    req.memory = V4L2_MEMORY_MMAP;

    if (ioctl(fd, VIDIOC_REQBUFS, &req) < 0) {
        if (err) *err = std::string("REQBUFS failed: ") + strerror(errno);
        close(fd); fd = -1; return false;
    }

    for (unsigned int i = 0; i < req.count; i++) {
        struct v4l2_buffer buf = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;

        if (ioctl(fd, VIDIOC_QUERYBUF, &buf) < 0) {
            if (err) *err = "QUERYBUF failed";
            return false;
        }

        void* start = mmap(NULL, buf.length, PROT_READ | PROT_WRITE,
                           MAP_SHARED, fd, buf.m.offset);
        if (start == MAP_FAILED) {
            if (err) *err = "mmap failed";
            return false;
        }
        buffers.push_back({start, buf.length});
    }
    return true;
}

bool V4L2CaptureImpl::start(CaptureFrameCallback cb, std::string* err) {
    (void)err;
    capture_cb_ = std::move(cb);

    for (unsigned int i = 0; i < buffers.size(); i++) {
        struct v4l2_buffer buf = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        if (ioctl(fd, VIDIOC_QBUF, &buf) < 0) return false;
    }

    enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    if (ioctl(fd, VIDIOC_STREAMON, &type) < 0) return false;

    running.store(true);
    capture_thread = std::thread(&V4L2CaptureImpl::capture_loop, this);
    return true;
}

void V4L2CaptureImpl::stop() {
    running.store(false);
    if (capture_thread.joinable()) capture_thread.join();

    if (fd >= 0) {
        enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        ioctl(fd, VIDIOC_STREAMOFF, &type);

        for (auto& buf : buffers) {
            if (buf.start && buf.start != MAP_FAILED) munmap(buf.start, buf.length);
        }
        buffers.clear();
        close(fd);
        fd = -1;
    }
    capture_cb_ = nullptr;
}

bool V4L2CaptureImpl::isRunning() const {
    return running.load() && thread_alive.load();
}

// --- C API Implementation ---
extern "C" {

V4L2CaptureHandle v4l2cap_create() {
    return new V4L2CaptureImpl();
}

void v4l2cap_destroy(V4L2CaptureHandle handle) {
    if (!handle) return;
    auto* impl = static_cast<V4L2CaptureImpl*>(handle);
    v4l2cap_stop(handle);
    delete impl;
}

bool v4l2cap_init(V4L2CaptureHandle handle, const V4L2CaptureParams* params) {
    if (!handle || !params) return false;
    auto* impl = static_cast<V4L2CaptureImpl*>(handle);

    impl->width = params->width;
    impl->height = params->height;
    impl->fps = params->fps;
    impl->use_mjpeg = (params->input_format == 1);

    const char* dev = params->device ? params->device : "/dev/video2";
    fprintf(stderr, "[V4L2Capture] Opening %s (%dx%d @ %dfps, format=%s)\n",
            dev, impl->width, impl->height, impl->fps,
            impl->use_mjpeg ? "MJPEG" : "YUYV");

    impl->fd = open(dev, O_RDWR);
    if (impl->fd < 0) {
        impl->error_msg = std::string("Failed to open ") + dev + ": " + strerror(errno);
        fprintf(stderr, "[V4L2Capture] %s\n", impl->error_msg.c_str());
        return false;
    }

    // Check capabilities
    struct v4l2_capability cap = {};
    if (ioctl(impl->fd, VIDIOC_QUERYCAP, &cap) < 0) {
        impl->error_msg = "QUERYCAP failed: " + std::string(strerror(errno));
        close(impl->fd); impl->fd = -1; return false;
    }
    
    uint32_t caps = (cap.capabilities & V4L2_CAP_DEVICE_CAPS) ? cap.device_caps : cap.capabilities;
    if (!(caps & (V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_VIDEO_CAPTURE_MPLANE))) {
        impl->error_msg = "Device does not support video capture (caps=" + std::to_string(caps) + ")";
        close(impl->fd); impl->fd = -1; return false;
    }

    // Set format
    struct v4l2_format fmt = {};
    fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    
    // Attempt G_FMT first
    if (ioctl(impl->fd, VIDIOC_G_FMT, &fmt) < 0) {
        fprintf(stderr, "[V4L2Capture] Warning: G_FMT failed, using defaults\n");
        fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    }

    fmt.fmt.pix.width = impl->width;
    fmt.fmt.pix.height = impl->height;
    fmt.fmt.pix.pixelformat = impl->use_mjpeg ? V4L2_PIX_FMT_MJPEG : V4L2_PIX_FMT_YUYV;
    fmt.fmt.pix.field = V4L2_FIELD_ANY; 

    if (ioctl(impl->fd, VIDIOC_S_FMT, &fmt) < 0) {
        impl->error_msg = std::string("S_FMT failed: ") + strerror(errno);
        fprintf(stderr, "[V4L2Capture] %s\n", impl->error_msg.c_str());
        close(impl->fd);
        impl->fd = -1;
        return false;
    }

    // Actual negotiated size might differ
    impl->width = fmt.fmt.pix.width;
    impl->height = fmt.fmt.pix.height;
    fprintf(stderr, "[V4L2Capture] Negotiated: %dx%d\n", impl->width, impl->height);

    // Set frame rate
    struct v4l2_streamparm parm = {};
    parm.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    parm.parm.capture.timeperframe.numerator = 1;
    parm.parm.capture.timeperframe.denominator = impl->fps;
    ioctl(impl->fd, VIDIOC_S_PARM, &parm);

    // Allocate YUV420P conversion buffer
    impl->yuv420_buf.resize(impl->width * impl->height * 3 / 2);

    // Request buffers - Increased to 16 for better jitter tolerance
    struct v4l2_requestbuffers req = {};
    req.count = 16;
    req.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    req.memory = V4L2_MEMORY_MMAP;

    if (ioctl(impl->fd, VIDIOC_REQBUFS, &req) < 0) {
        impl->error_msg = std::string("REQBUFS failed: ") + strerror(errno);
        close(impl->fd);
        impl->fd = -1;
        return false;
    }

    for (unsigned int i = 0; i < req.count; i++) {
        struct v4l2_buffer buf = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;

        if (ioctl(impl->fd, VIDIOC_QUERYBUF, &buf) < 0) {
            impl->error_msg = "QUERYBUF failed";
            return false;
        }

        void* start = mmap(NULL, buf.length, PROT_READ | PROT_WRITE, MAP_SHARED, impl->fd, buf.m.offset);
        if (start == MAP_FAILED) {
            impl->error_msg = "mmap failed";
            return false;
        }
        impl->buffers.push_back({start, buf.length});
    }

    fprintf(stderr, "[V4L2Capture] Init OK, %zu buffers allocated\n", impl->buffers.size());
    return true;
}

bool v4l2cap_start(V4L2CaptureHandle handle) {
    if (!handle) return false;
    auto* impl = static_cast<V4L2CaptureImpl*>(handle);

    // Queue all buffers
    for (unsigned int i = 0; i < impl->buffers.size(); i++) {
        struct v4l2_buffer buf = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        if (ioctl(impl->fd, VIDIOC_QBUF, &buf) < 0) return false;
    }

    // Stream ON
    enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    if (ioctl(impl->fd, VIDIOC_STREAMON, &type) < 0) {
        impl->error_msg = std::string("STREAMON failed: ") + strerror(errno);
        return false;
    }

    impl->running.store(true);
    impl->capture_thread = std::thread(&V4L2CaptureImpl::capture_loop, impl);
    fprintf(stderr, "[V4L2Capture] Streaming started\n");
    return true;
}

void v4l2cap_stop(V4L2CaptureHandle handle) {
    if (!handle) return;
    auto* impl = static_cast<V4L2CaptureImpl*>(handle);

    impl->running.store(false);
    if (impl->capture_thread.joinable()) {
        impl->capture_thread.join();
    }

    if (impl->fd >= 0) {
        enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        ioctl(impl->fd, VIDIOC_STREAMOFF, &type);

        for (auto& buf : impl->buffers) {
            if (buf.start && buf.start != MAP_FAILED) munmap(buf.start, buf.length);
        }
        impl->buffers.clear();

        close(impl->fd);
        impl->fd = -1;
    }
}

void v4l2cap_set_callback(V4L2CaptureHandle handle, V4L2FrameCallback callback, void* user_data) {
    if (!handle) return;
    auto* impl = static_cast<V4L2CaptureImpl*>(handle);
    impl->callback = callback;
    impl->user_data = user_data;
}

bool v4l2cap_is_running(V4L2CaptureHandle handle) {
    if (!handle) return false;
    return static_cast<V4L2CaptureImpl*>(handle)->is_healthy();
}

const char* v4l2cap_get_error(V4L2CaptureHandle handle) {
    if (!handle) return "NULL handle";
    return static_cast<V4L2CaptureImpl*>(handle)->error_msg.c_str();
}

} // extern "C"

// ---------------------------------------------------------------------------
// Factory for CaptureBackend (generic V4L2)
// ---------------------------------------------------------------------------
std::unique_ptr<CaptureBackend> create_capture_backend(const CaptureConfig& cfg) {
    auto impl = std::make_unique<V4L2CaptureImpl>();
    std::string err;
    if (!impl->init(cfg, &err)) {
        fprintf(stderr, "[V4L2Factory] init failed: %s\n", err.c_str());
        return nullptr;
    }
    return impl;
}
