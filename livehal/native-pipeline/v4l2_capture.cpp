#include "include/capture_backend.h"
#include "include/latency_stats.h"

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <linux/videodev2.h>
#include <mutex>
#include <string>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/select.h>
#include <sys/time.h>
#include <thread>
#include <unistd.h>
#include <utility>
#include <vector>

namespace {

constexpr uint32_t kBufferCount = 16;

const char* fourcc_to_string(uint32_t fourcc, char (&text)[5]) {
    text[0] = static_cast<char>(fourcc & 0xff);
    text[1] = static_cast<char>((fourcc >> 8) & 0xff);
    text[2] = static_cast<char>((fourcc >> 16) & 0xff);
    text[3] = static_cast<char>((fourcc >> 24) & 0xff);
    text[4] = '\0';
    return text;
}

std::vector<uint32_t> requested_fourccs(RawPixelFormat format) {
    switch (format) {
    case RawPixelFormat::Yuyv422:
        return {V4L2_PIX_FMT_YUYV};
    case RawPixelFormat::Nv12:
        return {V4L2_PIX_FMT_NV12, V4L2_PIX_FMT_NV12M};
    case RawPixelFormat::Yuv420p:
        return {V4L2_PIX_FMT_YUV420, V4L2_PIX_FMT_YUV420M};
    case RawPixelFormat::Rgb888:
        return {V4L2_PIX_FMT_RGB24};
    case RawPixelFormat::Uyvy422:
        return {V4L2_PIX_FMT_UYVY};
    }
    return {};
}

bool is_mplane_type(v4l2_buf_type type) {
    return type == V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
}

} // namespace

struct V4L2CaptureImpl : public CaptureBackend {
    struct IntegrityTracker {
        uint64_t bad_frames = 0;
        uint64_t seq_gaps = 0;
        uint64_t seq_gap_frames = 0;
        uint32_t last_bad_plane = 0;
        uint32_t last_bad_got = 0;
        uint32_t last_bad_want = 0;
        uint32_t expected_seq = 0;
        bool seq_valid = false;
        uint64_t window_start_us = 0;

        void reset() {
            bad_frames = 0;
            seq_gaps = 0;
            seq_gap_frames = 0;
            last_bad_plane = 0;
            last_bad_got = 0;
            last_bad_want = 0;
            expected_seq = 0;
            seq_valid = false;
            window_start_us = 0;
        }

        void record_bad_frame(uint32_t plane, uint32_t got, uint32_t want) {
            ++bad_frames;
            last_bad_plane = plane;
            last_bad_got = got;
            last_bad_want = want;
        }

        void record_sequence(uint32_t sequence) {
            if (seq_valid && sequence != expected_seq) {
                ++seq_gaps;
                seq_gap_frames += sequence - expected_seq;
            }
            expected_seq = sequence + 1;
            seq_valid = true;
        }

        void flush(uint64_t now_us, bool force = false) {
            if (!bad_frames && !seq_gaps) return;
            if (window_start_us == 0) {
                window_start_us = now_us;
            }
            if (!force && now_us - window_start_us < 1000000ULL) return;

            fprintf(stderr,
                    "[V4L2Capture] BAD-FRAME: count=%llu (last: plane%u "
                    "bytesused=%u want=%u), driver seq gaps=%llu "
                    "(~%llu frames lost)\n",
                    static_cast<unsigned long long>(bad_frames),
                    last_bad_plane, last_bad_got, last_bad_want,
                    static_cast<unsigned long long>(seq_gaps),
                    static_cast<unsigned long long>(seq_gap_frames));
            bad_frames = 0;
            seq_gaps = 0;
            seq_gap_frames = 0;
            last_bad_plane = 0;
            last_bad_got = 0;
            last_bad_want = 0;
            window_start_us = now_us;
        }
    };

    struct MappedPlane {
        void* start = nullptr;
        size_t length = 0;
        int dma_fd = -1;  // VIDIOC_EXPBUF export (only when prefer_dmabuf)
    };

    struct Buffer {
        std::vector<MappedPlane> planes;
    };

    int fd = -1;
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t fps = 0;
    uint32_t pixel_format = 0;
    v4l2_buf_type buffer_type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    bool prefer_dmabuf = false;
    uint32_t memory_plane_count = 1;
    std::array<uint32_t, VIDEO_MAX_PLANES> plane_strides{};
    std::array<uint32_t, VIDEO_MAX_PLANES> plane_sizeimages{};
    // Guarded by release_mutex: shared between the capture thread and the
    // encoder thread calling release_buffer() for held zero-copy buffers.
    bool streaming = false;
    std::mutex release_mutex;

    std::atomic<bool> running{false};
    std::atomic<bool> thread_alive{false};
    CaptureFrameCallback capture_cb_;
    uint64_t seq_ = 0;
    IntegrityTracker integrity_;
    std::vector<Buffer> buffers;
    std::vector<uint8_t> yuv420_buf;
    std::thread capture_thread;

    // [latency] "capture" stage: driver timestamp (SOF) → DQBUF in userspace.
    LatencyStats dqbuf_stats_{"capture"};
    bool ts_domain_logged_ = false;

    ~V4L2CaptureImpl() override { stop(); }

    bool init(const CaptureConfig& cfg, std::string* err) override;
    bool start(CaptureFrameCallback cb, std::string* err) override;
    void stop() override;
    bool isRunning() const override;
    RawPixelFormat outputFormat() const override;

    void capture_loop();
    void release_resources();
    bool select_format(uint32_t caps, RawPixelFormat requested, std::string* err);
    bool supports_format(v4l2_buf_type type, uint32_t fourcc) const;
    bool queue_buffer(uint32_t index, std::string* err);
    void yuyv_to_yuv420p(const uint8_t* src, uint32_t src_stride);
    bool make_frame(uint32_t index, const v4l2_buffer& buf,
                    const v4l2_plane* dequeued_planes, RawFrame* frame,
                    std::string* err);

    // Zero-copy release entry point wired into RawFrame::release.  The
    // encoder calls it (from any thread) once it finished reading a held
    // capture buffer; becomes a no-op once streaming stopped.
    static void release_trampoline(void* ctx, uint32_t buffer_index);
    void release_buffer(uint32_t index);
    // Attach the deferred-requeue contract to a DmaBuf frame.
    void arm_release(RawFrame* frame, uint32_t index) const;
};

bool V4L2CaptureImpl::supports_format(v4l2_buf_type type, uint32_t fourcc) const {
    v4l2_fmtdesc desc{};
    desc.type = type;
    for (desc.index = 0; ioctl(fd, VIDIOC_ENUM_FMT, &desc) == 0; ++desc.index) {
        if (desc.pixelformat == fourcc) {
            return true;
        }
    }
    return false;
}

// Maps the negotiated V4L2 fourcc to the RawPixelFormat that make_frame()
// actually emits.  YUYV is converted to YUV420P; everything else keeps its
// native layout.
RawPixelFormat V4L2CaptureImpl::outputFormat() const {
    switch (pixel_format) {
    case V4L2_PIX_FMT_YUYV:
        return RawPixelFormat::Yuv420p;
    case V4L2_PIX_FMT_NV12:
    case V4L2_PIX_FMT_NV12M:
        return RawPixelFormat::Nv12;
    case V4L2_PIX_FMT_YUV420:
    case V4L2_PIX_FMT_YUV420M:
        return RawPixelFormat::Yuv420p;
    case V4L2_PIX_FMT_RGB24:
        return RawPixelFormat::Rgb888;
    case V4L2_PIX_FMT_UYVY:
        return RawPixelFormat::Uyvy422;
    default:
        return RawPixelFormat::Yuv420p;
    }
}

bool V4L2CaptureImpl::select_format(
    uint32_t caps, RawPixelFormat requested, std::string* err) {
    if (requested == RawPixelFormat::Mjpeg) {
        if (err) {
            *err = "MJPEG capture is not supported by the livehal pipeline";
        }
        return false;
    }

    std::vector<v4l2_buf_type> types;
    if (caps & V4L2_CAP_VIDEO_CAPTURE_MPLANE) {
        types.push_back(V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE);
    }
    if (caps & V4L2_CAP_VIDEO_CAPTURE) {
        types.push_back(V4L2_BUF_TYPE_VIDEO_CAPTURE);
    }

    for (uint32_t fourcc : requested_fourccs(requested)) {
        for (v4l2_buf_type type : types) {
            if (!supports_format(type, fourcc)) {
                continue;
            }
            buffer_type = type;
            pixel_format = fourcc;
            return true;
        }
    }

    if (err) {
        *err = "requested pixel format is not advertised by the V4L2 capture device";
    }
    return false;
}

void V4L2CaptureImpl::yuyv_to_yuv420p(const uint8_t* src, uint32_t src_stride) {
    uint8_t* y_plane = yuv420_buf.data();
    uint8_t* u_plane = y_plane + static_cast<size_t>(width) * height;
    uint8_t* v_plane = u_plane + static_cast<size_t>(width) * height / 4;

    for (uint32_t row = 0; row < height; ++row) {
        const uint8_t* row_src = src + static_cast<size_t>(row) * src_stride;
        uint8_t* y_row = y_plane + static_cast<size_t>(row) * width;
        for (uint32_t col = 0; col < width; col += 2) {
            const size_t source_offset = static_cast<size_t>(col) * 2;
            y_row[col] = row_src[source_offset];
            if (col + 1 < width) {
                y_row[col + 1] = row_src[source_offset + 2];
            }
            if ((row & 1U) == 0 && col + 1 < width) {
                const size_t chroma_offset =
                    static_cast<size_t>(row / 2) * (width / 2) + col / 2;
                u_plane[chroma_offset] = row_src[source_offset + 1];
                v_plane[chroma_offset] = row_src[source_offset + 3];
            }
        }
    }
}

bool V4L2CaptureImpl::make_frame(
    uint32_t index, const v4l2_buffer& buf, const v4l2_plane* dequeued_planes,
    RawFrame* frame, std::string* err) {
    if (index >= buffers.size() || buffers[index].planes.empty()) {
        if (err) *err = "invalid capture buffer index";
        return false;
    }

    const Buffer& mapped = buffers[index];
    const bool mplane = is_mplane_type(buffer_type);
    auto data_offset = [&](uint32_t plane) -> size_t {
        return mplane ? dequeued_planes[plane].data_offset : 0;
    };
    auto bytes_used = [&](uint32_t plane) -> size_t {
        if (mplane) {
            const size_t used = dequeued_planes[plane].bytesused;
            const size_t offset = data_offset(plane);
            return used > offset ? used - offset : 0;
        }
        return buf.bytesused;
    };
    auto available_bytes = [&](uint32_t plane) -> size_t {
        const size_t length = mapped.planes[plane].length;
        const size_t offset = data_offset(plane);
        return length > offset ? length - offset : 0;
    };

    frame->kind = BufferKind::Cpu;
    frame->width = width;
    frame->height = height;
    frame->pts_us = static_cast<uint64_t>(buf.timestamp.tv_sec) * 1000000
                    + buf.timestamp.tv_usec;
    frame->seq = ++seq_;

    if (pixel_format == V4L2_PIX_FMT_UYVY) {
        const uint32_t stride = plane_strides[0] ? plane_strides[0] : width * 2;
        const size_t required = static_cast<size_t>(stride) * height;
        if ((width & 1U) != 0 || stride < width * 2
            || bytes_used(0) < required || available_bytes(0) < required) {
            if (err) *err = "invalid or short UYVY capture buffer";
            return false;
        }
        frame->format = RawPixelFormat::Uyvy422;
        frame->plane_count = 1;
        frame->planes[0] = {
            static_cast<const uint8_t*>(mapped.planes[0].start) + data_offset(0),
            stride, static_cast<uint32_t>(required), -1, 0};
        return true;
    }

    if (pixel_format == V4L2_PIX_FMT_YUYV) {
        const uint32_t stride = plane_strides[0] ? plane_strides[0] : width * 2;
        const size_t required = static_cast<size_t>(stride) * height;
        if (bytes_used(0) < required || available_bytes(0) < required) {
            if (err) *err = "short YUYV capture buffer";
            return false;
        }
        const auto* source =
            static_cast<const uint8_t*>(mapped.planes[0].start) + data_offset(0);
        yuyv_to_yuv420p(source, stride);
        frame->format = RawPixelFormat::Yuv420p;
        frame->plane_count = 1;
        frame->planes[0] = {
            yuv420_buf.data(), width,
            static_cast<uint32_t>(yuv420_buf.size()), -1, 0};
        return true;
    }

    if (pixel_format == V4L2_PIX_FMT_NV12M) {
        if (mapped.planes.size() < 2 || memory_plane_count < 2) {
            if (err) *err = "NV12M capture returned fewer than two planes";
            return false;
        }
        frame->format = RawPixelFormat::Nv12;
        frame->plane_count = 2;
        if (prefer_dmabuf && mapped.planes[0].dma_fd >= 0
            && mapped.planes[1].dma_fd >= 0) {
            frame->kind = BufferKind::DmaBuf;
            arm_release(frame, index);
        }
        for (uint32_t plane = 0; plane < 2; ++plane) {
            const size_t used = std::min(bytes_used(plane), available_bytes(plane));
            frame->planes[plane] = {
                static_cast<const uint8_t*>(mapped.planes[plane].start)
                    + data_offset(plane),
                plane_strides[plane] ? plane_strides[plane] : width,
                static_cast<uint32_t>(used), mapped.planes[plane].dma_fd, 0};
        }
        return true;
    }

    if (pixel_format == V4L2_PIX_FMT_NV12) {
        const uint32_t stride = plane_strides[0] ? plane_strides[0] : width;
        const size_t y_bytes = static_cast<size_t>(stride) * height;
        const size_t used = std::min(bytes_used(0), available_bytes(0));
        const size_t expected = y_bytes + static_cast<size_t>(stride) * (height / 2);
        if (used < expected) {
            if (err) *err = "short contiguous NV12 capture buffer";
            return false;
        }
        const auto* base =
            static_cast<const uint8_t*>(mapped.planes[0].start) + data_offset(0);
        const int dma_fd = mapped.planes[0].dma_fd;
        if (prefer_dmabuf && dma_fd >= 0) {
            frame->kind = BufferKind::DmaBuf;
            arm_release(frame, index);
        }
        frame->format = RawPixelFormat::Nv12;
        frame->plane_count = 2;
        frame->planes[0] = {
            base, stride, static_cast<uint32_t>(y_bytes), dma_fd, 0};
        frame->planes[1] = {
            base + y_bytes, stride,
            static_cast<uint32_t>(static_cast<size_t>(stride) * (height / 2)),
            dma_fd, static_cast<uint32_t>(y_bytes)};
        return true;
    }

    if (pixel_format == V4L2_PIX_FMT_YUV420
        || pixel_format == V4L2_PIX_FMT_YUV420M) {
        if (pixel_format == V4L2_PIX_FMT_YUV420M && mapped.planes.size() != 3) {
            if (err) *err = "YUV420M capture returned an unexpected plane count";
            return false;
        }
        frame->format = RawPixelFormat::Yuv420p;
        frame->plane_count = static_cast<uint32_t>(mapped.planes.size());
        for (uint32_t plane = 0; plane < frame->plane_count; ++plane) {
            const size_t used = std::min(bytes_used(plane), available_bytes(plane));
            frame->planes[plane] = {
                static_cast<const uint8_t*>(mapped.planes[plane].start)
                    + data_offset(plane),
                plane_strides[plane],
                static_cast<uint32_t>(used), -1, 0};
        }
        return true;
    }

    RawPixelFormat raw_format;
    if (pixel_format == V4L2_PIX_FMT_RGB24) {
        raw_format = RawPixelFormat::Rgb888;
    } else {
        if (err) *err = "unsupported negotiated V4L2 pixel format";
        return false;
    }

    frame->format = raw_format;
    frame->plane_count = 1;
    frame->planes[0] = {
        static_cast<const uint8_t*>(mapped.planes[0].start) + data_offset(0),
        plane_strides[0], static_cast<uint32_t>(
            std::min(bytes_used(0), available_bytes(0))),
        -1, 0};
    return true;
}

bool V4L2CaptureImpl::queue_buffer(uint32_t index, std::string* err) {
    v4l2_buffer buf{};
    std::array<v4l2_plane, VIDEO_MAX_PLANES> planes{};
    buf.type = buffer_type;
    buf.memory = V4L2_MEMORY_MMAP;
    buf.index = index;
    if (is_mplane_type(buffer_type)) {
        buf.length = memory_plane_count;
        buf.m.planes = planes.data();
    }
    if (ioctl(fd, VIDIOC_QBUF, &buf) < 0) {
        if (err) *err = std::string("QBUF failed: ") + strerror(errno);
        return false;
    }
    return true;
}

void V4L2CaptureImpl::arm_release(RawFrame* frame, uint32_t index) const {
    frame->buffer_index = index;
    frame->release = &V4L2CaptureImpl::release_trampoline;
    frame->release_ctx = const_cast<V4L2CaptureImpl*>(this);
}

void V4L2CaptureImpl::release_trampoline(void* ctx, uint32_t buffer_index) {
    static_cast<V4L2CaptureImpl*>(ctx)->release_buffer(buffer_index);
}

// Deferred requeue for a held zero-copy buffer.  Called by the encoder once
// the hardware finished reading the buffer (or on its error/stop paths).
// Races with stop() are fine: after STREAMOFF the driver has returned all
// buffers to us, so a late release is a no-op under the lock.
void V4L2CaptureImpl::release_buffer(uint32_t index) {
    std::lock_guard<std::mutex> lock(release_mutex);
    if (!streaming || index >= buffers.size()) return;
    std::string err;
    if (!queue_buffer(index, &err)) {
        fprintf(stderr, "[V4L2Capture] deferred %s\n", err.c_str());
    }
}

void V4L2CaptureImpl::capture_loop() {
    fprintf(stderr, "[V4L2Capture] capture thread started\n");
    thread_alive.store(true);

    while (running.load()) {
        fd_set fds;
        FD_ZERO(&fds);
        FD_SET(fd, &fds);
        timeval timeout{2, 0};
        const int selected = select(fd + 1, &fds, nullptr, nullptr, &timeout);
        if (selected < 0) {
            if (errno == EINTR) continue;
            fprintf(stderr, "[V4L2Capture] select failed: %s\n", strerror(errno));
            break;
        }
        if (selected == 0) continue;

        v4l2_buffer buf{};
        std::array<v4l2_plane, VIDEO_MAX_PLANES> planes{};
        buf.type = buffer_type;
        buf.memory = V4L2_MEMORY_MMAP;
        if (is_mplane_type(buffer_type)) {
            buf.length = memory_plane_count;
            buf.m.planes = planes.data();
        }

        if (ioctl(fd, VIDIOC_DQBUF, &buf) < 0) {
            if (errno == EAGAIN || errno == EINTR) continue;
            fprintf(stderr, "[V4L2Capture] DQBUF failed: %s\n", strerror(errno));
            break;
        }

        const uint64_t dqbuf_now_us = monotonic_now_us();
        const uint64_t buf_ts_us =
            static_cast<uint64_t>(buf.timestamp.tv_sec) * 1000000
            + buf.timestamp.tv_usec;
        if (!ts_domain_logged_) {
            // One-time clock-domain sanity check: the [latency] deltas are
            // only meaningful if the driver stamps buffers in the
            // CLOCK_MONOTONIC domain (SOF).  If buf lands near boottime
            // instead, expect a constant offset in all stages.
            timespec boot{};
            clock_gettime(CLOCK_BOOTTIME, &boot);
            fprintf(stderr,
                    "[latency] capture clock domain: buf=%llu mono=%llu boot=%llu (us)\n",
                    static_cast<unsigned long long>(buf_ts_us),
                    static_cast<unsigned long long>(dqbuf_now_us),
                    static_cast<unsigned long long>(boot.tv_sec) * 1000000ULL
                        + boot.tv_nsec / 1000ULL);
            ts_domain_logged_ = true;
        }
        dqbuf_stats_.sample(buf_ts_us, dqbuf_now_us);

        // Malformed frames must not reach the encoder: the Rockchip MPP
        // halts permanently on such input. Drop+requeue here and count it.
        bool bad = false;
        uint32_t bad_plane = 0, bad_got = 0, bad_want = 0;
        if (is_mplane_type(buffer_type)) {
            for (uint32_t i = 0; i < memory_plane_count; ++i) {
                if (planes[i].bytesused != plane_sizeimages[i]) {
                    bad = true;
                    bad_plane = i;
                    bad_got = planes[i].bytesused;
                    bad_want = plane_sizeimages[i];
                    break;
                }
            }
        } else if (buf.bytesused != plane_sizeimages[0]) {
            bad = true;
            bad_got = buf.bytesused;
            bad_want = plane_sizeimages[0];
        }
        if (bad) {
            integrity_.record_bad_frame(bad_plane, bad_got, bad_want);
            std::string queue_error;
            if (!queue_buffer(buf.index, &queue_error)) {
                fprintf(stderr, "[V4L2Capture] %s\n", queue_error.c_str());
                break;
            }
            integrity_.flush(dqbuf_now_us);
            continue;
        }

        // Driver-side frame loss shows up as sequence gaps. Count only.
        integrity_.record_sequence(buf.sequence);
        integrity_.flush(dqbuf_now_us);

        RawFrame frame{};
        std::string frame_error;
        if (make_frame(buf.index, buf, planes.data(), &frame, &frame_error)) {
            if (capture_cb_) capture_cb_(frame);
        } else {
            fprintf(stderr, "[V4L2Capture] dropped frame: %s\n", frame_error.c_str());
        }

        // Zero-copy frames with an armed release contract stay owned by the
        // consumer (encoder); it requeues via release_buffer() once the
        // hardware finished reading.  All other frames requeue immediately.
        if (frame.release != nullptr) continue;

        std::string queue_error;
        if (!queue_buffer(buf.index, &queue_error)) {
            fprintf(stderr, "[V4L2Capture] %s\n", queue_error.c_str());
            break;
        }
    }

    running.store(false);
    thread_alive.store(false);
    fprintf(stderr, "[V4L2Capture] capture thread stopped\n");
}

bool V4L2CaptureImpl::init(const CaptureConfig& cfg, std::string* err) {
    width = cfg.width;
    height = cfg.height;
    fps = cfg.fps;
    prefer_dmabuf = cfg.prefer_dmabuf;

    fd = ::open(cfg.device.c_str(), O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) {
        if (err) *err = "failed to open " + cfg.device + ": " + strerror(errno);
        return false;
    }

    v4l2_capability cap{};
    if (ioctl(fd, VIDIOC_QUERYCAP, &cap) < 0) {
        if (err) *err = std::string("QUERYCAP failed: ") + strerror(errno);
        release_resources();
        return false;
    }
    const uint32_t caps =
        (cap.capabilities & V4L2_CAP_DEVICE_CAPS) ? cap.device_caps : cap.capabilities;
    if (!(caps & (V4L2_CAP_VIDEO_CAPTURE | V4L2_CAP_VIDEO_CAPTURE_MPLANE))) {
        if (err) *err = "device does not support V4L2 video capture";
        release_resources();
        return false;
    }
    if (!(caps & V4L2_CAP_STREAMING)) {
        if (err) *err = "device does not support V4L2 streaming I/O";
        release_resources();
        return false;
    }
    if (!select_format(caps, cfg.pixel_format, err)) {
        release_resources();
        return false;
    }

    v4l2_format fmt{};
    fmt.type = buffer_type;
    if (is_mplane_type(buffer_type)) {
        fmt.fmt.pix_mp.width = width;
        fmt.fmt.pix_mp.height = height;
        fmt.fmt.pix_mp.pixelformat = pixel_format;
        fmt.fmt.pix_mp.field = V4L2_FIELD_ANY;
    } else {
        fmt.fmt.pix.width = width;
        fmt.fmt.pix.height = height;
        fmt.fmt.pix.pixelformat = pixel_format;
        fmt.fmt.pix.field = V4L2_FIELD_ANY;
    }
    if (ioctl(fd, VIDIOC_S_FMT, &fmt) < 0) {
        if (err) *err = std::string("S_FMT failed: ") + strerror(errno);
        release_resources();
        return false;
    }

    if (is_mplane_type(buffer_type)) {
        width = fmt.fmt.pix_mp.width;
        height = fmt.fmt.pix_mp.height;
        pixel_format = fmt.fmt.pix_mp.pixelformat;
        memory_plane_count = fmt.fmt.pix_mp.num_planes;
        if (memory_plane_count == 0 || memory_plane_count > VIDEO_MAX_PLANES) {
            if (err) *err = "driver returned an invalid V4L2 plane count";
            release_resources();
            return false;
        }
        for (uint32_t plane = 0; plane < memory_plane_count; ++plane) {
            plane_strides[plane] = fmt.fmt.pix_mp.plane_fmt[plane].bytesperline;
            plane_sizeimages[plane] = fmt.fmt.pix_mp.plane_fmt[plane].sizeimage;
        }
    } else {
        width = fmt.fmt.pix.width;
        height = fmt.fmt.pix.height;
        pixel_format = fmt.fmt.pix.pixelformat;
        memory_plane_count = 1;
        plane_strides[0] = fmt.fmt.pix.bytesperline;
        plane_sizeimages[0] = fmt.fmt.pix.sizeimage;
    }

    const auto accepted = requested_fourccs(cfg.pixel_format);
    if (std::find(accepted.begin(), accepted.end(), pixel_format) == accepted.end()) {
        char actual[5];
        if (err) {
            *err = std::string("driver changed requested pixel format to unsupported ")
                   + fourcc_to_string(pixel_format, actual);
        }
        release_resources();
        return false;
    }
    if ((pixel_format == V4L2_PIX_FMT_NV12
         || pixel_format == V4L2_PIX_FMT_NV12M
         || pixel_format == V4L2_PIX_FMT_YUV420
         || pixel_format == V4L2_PIX_FMT_YUV420M)
        && ((width & 1U) != 0 || (height & 1U) != 0)) {
        if (err) *err = "4:2:0 capture requires even width and height";
        release_resources();
        return false;
    }
    if (pixel_format == V4L2_PIX_FMT_UYVY && (width & 1U) != 0) {
        if (err) *err = "UYVY capture requires an even width";
        release_resources();
        return false;
    }

    v4l2_streamparm parm{};
    parm.type = buffer_type;
    parm.parm.capture.timeperframe.numerator = 1;
    parm.parm.capture.timeperframe.denominator = fps;
    if (fps != 0 && ioctl(fd, VIDIOC_S_PARM, &parm) < 0) {
        fprintf(stderr, "[V4L2Capture] S_PARM failed: %s\n", strerror(errno));
    }
    parm = {};
    parm.type = buffer_type;
    if (ioctl(fd, VIDIOC_G_PARM, &parm) == 0
        && parm.parm.capture.timeperframe.numerator != 0) {
        fps = parm.parm.capture.timeperframe.denominator
              / parm.parm.capture.timeperframe.numerator;
    }

    char actual[5];
    fprintf(stderr,
            "[V4L2Capture] negotiated %ux%u %s at %u fps, memory planes=%u\n",
            width, height, fourcc_to_string(pixel_format, actual), fps,
            memory_plane_count);

    if (pixel_format == V4L2_PIX_FMT_YUYV) {
        yuv420_buf.resize(static_cast<size_t>(width) * height * 3 / 2);
    }

    v4l2_requestbuffers req{};
    req.count = kBufferCount;
    req.type = buffer_type;
    req.memory = V4L2_MEMORY_MMAP;
    if (ioctl(fd, VIDIOC_REQBUFS, &req) < 0) {
        if (err) *err = std::string("REQBUFS failed: ") + strerror(errno);
        release_resources();
        return false;
    }
    if (req.count < 2) {
        if (err) *err = "REQBUFS returned fewer than two buffers";
        release_resources();
        return false;
    }

    for (uint32_t index = 0; index < req.count; ++index) {
        v4l2_buffer buf{};
        std::array<v4l2_plane, VIDEO_MAX_PLANES> planes{};
        buf.type = buffer_type;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = index;
        if (is_mplane_type(buffer_type)) {
            buf.length = memory_plane_count;
            buf.m.planes = planes.data();
        }
        if (ioctl(fd, VIDIOC_QUERYBUF, &buf) < 0) {
            if (err) *err = std::string("QUERYBUF failed: ") + strerror(errno);
            release_resources();
            return false;
        }

        Buffer mapped;
        mapped.planes.reserve(memory_plane_count);
        for (uint32_t plane = 0; plane < memory_plane_count; ++plane) {
            const size_t length =
                is_mplane_type(buffer_type) ? planes[plane].length : buf.length;
            const off_t offset =
                is_mplane_type(buffer_type) ? planes[plane].m.mem_offset : buf.m.offset;
            void* start = mmap(
                nullptr, length, PROT_READ | PROT_WRITE, MAP_SHARED, fd, offset);
            if (start == MAP_FAILED) {
                if (err) *err = std::string("mmap failed: ") + strerror(errno);
                for (MappedPlane& mapped_plane : mapped.planes) {
                    munmap(mapped_plane.start, mapped_plane.length);
                    if (mapped_plane.dma_fd >= 0) {
                        close(mapped_plane.dma_fd);
                    }
                }
                release_resources();
                return false;
            }
            mapped.planes.push_back({start, length});
        }

        // Zero-copy: export each buffer as a dmabuf (works for MMAP buffers;
        // rkaiisp/rkcif support expbuf, mirroring what the vendor VI uses).
        if (prefer_dmabuf) {
            for (uint32_t plane = 0; plane < memory_plane_count; ++plane) {
                v4l2_exportbuffer ebuf{};
                ebuf.type = buffer_type;
                ebuf.index = index;
                ebuf.plane = plane;
                ebuf.flags = O_RDWR | O_CLOEXEC;
                if (ioctl(fd, VIDIOC_EXPBUF, &ebuf) == 0) {
                    mapped.planes[plane].dma_fd = ebuf.fd;
                } else {
                    fprintf(stderr,
                        "[V4L2Capture] EXPBUF failed for buffer %u plane %u: %s "
                        "(zero-copy disabled, CPU path in use)\n",
                        index, plane, strerror(errno));
                    prefer_dmabuf = false;  // don't retry every buffer
                    break;
                }
            }
        }
        buffers.push_back(std::move(mapped));
    }
    return true;
}

bool V4L2CaptureImpl::start(CaptureFrameCallback cb, std::string* err) {
    if (fd < 0) {
        if (err) *err = "capture backend is not initialised";
        return false;
    }
    if (running.load() || capture_thread.joinable()) {
        if (err) *err = "capture already running";
        return false;
    }
    capture_cb_ = std::move(cb);
    integrity_.reset();

    for (uint32_t index = 0; index < buffers.size(); ++index) {
        if (!queue_buffer(index, err)) return false;
    }
    if (ioctl(fd, VIDIOC_STREAMON, &buffer_type) < 0) {
        if (err) *err = std::string("STREAMON failed: ") + strerror(errno);
        return false;
    }
    {
        std::lock_guard<std::mutex> lock(release_mutex);
        streaming = true;
    }
    running.store(true);
    capture_thread = std::thread(&V4L2CaptureImpl::capture_loop, this);
    return true;
}

void V4L2CaptureImpl::release_resources() {
    {
        // Block deferred requeues before STREAMOFF: once streaming is off
        // the driver returns all buffers (including held zero-copy ones),
        // so any late release_buffer() must become a no-op.
        std::lock_guard<std::mutex> lock(release_mutex);
        if (fd >= 0 && streaming) {
            ioctl(fd, VIDIOC_STREAMOFF, &buffer_type);
            streaming = false;
        }
    }
    for (Buffer& buffer : buffers) {
        for (MappedPlane& plane : buffer.planes) {
            if (plane.start && plane.start != MAP_FAILED) {
                munmap(plane.start, plane.length);
                plane.start = nullptr;
            }
            if (plane.dma_fd >= 0) {
                close(plane.dma_fd);
                plane.dma_fd = -1;
            }
        }
    }
    buffers.clear();
    if (fd >= 0) {
        close(fd);
        fd = -1;
    }
    capture_cb_ = nullptr;
}

void V4L2CaptureImpl::stop() {
    running.store(false);
    if (capture_thread.joinable()) capture_thread.join();
    integrity_.flush(monotonic_now_us(), true);
    release_resources();
}

bool V4L2CaptureImpl::isRunning() const {
    return running.load() && thread_alive.load();
}

std::shared_ptr<CaptureBackend> create_v4l2_capture_backend(
    const CaptureConfig& cfg) {
    (void)cfg;
    return std::make_shared<V4L2CaptureImpl>();
}
