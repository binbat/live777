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
    void release_resources();
    bool is_healthy() const { return running.load() && thread_alive.load(); }
    void yuyv_to_yuv420p(const uint8_t* src, uint8_t* dst, int w, int h);

    // --- CaptureBackend overrides ---
    ~V4L2CaptureImpl() override { stop(); }
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
            y_row[col] = row_src[idx + 0]; // Y0
            if (col + 1 < w) {
                y_row[col + 1] = row_src[idx + 2]; // Y1
            }

            // Subsample U and V: every 2x2 block shares one U and V
            if (row % 2 == 0 && col + 1 < w) {
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

        if (buf.index >= buffers.size()) {
            fprintf(stderr, "[V4L2Capture] invalid buffer index %u\n", buf.index);
            break;
        }

        uint64_t ts_us = (uint64_t)buf.timestamp.tv_sec * 1000000 + buf.timestamp.tv_usec;

        // Convert YUYV → YUV420P when CaptureBackend callback is set
        bool need_yuv420 = !use_mjpeg && capture_cb_;
        if (need_yuv420) {
            yuyv_to_yuv420p(
                static_cast<uint8_t*>(buffers[buf.index].start),
                yuv420_buf.data(), width, height
            );
        }

        // Dispatch to CaptureBackend callback
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
    fd = ::open(dev, O_RDWR | O_CLOEXEC);
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
    if (req.count < 2) {
        if (err) *err = std::string("REQBUFS returned too few buffers: ") + std::to_string(req.count);
        release_resources();
        return false;
    }

    for (unsigned int i = 0; i < req.count; i++) {
        struct v4l2_buffer buf = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;

        if (ioctl(fd, VIDIOC_QUERYBUF, &buf) < 0) {
            if (err) *err = "QUERYBUF failed";
            release_resources();
            return false;
        }

        void* start = mmap(NULL, buf.length, PROT_READ | PROT_WRITE,
                           MAP_SHARED, fd, buf.m.offset);
        if (start == MAP_FAILED) {
            if (err) *err = "mmap failed";
            release_resources();
            return false;
        }
        buffers.push_back({start, buf.length});
    }
    return true;
}

bool V4L2CaptureImpl::start(CaptureFrameCallback cb, std::string* err) {
    if (running.load() || capture_thread.joinable()) {
        if (err) *err = "capture already running";
        return false;
    }
    capture_cb_ = std::move(cb);

    for (unsigned int i = 0; i < buffers.size(); i++) {
        struct v4l2_buffer buf = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        if (ioctl(fd, VIDIOC_QBUF, &buf) < 0) {
            if (err) *err = std::string("QBUF failed: ") + strerror(errno);
            return false;
        }
    }

    enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    if (ioctl(fd, VIDIOC_STREAMON, &type) < 0) {
        if (err) *err = std::string("STREAMON failed: ") + strerror(errno);
        return false;
    }

    running.store(true);
    capture_thread = std::thread(&V4L2CaptureImpl::capture_loop, this);
    return true;
}

void V4L2CaptureImpl::release_resources() {
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

void V4L2CaptureImpl::stop() {
    running.store(false);
    if (capture_thread.joinable()) capture_thread.join();
    release_resources();
}

bool V4L2CaptureImpl::isRunning() const {
    return running.load() && thread_alive.load();
}

// ---------------------------------------------------------------------------
// Factory for CaptureBackend (generic V4L2)
// ---------------------------------------------------------------------------
std::shared_ptr<CaptureBackend> create_v4l2_capture_backend(const CaptureConfig& cfg) {
    (void)cfg;
    return std::make_shared<V4L2CaptureImpl>();
}
