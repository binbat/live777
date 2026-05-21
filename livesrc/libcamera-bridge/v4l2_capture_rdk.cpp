#include "v4l2_capture.h"
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

#define BUFFER_COUNT 16

struct Buffer {
    void* start;
    size_t length;
    int dbuf_fd; // The DMA-BUF File Descriptor
};

class V4L2CaptureImpl {
public:
    int fd = -1;
    std::string error_msg;
    uint32_t width, height, fps;
    std::vector<Buffer> buffers;
    std::thread cap_thread;
    std::atomic<bool> running{false};

    V4L2FrameCallback callback = nullptr;
    V4L2FDFrameCallback fd_callback = nullptr;
    void* user_data = nullptr;

    ~V4L2CaptureImpl() {
        stop();
        if (fd >= 0) close(fd);
    }

    void stop() {
        running = false;
        if (cap_thread.joinable()) cap_thread.join();

        if (fd >= 0) {
            enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            ioctl(fd, VIDIOC_STREAMOFF, &type);

            for (auto& buf : buffers) {
                if (buf.start) munmap(buf.start, buf.length);
                if (buf.dbuf_fd >= 0) close(buf.dbuf_fd);
            }
            buffers.clear();
        }
    }
};

extern "C" {

V4L2CaptureHandle v4l2cap_create() {
    return new V4L2CaptureImpl();
}

void v4l2cap_destroy(V4L2CaptureHandle handle) {
    delete static_cast<V4L2CaptureImpl*>(handle);
}

bool v4l2cap_init(V4L2CaptureHandle handle, const V4L2CaptureParams* params) {
    auto* impl = static_cast<V4L2CaptureImpl*>(handle);
    impl->fd = open(params->device, O_RDWR);
    if (impl->fd < 0) {
        impl->error_msg = "Failed to open device: " + std::string(strerror(errno));
        return false;
    }

    impl->width = params->width;
    impl->height = params->height;
    impl->fps = params->fps;

    // 1. Set Format
    struct v4l2_format fmt = {};
    fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    fmt.fmt.pix.width = params->width;
    fmt.fmt.pix.height = params->height;
    fmt.fmt.pix.pixelformat = V4L2_PIX_FMT_YUYV; // Typically USB cameras
    fmt.fmt.pix.field = V4L2_FIELD_ANY;

    if (ioctl(impl->fd, VIDIOC_S_FMT, &fmt) < 0) {
        impl->error_msg = "S_FMT failed";
        return false;
    }

    // 2. Request Buffers
    struct v4l2_requestbuffers req = {};
    req.count = BUFFER_COUNT;
    req.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    req.memory = V4L2_MEMORY_MMAP;

    if (ioctl(impl->fd, VIDIOC_REQBUFS, &req) < 0) {
        impl->error_msg = "REQBUFS failed";
        return false;
    }

    // 3. Map Buffers & EXPORT DMA-BUF FDs
    for (uint32_t i = 0; i < req.count; ++i) {
        struct v4l2_buffer buf = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;

        if (ioctl(impl->fd, VIDIOC_QUERYBUF, &buf) < 0) return false;

        Buffer b;
        b.length = buf.length;
        b.start = mmap(NULL, buf.length, PROT_READ | PROT_WRITE, MAP_SHARED, impl->fd, buf.m.offset);
        
        // --- THE ZERO-COPY KEY: Export FD ---
        struct v4l2_exportbuffer expbuf = {};
        expbuf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        expbuf.index = i;
        if (ioctl(impl->fd, VIDIOC_EXPBUF, &expbuf) < 0) {
            b.dbuf_fd = -1; // Fallback to copy if export fails
        } else {
            b.dbuf_fd = expbuf.fd;
        }

        impl->buffers.push_back(b);
        ioctl(impl->fd, VIDIOC_QBUF, &buf);
    }

    return true;
}

static void capture_loop(V4L2CaptureImpl* impl) {
    enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    ioctl(impl->fd, VIDIOC_STREAMON, &type);

    while (impl->running) {
        struct v4l2_buffer buf = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;

        if (ioctl(impl->fd, VIDIOC_DQBUF, &buf) < 0) continue;

        uint64_t ts = (uint64_t)buf.timestamp.tv_sec * 1000000 + buf.timestamp.tv_usec;

        // Path A: Low Latency / Zero Copy (RDK Path)
        if (impl->fd_callback && impl->buffers[buf.index].dbuf_fd >= 0) {
            impl->fd_callback(impl->buffers[buf.index].dbuf_fd, buf.bytesused, ts, impl->user_data);
        } 
        // Path B: Legacy CPU Copy (Fallback)
        else if (impl->callback) {
            impl->callback((uint8_t*)impl->buffers[buf.index].start, buf.bytesused, ts, impl->user_data);
        }

        ioctl(impl->fd, VIDIOC_QBUF, &buf);
    }
}

bool v4l2cap_start(V4L2CaptureHandle handle) {
    auto* impl = static_cast<V4L2CaptureImpl*>(handle);
    if (impl->running) return true;
    impl->running = true;
    impl->cap_thread = std::thread(capture_loop, impl);
    return true;
}

void v4l2cap_set_callback(V4L2CaptureHandle handle, V4L2FrameCallback callback, void* user_data) {
    auto* impl = static_cast<V4L2CaptureImpl*>(handle);
    impl->callback = callback;
    impl->user_data = user_data;
}

void v4l2cap_set_fd_callback(V4L2CaptureHandle handle, V4L2FDFrameCallback callback, void* user_data) {
    auto* impl = static_cast<V4L2CaptureImpl*>(handle);
    impl->fd_callback = callback;
    impl->user_data = user_data;
}

void v4l2cap_stop(V4L2CaptureHandle handle) {
    static_cast<V4L2CaptureImpl*>(handle)->stop();
}

bool v4l2cap_is_running(V4L2CaptureHandle handle) {
    return static_cast<V4L2CaptureImpl*>(handle)->running;
}

const char* v4l2cap_get_error(V4L2CaptureHandle handle) {
    return static_cast<V4L2CaptureImpl*>(handle)->error_msg.c_str();
}

} // extern "C"
