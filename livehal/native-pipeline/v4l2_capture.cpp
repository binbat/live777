//! Generic V4L2 capture backend.
//!
//! Supports both single-planar (V4L2_BUF_TYPE_VIDEO_CAPTURE) and
//! multi-planar (V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE) devices.
//! Auto-detects the correct API during init().

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
    int fmt_pixelformat = 0;
    uint32_t bytesperline_ = 0;  // actual row stride from V4L2 (may differ from width)
    uint32_t num_planes_ = 1;     // number of planes in multiplanar mode
    bool use_mplane = false;
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

// ---------------------------------------------------------------------------
// YUYV to YUV420P conversion
// ---------------------------------------------------------------------------
void V4L2CaptureImpl::yuyv_to_yuv420p(const uint8_t* src, uint8_t* dst, int w, int h) {
    uint8_t* y_plane = dst;
    uint8_t* u_plane = dst + w * h;
    uint8_t* v_plane = dst + w * h + (w * h / 4);

    for (int row = 0; row < h; row++) {
        const uint8_t* row_src = src + row * bytesperline_;
        uint8_t* y_row = y_plane + row * w;

        for (int col = 0; col < w; col += 2) {
            int idx = col * 2;
            y_row[col] = row_src[idx + 0]; // Y0
            if (col + 1 < w) {
                y_row[col + 1] = row_src[idx + 2]; // Y1
            }
            if (row % 2 == 0 && col + 1 < w) {
                int uv_col = col / 2;
                int uv_row = row / 2;
                u_plane[uv_row * (w / 2) + uv_col] = row_src[idx + 1]; // U
                v_plane[uv_row * (w / 2) + uv_col] = row_src[idx + 3]; // V
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Capture loop
// ---------------------------------------------------------------------------
void V4L2CaptureImpl::capture_loop() {
    fprintf(stderr, "[V4L2Capture] Capture thread started (%s)\n",
            use_mplane ? "multiplanar" : "single-planar");
    thread_alive.store(true);

    // Multiplanar: up to 3 planes per buffer.
    struct v4l2_plane planes[3];

    while (running.load()) {
        fd_set fds;
        FD_ZERO(&fds);
        FD_SET(fd, &fds);

        struct timeval tv = {2, 0};
        int r = select(fd + 1, &fds, nullptr, nullptr, &tv);
        if (r <= 0) continue;

        // --- Dequeue buffer ---
        struct v4l2_buffer buf = {};
        if (use_mplane) {
            buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
            buf.memory = V4L2_MEMORY_MMAP;
            buf.m.planes = planes;
            buf.length = num_planes_;
        } else {
            buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            buf.memory = V4L2_MEMORY_MMAP;
        }

        if (ioctl(fd, VIDIOC_DQBUF, &buf) < 0) {
            if (errno == EAGAIN) continue;
            fprintf(stderr, "[V4L2Capture] DQBUF failed: %s\n", strerror(errno));
            break;
        }

        if (buf.index >= buffers.size()) {
            fprintf(stderr, "[V4L2Capture] invalid buffer index %u\n", buf.index);
            break;
        }

        uint64_t ts_us = (uint64_t)buf.timestamp.tv_sec * 1000000
                       + buf.timestamp.tv_usec;

        // Diagnostic: log V4L2 field type on first frame and on change
        {
            static int last_field = -1;
            int cur_field = static_cast<int>(buf.field);
            if (cur_field != last_field) {
                std::fprintf(stderr, "[V4L2Capture] field=%d (V4L2_FIELD_NONE=%d)\n",
                    cur_field, V4L2_FIELD_NONE);
                last_field = cur_field;
            }
        }

        // Bytes used: multiplanar reads from plane[0], single-planar from buf
        uint32_t bytesused = use_mplane
            ? buf.m.planes[0].bytesused
            : buf.bytesused;

        // NV12 output from ISP: single-plane multiplanar buffer with Y+UV
        // laid out as a single contiguous NV12 buffer.
        bool is_nv12 = (fmt_pixelformat == V4L2_PIX_FMT_NV12
                     || fmt_pixelformat == V4L2_PIX_FMT_NV21);

        // Convert YUYV → YUV420P when needed
        bool need_yuv420 = !use_mjpeg && !is_nv12 && capture_cb_;
        if (need_yuv420) {
            yuyv_to_yuv420p(
                static_cast<uint8_t*>(buffers[buf.index].start),
                yuv420_buf.data(), width, height
            );
        }

        // Dispatch to callback
        if (capture_cb_) {
            RawFrame f{};
            f.kind = BufferKind::Cpu;
            f.width = static_cast<uint32_t>(width);
            f.height = static_cast<uint32_t>(height);
            f.pts_us = ts_us;
            f.seq = ++seq_;

            if (use_mjpeg) {
                f.format = RawPixelFormat::Mjpeg;
                f.plane_count = 1;
                f.planes[0] = {
                    static_cast<const uint8_t*>(buffers[buf.index].start),
                    static_cast<uint32_t>(width),
                    bytesused,
                    -1, 0
                };
            } else if (is_nv12) {
                // NV12: single contiguous buffer with Y plane + interleaved UV
                f.format = RawPixelFormat::Nv12;
                f.plane_count = 2;
                uint32_t y_size = bytesperline_ * static_cast<uint32_t>(height);
                const uint8_t* uv_start;
                if (num_planes_ > 1 && buf.m.planes[1].m.mem_offset > 0) {
                    // Multi-plane: UV plane may be at a different offset
                    uv_start = static_cast<const uint8_t*>(buffers[buf.index].start)
                        + (buf.m.planes[1].m.mem_offset - buf.m.planes[0].m.mem_offset);
                } else {
                    // Single-plane (common): UV follows Y contiguously
                    uv_start = static_cast<const uint8_t*>(buffers[buf.index].start) + y_size;
                }
                f.planes[0] = {
                    static_cast<const uint8_t*>(buffers[buf.index].start),
                    bytesperline_,
                    y_size,
                    -1, 0
                };
                f.planes[1] = {
                    uv_start,
                    bytesperline_,
                    y_size / 2,
                    -1, 0
                };
            } else {
                // YUYV → YUV420P already converted
                f.format = RawPixelFormat::Yuv420p;
                f.plane_count = 1;
                f.planes[0] = {
                    yuv420_buf.data(),
                    static_cast<uint32_t>(width),
                    yuv420_buf.size(),
                    -1, 0
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

    // --- Query capabilities ---
    struct v4l2_capability cap = {};
    if (ioctl(fd, VIDIOC_QUERYCAP, &cap) < 0) {
        if (err) *err = std::string("QUERYCAP failed: ") + strerror(errno);
        close(fd); fd = -1; return false;
    }

    uint32_t caps = (cap.capabilities & V4L2_CAP_DEVICE_CAPS)
        ? cap.device_caps : cap.capabilities;

    // Prefer multiplanar if available; fall back to single-planar
    if (caps & V4L2_CAP_VIDEO_CAPTURE_MPLANE) {
        use_mplane = true;
    } else if (caps & V4L2_CAP_VIDEO_CAPTURE) {
        use_mplane = false;
    } else {
        if (err) *err = "Device does not support video capture";
        close(fd); fd = -1; return false;
    }

    // --- Try NV12 first (native for MPP), then YUYV, then let driver pick ---
    struct v4l2_format fmt = {};
    fmt.type = use_mplane
        ? V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
        : V4L2_BUF_TYPE_VIDEO_CAPTURE;

    if (use_mplane) {
        fmt.fmt.pix_mp.width = static_cast<__u32>(cfg.width);
        fmt.fmt.pix_mp.height = static_cast<__u32>(cfg.height);
        fmt.fmt.pix_mp.pixelformat = V4L2_PIX_FMT_NV12;
        fmt.fmt.pix_mp.field = V4L2_FIELD_ANY;
    } else {
        fmt.fmt.pix.width = static_cast<__u32>(cfg.width);
        fmt.fmt.pix.height = static_cast<__u32>(cfg.height);
        fmt.fmt.pix.pixelformat = V4L2_PIX_FMT_YUYV;
        fmt.fmt.pix.field = V4L2_FIELD_ANY;
    }

    if (ioctl(fd, VIDIOC_S_FMT, &fmt) < 0) {
        // NV12 may not be supported — try YUYV for multiplanar too
        if (use_mplane) {
            fmt.fmt.pix_mp.pixelformat = V4L2_PIX_FMT_YUYV;
            if (ioctl(fd, VIDIOC_S_FMT, &fmt) < 0) {
                if (err) *err = std::string("S_FMT failed: ") + strerror(errno);
                close(fd); fd = -1; return false;
            }
        } else {
            if (err) *err = std::string("S_FMT failed: ") + strerror(errno);
            close(fd); fd = -1; return false;
        }
    }

    // Read back the actual format
    if (use_mplane) {
        width = static_cast<int>(fmt.fmt.pix_mp.width);
        height = static_cast<int>(fmt.fmt.pix_mp.height);
        fmt_pixelformat = static_cast<int>(fmt.fmt.pix_mp.pixelformat);
        bytesperline_ = fmt.fmt.pix_mp.plane_fmt[0].bytesperline;
        if (bytesperline_ == 0) bytesperline_ = static_cast<uint32_t>(width);
        num_planes_ = fmt.fmt.pix_mp.num_planes > 0
            ? fmt.fmt.pix_mp.num_planes : 1;
    } else {
        width = static_cast<int>(fmt.fmt.pix.width);
        height = static_cast<int>(fmt.fmt.pix.height);
        fmt_pixelformat = static_cast<int>(fmt.fmt.pix.pixelformat);
        bytesperline_ = fmt.fmt.pix.bytesperline;
        if (bytesperline_ == 0) bytesperline_ = static_cast<uint32_t>(width) * 2;
    }

    use_mjpeg = (fmt_pixelformat == V4L2_PIX_FMT_MJPEG);

    fprintf(stderr, "[V4L2Capture] Device %s: %dx%d fmt=0x%x %s\n",
            dev, width, height, fmt_pixelformat,
            use_mplane ? "multiplanar" : "single-planar");

    // --- Set FPS ---
    struct v4l2_streamparm parm = {};
    parm.type = use_mplane
        ? V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
        : V4L2_BUF_TYPE_VIDEO_CAPTURE;
    parm.parm.capture.timeperframe.numerator = 1;
    parm.parm.capture.timeperframe.denominator = static_cast<__u32>(fps);
    ioctl(fd, VIDIOC_S_PARM, &parm);

    // Pre-allocate YUV420P conversion buffer
    yuv420_buf.resize(static_cast<size_t>(width) * static_cast<size_t>(height) * 3 / 2);

    // --- Request buffers ---
    struct v4l2_requestbuffers req = {};
    req.count = 16;
    req.type = use_mplane
        ? V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
        : V4L2_BUF_TYPE_VIDEO_CAPTURE;
    req.memory = V4L2_MEMORY_MMAP;

    if (ioctl(fd, VIDIOC_REQBUFS, &req) < 0) {
        if (err) *err = std::string("REQBUFS failed: ") + strerror(errno);
        close(fd); fd = -1; return false;
    }
    if (req.count < 2) {
        if (err) *err = std::string("REQBUFS returned too few buffers: ")
            + std::to_string(req.count);
        release_resources();
        return false;
    }

    // --- Map buffers ---
    struct v4l2_plane query_planes[3];
    for (unsigned int i = 0; i < req.count; i++) {
        struct v4l2_buffer buf = {};
        buf.type = use_mplane
            ? V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
            : V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        if (use_mplane) {
            buf.m.planes = query_planes;
            buf.length = 1;
        }

        if (ioctl(fd, VIDIOC_QUERYBUF, &buf) < 0) {
            if (err) *err = "QUERYBUF failed";
            release_resources();
            return false;
        }

        uint32_t offset = use_mplane
            ? buf.m.planes[0].m.mem_offset
            : buf.m.offset;
        uint32_t length = use_mplane
            ? buf.m.planes[0].length
            : buf.length;

        void* start = mmap(nullptr, length, PROT_READ | PROT_WRITE,
                           MAP_SHARED, fd, offset);
        if (start == MAP_FAILED) {
            if (err) *err = "mmap failed";
            release_resources();
            return false;
        }
        buffers.push_back({start, length});
    }
    return true;
}

// ---------------------------------------------------------------------------
// Start / Stop
// ---------------------------------------------------------------------------
bool V4L2CaptureImpl::start(CaptureFrameCallback cb, std::string* err) {
    if (running.load() || capture_thread.joinable()) {
        if (err) *err = "capture already running";
        return false;
    }
    capture_cb_ = std::move(cb);

    struct v4l2_plane start_planes[3];
    for (unsigned int i = 0; i < buffers.size(); i++) {
        struct v4l2_buffer buf = {};
        buf.type = use_mplane
            ? V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
            : V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        if (use_mplane) {
            buf.m.planes = start_planes;
            buf.length = num_planes_;
        }
        if (ioctl(fd, VIDIOC_QBUF, &buf) < 0) {
            if (err) *err = std::string("QBUF failed: ") + strerror(errno);
            return false;
        }
    }

    enum v4l2_buf_type type = use_mplane
        ? V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
        : V4L2_BUF_TYPE_VIDEO_CAPTURE;
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
        enum v4l2_buf_type type = use_mplane
            ? V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE
            : V4L2_BUF_TYPE_VIDEO_CAPTURE;
        ioctl(fd, VIDIOC_STREAMOFF, &type);

        for (auto& buf : buffers) {
            if (buf.start && buf.start != MAP_FAILED) {
                munmap(buf.start, buf.length);
            }
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
// Factory
// ---------------------------------------------------------------------------
std::shared_ptr<CaptureBackend> create_v4l2_capture_backend(const CaptureConfig& cfg) {
    (void)cfg;
    return std::make_shared<V4L2CaptureImpl>();
}
