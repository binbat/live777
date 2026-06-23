#include "include/encoder_backend.h"
#include <atomic>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <linux/videodev2.h>
#include <mutex>
#include <queue>
#include <string>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>
#include <utility>
#include <vector>

// ---------------------------------------------------------------------------
// EncoderBackend implementation (V4L2 M2M)
// ---------------------------------------------------------------------------
class V4l2M2mEncoder : public EncoderBackend {
public:
    int fd = -1;
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t fps = 0;
    uint32_t bitrate = 0;
    VideoCodec codec_ = VideoCodec::H264;

    std::string errorMsg;

    EncodedPacketCallback encoded_cb_;

    struct Buffer {
        void* start;
        size_t length;
    };

    std::vector<Buffer> inputBuffers;
    std::vector<Buffer> outputBuffers;
    std::queue<int> freeInputIndices;
    std::atomic<bool> force_idr{false};
    int frames_injected = 0;
    int frames_dropped = 0;
    std::atomic<bool> running_{false};

    // Serialises submit() with stop()/cleanup() so that buffers/fd are not
    // released while a frame is being processed.
    std::mutex mutex_;

    V4l2M2mEncoder() = default;
    ~V4l2M2mEncoder() override { cleanup(); }

    void cleanup();
    static const char* default_device_path();
    static uint32_t codec_to_v4l2_pixelformat(VideoCodec codec);

    // --- EncoderBackend overrides ---
    bool init(const EncoderConfig& cfg, std::string* err) override;
    bool submit(const RawFrame& frame, std::string* err) override;
    void requestKeyframe() override;
    void stop() override;
    bool isRunning() const override;
    void setCallback(EncodedPacketCallback cb) override;
};

const char* V4l2M2mEncoder::default_device_path() {
    if (const char* env = std::getenv("LIVE777_ENCODER_V4L2_M2M_DEVICE")) {
        return env;
    }
    return "/dev/video11";
}

uint32_t V4l2M2mEncoder::codec_to_v4l2_pixelformat(VideoCodec codec) {
    switch (codec) {
    case VideoCodec::H265:
        return V4L2_PIX_FMT_HEVC;
    case VideoCodec::H264:
    default:
        return V4L2_PIX_FMT_H264;
    }
}

bool V4l2M2mEncoder::init(const EncoderConfig& cfg, std::string* err) {
    width = cfg.width;
    height = cfg.height;
    fps = cfg.fps;
    bitrate = cfg.bitrate;
    codec_ = cfg.codec;

    const char* dev = default_device_path();
    fd = open(dev, O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) {
        if (err) *err = std::string("Failed to open ") + dev + ": " + strerror(errno);
        return false;
    }

    struct v4l2_format fmt = {};
    fmt.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    fmt.fmt.pix_mp.width = width;
    fmt.fmt.pix_mp.height = height;
    fmt.fmt.pix_mp.pixelformat = V4L2_PIX_FMT_YUV420;
    fmt.fmt.pix_mp.field = V4L2_FIELD_NONE;
    fmt.fmt.pix_mp.num_planes = 1;

    if (ioctl(fd, VIDIOC_S_FMT, &fmt) < 0) {
        if (err) *err = std::string("S_FMT (OUTPUT) failed: ") + strerror(errno);
        cleanup();
        return false;
    }

    fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    fmt.fmt.pix_mp.pixelformat = codec_to_v4l2_pixelformat(codec_);
    if (ioctl(fd, VIDIOC_S_FMT, &fmt) < 0) {
        if (err) *err = std::string("S_FMT (CAPTURE) failed: ") + strerror(errno);
        cleanup();
        return false;
    }

    struct v4l2_control ctrl = {};
    ctrl.id = V4L2_CID_MPEG_VIDEO_BITRATE;
    ctrl.value = bitrate;
    if (ioctl(fd, VIDIOC_S_CTRL, &ctrl) < 0) {
        if (err) *err = std::string("S_CTRL BITRATE failed: ") + strerror(errno);
        cleanup();
        return false;
    }

    ctrl.id = V4L2_CID_MPEG_VIDEO_H264_I_PERIOD;
    ctrl.value = fps * 2;
    if (ioctl(fd, VIDIOC_S_CTRL, &ctrl) < 0) {
        if (err) *err = std::string("S_CTRL I_PERIOD failed: ") + strerror(errno);
        cleanup();
        return false;
    }

    ctrl.id = V4L2_CID_MPEG_VIDEO_REPEAT_SEQ_HEADER;
    ctrl.value = 1;
    if (ioctl(fd, VIDIOC_S_CTRL, &ctrl) < 0) {
        if (err) *err = std::string("S_CTRL REPEAT_SEQ_HEADER failed: ") + strerror(errno);
        cleanup();
        return false;
    }

    struct v4l2_requestbuffers req = {};
    req.count = 8;
    req.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    req.memory = V4L2_MEMORY_MMAP;
    if (ioctl(fd, VIDIOC_REQBUFS, &req) < 0) {
        if (err) *err = std::string("REQBUFS (OUTPUT) failed: ") + strerror(errno);
        cleanup();
        return false;
    }

    for (unsigned int i = 0; i < req.count; i++) {
        struct v4l2_buffer buf = {};
        struct v4l2_plane planes[1] = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        buf.length = 1;
        buf.m.planes = planes;
        if (ioctl(fd, VIDIOC_QUERYBUF, &buf) < 0) {
            if (err) *err = std::string("QUERYBUF (OUTPUT) failed: ") + strerror(errno);
            cleanup();
            return false;
        }
        void* start = mmap(NULL, planes[0].length, PROT_READ | PROT_WRITE,
                           MAP_SHARED, fd, planes[0].m.mem_offset);
        if (start == MAP_FAILED) {
            if (err) *err = "mmap failed for input buffer";
            cleanup();
            return false;
        }
        inputBuffers.push_back({start, planes[0].length});
        freeInputIndices.push(i);
    }

    req.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    if (ioctl(fd, VIDIOC_REQBUFS, &req) < 0) {
        if (err) *err = std::string("REQBUFS (CAPTURE) failed: ") + strerror(errno);
        cleanup();
        return false;
    }
    for (unsigned int i = 0; i < req.count; i++) {
        struct v4l2_buffer buf = {};
        struct v4l2_plane planes[1] = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        buf.length = 1;
        buf.m.planes = planes;
        if (ioctl(fd, VIDIOC_QUERYBUF, &buf) < 0) {
            if (err) *err = std::string("QUERYBUF (CAPTURE) failed: ") + strerror(errno);
            cleanup();
            return false;
        }
        void* start = mmap(NULL, planes[0].length, PROT_READ | PROT_WRITE,
                           MAP_SHARED, fd, planes[0].m.mem_offset);
        if (start == MAP_FAILED) {
            if (err) *err = "mmap failed for output buffer";
            cleanup();
            return false;
        }
        outputBuffers.push_back({start, planes[0].length});
        if (ioctl(fd, VIDIOC_QBUF, &buf) < 0) {
            if (err) *err = std::string("QBUF (CAPTURE) failed: ") + strerror(errno);
            cleanup();
            return false;
        }
    }

    enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    if (ioctl(fd, VIDIOC_STREAMON, &type) < 0) {
        if (err) *err = std::string("STREAMON (OUTPUT) failed: ") + strerror(errno);
        cleanup();
        return false;
    }
    type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    if (ioctl(fd, VIDIOC_STREAMON, &type) < 0) {
        if (err) *err = std::string("STREAMON (CAPTURE) failed: ") + strerror(errno);
        cleanup();
        return false;
    }

    running_.store(true);
    return true;
}

bool V4l2M2mEncoder::submit(const RawFrame& frame, std::string* err) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (fd < 0 || !running_.load()) {
        if (err) *err = "encoder not running";
        return false;
    }
    if (frame.kind != BufferKind::Cpu) {
        if (err) *err = "CPU-frame required for V4L2 M2M encoder";
        return false;
    }

    // Drain output pool and dispatch via new callback
    struct v4l2_buffer buf_out = {};
    struct v4l2_plane planes_out[1] = {};
    buf_out.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    buf_out.memory = V4L2_MEMORY_MMAP;
    buf_out.length = 1;
    buf_out.m.planes = planes_out;

    while (ioctl(fd, VIDIOC_DQBUF, &buf_out) == 0) {
        if (buf_out.index >= outputBuffers.size()) {
            fprintf(stderr, "[V4l2M2mEncoder] invalid output buffer index %u\n", buf_out.index);
            break;
        }
        if (encoded_cb_) {
            uint8_t* raw = static_cast<uint8_t*>(outputBuffers[buf_out.index].start);
            size_t len = planes_out[0].bytesused;
            uint32_t flags = 0;
            if (buf_out.flags & V4L2_BUF_FLAG_KEYFRAME) flags |= static_cast<uint32_t>(EncodedKeyframe);

            EncodedPacket pkt{};
            pkt.codec = codec_;
            pkt.data = raw;
            pkt.size = len;
            pkt.pts_us = (uint64_t)buf_out.timestamp.tv_sec * 1000000
                         + buf_out.timestamp.tv_usec;
            pkt.dts_us = pkt.pts_us;
            pkt.flags = flags;
            encoded_cb_(pkt);
        }
        if (ioctl(fd, VIDIOC_QBUF, &buf_out) < 0) {
            fprintf(stderr, "[V4l2M2mEncoder] requeue output buffer failed: %s\n", strerror(errno));
            break;
        }
    }

    // Reclaim input pool
    struct v4l2_buffer buf_in = {};
    struct v4l2_plane planes_in[1] = {};
    buf_in.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    buf_in.memory = V4L2_MEMORY_MMAP;
    buf_in.length = 1;
    buf_in.m.planes = planes_in;

    while (ioctl(fd, VIDIOC_DQBUF, &buf_in) == 0) {
        if (buf_in.index < inputBuffers.size()) {
            freeInputIndices.push(buf_in.index);
        } else {
            fprintf(stderr, "[V4l2M2mEncoder] invalid input buffer index %u\n", buf_in.index);
        }
    }

    // Feed input frame
    if (!freeInputIndices.empty()) {
        const uint8_t* src = frame.planes[0].data;
        size_t src_size = frame.planes[0].bytes;

        if (src == nullptr) {
            if (err) *err = "frame data is null";
            return false;
        }

        int idx = freeInputIndices.front();
        freeInputIndices.pop();

        if (src_size > inputBuffers[idx].length) {
            freeInputIndices.push(idx);
            if (err) *err = "frame size exceeds input buffer";
            return false;
        }
        memcpy(inputBuffers[idx].start, src, src_size);

        // Re-initialise the buffer structure before QBUF.  The same
        // structure was used for DQBUF above and may contain stale
        // flags/timestamps written back by the driver.
        buf_in = {};
        planes_in[0] = {};
        buf_in.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
        buf_in.memory = V4L2_MEMORY_MMAP;
        buf_in.length = 1;
        buf_in.m.planes = planes_in;
        buf_in.index = idx;
        planes_in[0].bytesused = src_size;
        buf_in.timestamp.tv_sec = frame.pts_us / 1000000;
        buf_in.timestamp.tv_usec = frame.pts_us % 1000000;

        if (force_idr.exchange(false)) {
            struct v4l2_control ctrl = {};
            ctrl.id = V4L2_CID_MPEG_VIDEO_FORCE_KEY_FRAME;
            ctrl.value = 1;
            ioctl(fd, VIDIOC_S_CTRL, &ctrl);
        }

        if (ioctl(fd, VIDIOC_QBUF, &buf_in) == 0) {
            frames_injected++;
        } else {
            // Return the buffer index to the free pool on queue failure.
            freeInputIndices.push(idx);
            if (err) *err = std::string("QBUF (OUTPUT) failed: ") + strerror(errno);
            return false;
        }
    } else {
        frames_dropped++;
    }
    return true;
}

void V4l2M2mEncoder::requestKeyframe() {
    force_idr.store(true);
}

void V4l2M2mEncoder::stop() {
    std::lock_guard<std::mutex> lock(mutex_);
    running_.store(false);
    cleanup();
}

bool V4l2M2mEncoder::isRunning() const {
    return running_.load();
}

void V4l2M2mEncoder::setCallback(EncodedPacketCallback cb) {
    encoded_cb_ = std::move(cb);
}

void V4l2M2mEncoder::cleanup() {
    // Caller must hold mutex_.
    if (fd >= 0) {
        enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
        ioctl(fd, VIDIOC_STREAMOFF, &type);
        type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        ioctl(fd, VIDIOC_STREAMOFF, &type);

        for (auto& buf : inputBuffers) {
            if (buf.start && buf.start != MAP_FAILED) munmap(buf.start, buf.length);
        }
        for (auto& buf : outputBuffers) {
            if (buf.start && buf.start != MAP_FAILED) munmap(buf.start, buf.length);
        }

        close(fd);
        fd = -1;
    }
    inputBuffers.clear();
    outputBuffers.clear();
    while (!freeInputIndices.empty()) freeInputIndices.pop();
    running_.store(false);
}

// ---------------------------------------------------------------------------
// Factory for CaptureBackend (V4L2 M2M)
// ---------------------------------------------------------------------------
std::shared_ptr<EncoderBackend> create_v4l2_m2m_encoder_backend(const EncoderConfig& cfg) {
    (void)cfg;
    return std::make_shared<V4l2M2mEncoder>();
}
