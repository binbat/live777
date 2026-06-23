#include "include/capture_backend.h"
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <linux/videodev2.h>
#include <string>
#include <vector>
#include <thread>
#include <atomic>
#include <utility>

#define BUFFER_COUNT 16

struct Buffer {
    void* start;
    size_t length;
    int dbuf_fd; // The DMA-BUF File Descriptor
};

class V4L2CaptureImpl : public CaptureBackend {
public:
    int fd = -1;
    std::string error_msg;
    uint32_t width = 0, height = 0, fps = 0;
    std::vector<Buffer> buffers;
    std::thread cap_thread;
    std::atomic<bool> running{false};

    CaptureFrameCallback capture_cb_;
    uint64_t seq_ = 0;
    bool prefer_dmabuf = false;

    ~V4L2CaptureImpl() {
        stop();
        if (fd >= 0) {
            close(fd);
            fd = -1;
        }
    }

    void release_resources() {
        if (fd >= 0) {
            enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            ioctl(fd, VIDIOC_STREAMOFF, &type);

            for (auto& buf : buffers) {
                if (buf.start) munmap(buf.start, buf.length);
                if (buf.dbuf_fd >= 0) close(buf.dbuf_fd);
            }
            buffers.clear();

            close(fd);
            fd = -1;
        }
        capture_cb_ = nullptr;
    }

    void stop() {
        running = false;
        if (cap_thread.joinable()) cap_thread.join();
        release_resources();
    }

    // --- CaptureBackend overrides ---
    bool init(const CaptureConfig& cfg, std::string* err) override;
    bool start(CaptureFrameCallback cb, std::string* err) override;
    bool isRunning() const override;
};

// ---------------------------------------------------------------------------
// CaptureBackend implementation
// ---------------------------------------------------------------------------
bool V4L2CaptureImpl::init(const CaptureConfig& cfg, std::string* err) {
    width = cfg.width;
    height = cfg.height;
    fps = cfg.fps;
    prefer_dmabuf = cfg.prefer_dmabuf;

    fd = ::open(cfg.device.c_str(), O_RDWR);
    if (fd < 0) {
        if (err) *err = "Failed to open device: " + std::string(strerror(errno));
        return false;
    }

    struct v4l2_format fmt = {};
    fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    fmt.fmt.pix.width = cfg.width;
    fmt.fmt.pix.height = cfg.height;
    fmt.fmt.pix.pixelformat = V4L2_PIX_FMT_YUYV;
    fmt.fmt.pix.field = V4L2_FIELD_ANY;

    if (ioctl(fd, VIDIOC_S_FMT, &fmt) < 0) {
        if (err) *err = "S_FMT failed";
        release_resources();
        return false;
    }

    struct v4l2_requestbuffers req = {};
    req.count = BUFFER_COUNT;
    req.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    req.memory = V4L2_MEMORY_MMAP;

    if (ioctl(fd, VIDIOC_REQBUFS, &req) < 0) {
        if (err) *err = "REQBUFS failed";
        release_resources();
        return false;
    }

    for (uint32_t i = 0; i < req.count; ++i) {
        struct v4l2_buffer buf = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;

        if (ioctl(fd, VIDIOC_QUERYBUF, &buf) < 0) {
            if (err) *err = "QUERYBUF failed";
            release_resources();
            return false;
        }

        Buffer b;
        b.length = buf.length;
        b.start = mmap(NULL, buf.length, PROT_READ | PROT_WRITE, MAP_SHARED, fd, buf.m.offset);
        if (b.start == MAP_FAILED) {
            if (err) *err = "mmap failed";
            release_resources();
            return false;
        }

        struct v4l2_exportbuffer expbuf = {};
        expbuf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        expbuf.index = i;
        if (ioctl(fd, VIDIOC_EXPBUF, &expbuf) < 0) {
            b.dbuf_fd = -1;
        } else {
            b.dbuf_fd = expbuf.fd;
        }

        buffers.push_back(b);
        if (ioctl(fd, VIDIOC_QBUF, &buf) < 0) {
            if (err) *err = "QBUF failed";
            release_resources();
            return false;
        }
    }

    return true;
}

// Forward declaration — defined before extern "C" block
static void capture_loop(V4L2CaptureImpl* impl);

bool V4L2CaptureImpl::start(CaptureFrameCallback cb, std::string* err) {
    (void)err;
    capture_cb_ = std::move(cb);
    if (running) return true;
    running = true;
    cap_thread = std::thread(capture_loop, this);
    return true;
}

bool V4L2CaptureImpl::isRunning() const {
    return running.load();
}

// ---- capture_loop (C++ linkage, private helper) ----

static void capture_loop(V4L2CaptureImpl* impl) {
    enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    ioctl(impl->fd, VIDIOC_STREAMON, &type);

    while (impl->running) {
        struct v4l2_buffer buf = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;

        if (ioctl(impl->fd, VIDIOC_DQBUF, &buf) < 0) continue;

        uint64_t ts = (uint64_t)buf.timestamp.tv_sec * 1000000 + buf.timestamp.tv_usec;

        // Dispatch to CaptureBackend callback
        if (impl->capture_cb_) {
            RawFrame f{};
            f.format = RawPixelFormat::Yuyv422;
            f.width = impl->width;
            f.height = impl->height;
            f.pts_us = ts;
            f.seq = ++impl->seq_;
            f.plane_count = 1;

            // Only emit DmaBuf when prefer_dmabuf is true AND export succeeded.
            // Otherwise always emit Cpu (the RDK encoder CPU path expects YUYV).
            bool use_dmabuf = impl->prefer_dmabuf
                              && impl->buffers[buf.index].dbuf_fd >= 0;
            if (use_dmabuf) {
                f.kind = BufferKind::DmaBuf;
                f.planes[0] = {nullptr, 0, buf.bytesused,
                               impl->buffers[buf.index].dbuf_fd, 0};
            } else {
                f.kind = BufferKind::Cpu;
                // YUYV422 stride = width * 2 bytes
                f.planes[0] = {
                    static_cast<const uint8_t*>(impl->buffers[buf.index].start),
                    impl->width * 2, buf.bytesused, -1, 0};
            }
            impl->capture_cb_(f);
        }

        ioctl(impl->fd, VIDIOC_QBUF, &buf);
    }
}

// ---------------------------------------------------------------------------
// Factory for CaptureBackend (RDK X5 V4L2)
// ---------------------------------------------------------------------------
std::shared_ptr<CaptureBackend> create_rdk_v4l2_capture_backend(const CaptureConfig& cfg) {
    (void)cfg;
    return std::make_shared<V4L2CaptureImpl>();
}
