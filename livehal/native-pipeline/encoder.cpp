#include "include/encoder_backend.h"
#include "include/latency_stats.h"
#include "include/v4l2_m2m_dmabuf.h"
#include "include/v4l2_m2m_format.h"
#include <atomic>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <fcntl.h>
#include <linux/dma-heap.h>
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
    RawPixelFormat input_format_ = RawPixelFormat::Yuv420p;
    uint32_t input_stride_ = 0;
    size_t input_frame_bytes_ = 0;

    std::string errorMsg;

    EncodedPacketCallback encoded_cb_;

    struct Buffer {
        void* start;
        size_t length;
    };

    // OUTPUT (raw input) queue mode.  Mmap: driver buffers are mmap'd and
    // frames are memcpy'd in (the universal path).  Dmabuf: zero-copy —
    // capture dmabuf fds are queued directly, with a dma_heap fallback pool
    // for frames whose layout the encoder cannot import.
    enum class InputMode { Mmap, Dmabuf };
    InputMode input_mode_ = InputMode::Mmap;

    std::vector<Buffer> inputBuffers;
    std::vector<Buffer> outputBuffers;
    std::queue<int> freeInputIndices;

    // Dmabuf mode: ownership pairing for queued OUTPUT slots.  OUTPUT DQBUF
    // returns buffers in submission order, so each dequeue completes the
    // oldest entry: a held capture buffer goes back via its release callback
    // (deferred requeue), a fallback-pool slot returns to the free list.
    struct InFlightEntry {
        CaptureBufferReleaseFn release = nullptr;
        void* release_ctx = nullptr;
        uint32_t buffer_index = 0;
        int pool_slot = -1; // >= 0: dma_heap fallback pool slot to return
    };
    std::deque<InFlightEntry> in_flight_;

    // Dmabuf mode copy fallback: CPU-writable dmabufs for frames the encoder
    // cannot import (CPU frames, stride/offset mismatch).  Same per-frame
    // cost as the Mmap path.
    struct PoolBuffer {
        int fd = -1;
        void* start = nullptr;
        size_t length = 0;
    };
    std::vector<PoolBuffer> fallback_pool_;
    std::vector<int> free_pool_slots_;
    int dma_heap_fd_ = -1;
    bool dmabuf_fallback_logged_ = false;

    std::atomic<bool> force_idr{false};
    int frames_injected = 0;
    int frames_dropped = 0;
    std::atomic<bool> running_{false};

    // [latency] "encode" stage: capture SOF → encoded frame dequeued.
    LatencyStats enc_stats_{"encode"};

    // Serialises submit() with stop()/cleanup() so that buffers/fd are not
    // released while a frame is being processed.
    std::mutex mutex_;

    V4l2M2mEncoder() = default;
    ~V4l2M2mEncoder() override { cleanup(); }

    void cleanup();
    static const char* default_device_path();
    static uint32_t codec_to_v4l2_pixelformat(VideoCodec codec);
    bool allocate_fallback_pool(unsigned int count);
    void release_in_flight_head();
    bool copy_frame_into(const RawFrame& frame, uint8_t* destination,
                         size_t destination_length, size_t* out_size,
                         std::string* err) const;
    bool queue_dmabuf_input(int idx, int dma_fd, size_t length,
                            size_t bytes_used, uint64_t pts_us,
                            std::string* err);

    // --- EncoderBackend overrides ---
    bool init(const EncoderConfig& cfg, std::string* err) override;
    bool submit(const RawFrame& frame, std::string* err) override;
    void requestKeyframe() override;
    bool setBitrate(uint32_t bps) override;
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
    input_format_ = cfg.input_format;

    uint32_t input_fourcc = 0;
    if (!v4l2_m2m_input_fourcc(input_format_, &input_fourcc)) {
        if (err) *err = "V4L2 M2M encoder supports only NV12, YUV420P or UYVY input";
        return false;
    }

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
    fmt.fmt.pix_mp.pixelformat = input_fourcc;
    fmt.fmt.pix_mp.field = V4L2_FIELD_NONE;
    fmt.fmt.pix_mp.num_planes = 1;

    if (ioctl(fd, VIDIOC_S_FMT, &fmt) < 0) {
        if (err) *err = std::string("S_FMT (OUTPUT) failed: ") + strerror(errno);
        cleanup();
        return false;
    }
    if (fmt.fmt.pix_mp.pixelformat != input_fourcc
        || fmt.fmt.pix_mp.num_planes != 1) {
        if (err) *err = "V4L2 M2M encoder did not accept the requested single-plane input format";
        cleanup();
        return false;
    }
    input_stride_ = fmt.fmt.pix_mp.plane_fmt[0].bytesperline;
    if (input_stride_ == 0) {
        input_stride_ = input_format_ == RawPixelFormat::Uyvy422
            ? width * 2 : width;
    }
    V4l2M2mInputLayout input_layout{};
    if (!resolve_v4l2_m2m_input_layout(
            input_format_, width, height, input_stride_, &input_layout)) {
        if (err) *err = "encoder returned an invalid input layout";
        cleanup();
        return false;
    }
    input_frame_bytes_ = input_layout.frame_bytes;

    fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    fmt.fmt.pix_mp.pixelformat = codec_to_v4l2_pixelformat(codec_);
    if (ioctl(fd, VIDIOC_S_FMT, &fmt) < 0) {
        if (err) *err = std::string("S_FMT (CAPTURE) failed: ") + strerror(errno);
        cleanup();
        return false;
    }

    struct v4l2_control ctrl = {};

    // Rate-control mode (VBR/CBR).  Not all M2M drivers expose this control
    // (bcm2835-codec does, defaulting to VBR), so a failure is a warning —
    // the encoder still runs with the driver's default mode.
    if (cfg.bitrate_mode != 0) {
        ctrl.id = V4L2_CID_MPEG_VIDEO_BITRATE_MODE;
        ctrl.value = cfg.bitrate_mode == 2
            ? V4L2_MPEG_VIDEO_BITRATE_MODE_CBR
            : V4L2_MPEG_VIDEO_BITRATE_MODE_VBR;
        if (ioctl(fd, VIDIOC_S_CTRL, &ctrl) < 0) {
            fprintf(stderr,
                    "[V4l2M2mEncoder] S_CTRL BITRATE_MODE(%s) failed: %s — "
                    "keeping driver default\n",
                    cfg.bitrate_mode == 2 ? "CBR" : "VBR", strerror(errno));
        }
    }

    ctrl.id = V4L2_CID_MPEG_VIDEO_BITRATE;
    ctrl.value = bitrate;
    if (ioctl(fd, VIDIOC_S_CTRL, &ctrl) < 0) {
        if (err) *err = std::string("S_CTRL BITRATE failed: ") + strerror(errno);
        cleanup();
        return false;
    }

    // H.264-specific controls: only apply when the output codec is H.264.
    if (codec_ == VideoCodec::H264) {
        // bcm2835-codec: writing I_PERIOD also updates GOP_SIZE, and every
        // I frame is an IDR, so this is the keyframe interval in frames.
        // gop == 0 disables periodic keyframes entirely (MMAL INTRAPERIOD=0:
        // one initial IDR, then P-frames only) — keyframes are then produced
        // solely by FORCE_KEY_FRAME (RTCP PLI/FIR / subscriber join).
        ctrl.id = V4L2_CID_MPEG_VIDEO_H264_I_PERIOD;
        ctrl.value = static_cast<int>(cfg.gop);
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
    }

    struct v4l2_requestbuffers req = {};
    req.count = 8;
    req.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    if (cfg.prefer_dmabuf) {
        // Zero-copy input: capture dmabuf fds are queued directly, so the
        // OUTPUT queue needs buffer slots but no driver-allocated memory.
        req.memory = V4L2_MEMORY_DMABUF;
        if (ioctl(fd, VIDIOC_REQBUFS, &req) == 0) {
            if (req.count >= 2) {
                input_mode_ = InputMode::Dmabuf;
                for (unsigned int i = 0; i < req.count; i++) {
                    freeInputIndices.push(i);
                }
                if (!allocate_fallback_pool(req.count)) {
                    fprintf(stderr,
                            "[V4l2M2mEncoder] dma_heap unavailable: no copy "
                            "fallback for non-importable frames\n");
                }
                fprintf(stderr,
                        "[V4l2M2mEncoder] zero-copy input enabled (%u DMABUF "
                        "slots, %zu fallback buffers)\n",
                        req.count, fallback_pool_.size());
            } else {
                struct v4l2_requestbuffers release_req = {};
                release_req.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
                release_req.memory = V4L2_MEMORY_DMABUF;
                ioctl(fd, VIDIOC_REQBUFS, &release_req);
            }
        } else {
            fprintf(stderr,
                    "[V4l2M2mEncoder] DMABUF REQBUFS failed: %s — falling "
                    "back to MMAP input\n",
                    strerror(errno));
        }
        req = {};
        req.count = 8;
        req.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    }
    if (input_mode_ == InputMode::Mmap) {
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
    }

    req.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    req.memory = V4L2_MEMORY_MMAP;
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

    // Deferred-requeue contract (media_types.h): in Dmabuf mode a zero-copy
    // frame's capture buffer moves into in_flight_ on a successful QBUF and
    // is handed back when the hardware returns the OUTPUT slot; every other
    // path (validation failure, copy fallback, Mmap mode) finishes with the
    // buffer synchronously, so the guard releases it on return.
    FrameReleaseGuard release_guard(frame);

    if (fd < 0 || !running_.load()) {
        if (err) *err = "encoder not running";
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
        enc_stats_.sample(
            static_cast<uint64_t>(buf_out.timestamp.tv_sec) * 1000000
                + buf_out.timestamp.tv_usec,
            monotonic_now_us());
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

    // Reclaim input slots.  In Dmabuf mode each dequeued OUTPUT slot also
    // completes the oldest in-flight entry: a held capture buffer goes back
    // to the capture backend (deferred requeue), a fallback-pool slot
    // returns to the free list.
    struct v4l2_buffer buf_in = {};
    struct v4l2_plane planes_in[1] = {};
    buf_in.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    buf_in.memory = input_mode_ == InputMode::Dmabuf
        ? V4L2_MEMORY_DMABUF : V4L2_MEMORY_MMAP;
    buf_in.length = 1;
    buf_in.m.planes = planes_in;

    while (ioctl(fd, VIDIOC_DQBUF, &buf_in) == 0) {
        if (input_mode_ == InputMode::Dmabuf) {
            release_in_flight_head();
            freeInputIndices.push(buf_in.index);
        } else if (buf_in.index < inputBuffers.size()) {
            freeInputIndices.push(buf_in.index);
        } else {
            fprintf(stderr, "[V4l2M2mEncoder] invalid input buffer index %u\n", buf_in.index);
        }
    }

    // Feed input frame
    if (!freeInputIndices.empty()) {
        if (frame.width != width || frame.height != height) {
            if (err) *err = "frame dimensions do not match encoder configuration";
            return false;
        }

        if (force_idr.exchange(false)) {
            struct v4l2_control ctrl = {};
            ctrl.id = V4L2_CID_MPEG_VIDEO_FORCE_KEY_FRAME;
            ctrl.value = 1;
            ioctl(fd, VIDIOC_S_CTRL, &ctrl);
        }

        int idx = freeInputIndices.front();
        freeInputIndices.pop();
        auto fail_input = [&](const char* message) {
            freeInputIndices.push(idx);
            if (err) *err = message;
            return false;
        };

        if (input_mode_ == InputMode::Dmabuf) {
            size_t import_bytes = 0;
            if (v4l2_m2m_dmabuf_importable(
                    input_format_, width, height, input_stride_, frame,
                    &import_bytes)) {
                // Zero-copy: queue the capture dmabuf as-is.  Ownership of
                // the held capture buffer moves to in_flight_ only on a
                // successful QBUF; the guard covers every failure path.
                if (!queue_dmabuf_input(idx, frame.planes[0].dma_fd,
                                        import_bytes, import_bytes,
                                        frame.pts_us, err)) {
                    freeInputIndices.push(idx);
                    return false;
                }
                in_flight_.push_back(
                    {frame.release, frame.release_ctx, frame.buffer_index, -1});
                release_guard.disarm();
            } else {
                // Copy fallback: the frame layout is not importable (CPU
                // frame, stride/offset mismatch).  Copy into a dma_heap pool
                // buffer and queue that — same per-frame cost as Mmap mode.
                if (!dmabuf_fallback_logged_) {
                    dmabuf_fallback_logged_ = true;
                    fprintf(stderr,
                            "[V4l2M2mEncoder] frame layout not dmabuf-"
                            "importable (kind=%u format=%u planes=%u stride "
                            "%u/%u/%u vs encoder %u) — using copy fallback\n",
                            static_cast<unsigned>(frame.kind),
                            static_cast<unsigned>(frame.format),
                            frame.plane_count, frame.planes[0].stride,
                            frame.planes[1].stride, frame.planes[2].stride,
                            input_stride_);
                }
                if (free_pool_slots_.empty()) {
                    freeInputIndices.push(idx);
                    frames_dropped++;
                    fprintf(stderr,
                            "[V4l2M2mEncoder] dropped frame (no free "
                            "fallback buffer), total_dropped=%d\n",
                            frames_dropped);
                    return true;
                }
                const int slot = free_pool_slots_.back();
                free_pool_slots_.pop_back();
                PoolBuffer& pool = fallback_pool_[slot];
                size_t src_size = 0;
                std::string copy_err;
                if (!copy_frame_into(frame,
                                     static_cast<uint8_t*>(pool.start),
                                     pool.length, &src_size, &copy_err)) {
                    free_pool_slots_.push_back(slot);
                    freeInputIndices.push(idx);
                    if (err) *err = copy_err;
                    return false;
                }
                if (!queue_dmabuf_input(idx, pool.fd, pool.length, src_size,
                                        frame.pts_us, err)) {
                    free_pool_slots_.push_back(slot);
                    freeInputIndices.push(idx);
                    return false;
                }
                in_flight_.push_back({nullptr, nullptr, 0, slot});
            }
        } else {
            auto* destination = static_cast<uint8_t*>(inputBuffers[idx].start);
            size_t src_size = 0;
            std::string copy_err;
            if (!copy_frame_into(frame, destination, inputBuffers[idx].length,
                                 &src_size, &copy_err)) {
                return fail_input(copy_err.c_str());
            }

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

            if (ioctl(fd, VIDIOC_QBUF, &buf_in) < 0) {
                // Return the buffer index to the free pool on queue failure.
                freeInputIndices.push(idx);
                if (err) *err = std::string("QBUF (OUTPUT) failed: ") + strerror(errno);
                return false;
            }
        }

        frames_injected++;
        if (frames_injected % 60 == 0) {
            fprintf(stderr, "[V4l2M2mEncoder] stats: injected=%d dropped=%d\n",
                    frames_injected, frames_dropped);
        }
    } else {
        frames_dropped++;
        fprintf(stderr, "[V4l2M2mEncoder] dropped frame (no free input buffer), total_dropped=%d\n",
                frames_dropped);
    }
    return true;
}

// Copy a frame into an encoder-layout buffer (input_stride_, single plane).
// Used by the Mmap input path and by the Dmabuf mode copy fallback.  The
// source plane pointers must be valid for the duration of the call.
bool V4l2M2mEncoder::copy_frame_into(
    const RawFrame& frame, uint8_t* destination, size_t destination_length,
    size_t* out_size, std::string* err) const {
    size_t src_size = 0;
    if (input_format_ == RawPixelFormat::Nv12) {
        if (frame.format != RawPixelFormat::Nv12 || frame.plane_count < 2
            || frame.planes[0].data == nullptr
            || frame.planes[1].data == nullptr) {
            if (err) *err = "NV12 encoder input requires Y and UV planes";
            return false;
        }
        const size_t y_destination_bytes =
            static_cast<size_t>(input_stride_) * height;
        const size_t uv_destination_bytes =
            static_cast<size_t>(input_stride_) * (height / 2);
        src_size = y_destination_bytes + uv_destination_bytes;
        if (src_size > destination_length) {
            if (err) *err = "NV12 frame exceeds encoder input buffer";
            return false;
        }
        if (frame.planes[0].stride < width || frame.planes[1].stride < width
            || frame.planes[0].bytes
                   < static_cast<size_t>(frame.planes[0].stride) * height
            || frame.planes[1].bytes
                   < static_cast<size_t>(frame.planes[1].stride) * (height / 2)) {
            if (err) *err = "NV12 frame plane is shorter than its declared stride";
            return false;
        }
        memset(destination, 0, src_size);
        for (uint32_t row = 0; row < height; ++row) {
            memcpy(destination + static_cast<size_t>(row) * input_stride_,
                   frame.planes[0].data
                       + static_cast<size_t>(row) * frame.planes[0].stride,
                   width);
        }
        uint8_t* destination_uv = destination + y_destination_bytes;
        for (uint32_t row = 0; row < height / 2; ++row) {
            memcpy(destination_uv + static_cast<size_t>(row) * input_stride_,
                   frame.planes[1].data
                       + static_cast<size_t>(row) * frame.planes[1].stride,
                   width);
        }
    } else if (input_format_ == RawPixelFormat::Uyvy422) {
        if (frame.format != RawPixelFormat::Uyvy422
            || frame.plane_count != 1
            || frame.planes[0].data == nullptr) {
            if (err) *err = "UYVY encoder input requires one packed plane";
            return false;
        }
        const uint32_t row_bytes = width * 2;
        const uint32_t source_stride = frame.planes[0].stride
            ? frame.planes[0].stride : row_bytes;
        src_size = input_frame_bytes_;
        if (source_stride < row_bytes
            || frame.planes[0].bytes
                   < static_cast<size_t>(source_stride) * height) {
            if (err) *err = "UYVY frame plane is shorter than its declared stride";
            return false;
        }
        if (src_size > destination_length) {
            if (err) *err = "UYVY frame exceeds encoder input buffer";
            return false;
        }
        memset(destination, 0, src_size);
        for (uint32_t row = 0; row < height; ++row) {
            memcpy(destination + static_cast<size_t>(row) * input_stride_,
                   frame.planes[0].data
                       + static_cast<size_t>(row) * source_stride,
                   row_bytes);
        }
    } else {
        if (frame.format != RawPixelFormat::Yuv420p
            || (frame.plane_count != 1 && frame.plane_count != 3)
            || frame.planes[0].data == nullptr) {
            if (err) *err = "YUV420P encoder input requires one or three planes";
            return false;
        }

        const uint32_t destination_chroma_stride = input_stride_ / 2;
        const size_t destination_y_bytes =
            static_cast<size_t>(input_stride_) * height;
        const size_t destination_chroma_bytes =
            static_cast<size_t>(destination_chroma_stride) * (height / 2);
        src_size = destination_y_bytes + destination_chroma_bytes * 2;
        if (src_size > destination_length) {
            if (err) *err = "YUV420P frame exceeds encoder input buffer";
            return false;
        }

        const uint8_t* source_y = frame.planes[0].data;
        const uint32_t source_y_stride =
            frame.planes[0].stride ? frame.planes[0].stride : width;
        const uint8_t* source_u = nullptr;
        const uint8_t* source_v = nullptr;
        uint32_t source_u_stride = 0;
        uint32_t source_v_stride = 0;

        if (frame.plane_count == 3) {
            if (frame.planes[1].data == nullptr || frame.planes[2].data == nullptr) {
                if (err) *err = "YUV420P chroma plane is null";
                return false;
            }
            source_u = frame.planes[1].data;
            source_v = frame.planes[2].data;
            source_u_stride =
                frame.planes[1].stride ? frame.planes[1].stride : width / 2;
            source_v_stride =
                frame.planes[2].stride ? frame.planes[2].stride : width / 2;
            if (frame.planes[0].bytes
                    < static_cast<size_t>(source_y_stride) * height
                || frame.planes[1].bytes
                    < static_cast<size_t>(source_u_stride) * (height / 2)
                || frame.planes[2].bytes
                    < static_cast<size_t>(source_v_stride) * (height / 2)) {
                if (err) *err = "YUV420P plane is shorter than its declared stride";
                return false;
            }
        } else {
            const uint32_t source_chroma_stride = source_y_stride / 2;
            const size_t source_y_bytes =
                static_cast<size_t>(source_y_stride) * height;
            const size_t source_chroma_bytes =
                static_cast<size_t>(source_chroma_stride) * (height / 2);
            if (frame.planes[0].bytes
                < source_y_bytes + source_chroma_bytes * 2) {
                if (err) *err = "contiguous YUV420P frame is too short";
                return false;
            }
            source_u = source_y + source_y_bytes;
            source_v = source_u + source_chroma_bytes;
            source_u_stride = source_chroma_stride;
            source_v_stride = source_chroma_stride;
        }

        memset(destination, 0, src_size);
        for (uint32_t row = 0; row < height; ++row) {
            memcpy(destination + static_cast<size_t>(row) * input_stride_,
                   source_y + static_cast<size_t>(row) * source_y_stride,
                   width);
        }
        uint8_t* destination_u = destination + destination_y_bytes;
        uint8_t* destination_v = destination_u + destination_chroma_bytes;
        for (uint32_t row = 0; row < height / 2; ++row) {
            memcpy(destination_u
                       + static_cast<size_t>(row) * destination_chroma_stride,
                   source_u + static_cast<size_t>(row) * source_u_stride,
                   width / 2);
            memcpy(destination_v
                       + static_cast<size_t>(row) * destination_chroma_stride,
                   source_v + static_cast<size_t>(row) * source_v_stride,
                   width / 2);
        }
    }
    if (out_size) *out_size = src_size;
    return true;
}

bool V4l2M2mEncoder::queue_dmabuf_input(
    int idx, int dma_fd, size_t length, size_t bytes_used, uint64_t pts_us,
    std::string* err) {
    struct v4l2_buffer buf = {};
    struct v4l2_plane planes[1] = {};
    buf.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    buf.memory = V4L2_MEMORY_DMABUF;
    buf.index = idx;
    buf.length = 1;
    buf.m.planes = planes;
    planes[0].m.fd = dma_fd;
    planes[0].length = length;
    planes[0].bytesused = bytes_used;
    buf.timestamp.tv_sec = pts_us / 1000000;
    buf.timestamp.tv_usec = pts_us % 1000000;

    if (ioctl(fd, VIDIOC_QBUF, &buf) < 0) {
        if (err) *err = std::string("QBUF (OUTPUT, DMABUF) failed: ") + strerror(errno);
        return false;
    }
    return true;
}

void V4l2M2mEncoder::release_in_flight_head() {
    if (in_flight_.empty()) {
        fprintf(stderr, "[V4l2M2mEncoder] OUTPUT DQBUF with empty in-flight queue\n");
        return;
    }
    InFlightEntry entry = in_flight_.front();
    in_flight_.pop_front();
    if (entry.release) {
        entry.release(entry.release_ctx, entry.buffer_index);
    }
    if (entry.pool_slot >= 0) {
        free_pool_slots_.push_back(entry.pool_slot);
    }
}

// Copy-fallback pool for Dmabuf mode: CPU-writable dma-bufs from the
// kernel dma_heap allocator (CMA first, system heap as fallback), so any
// frame can still be queued with one copy when it cannot be imported.
bool V4l2M2mEncoder::allocate_fallback_pool(unsigned int count) {
    const char* heap_paths[] = {"/dev/dma_heap/linux,cma", "/dev/dma_heap/system"};
    for (const char* path : heap_paths) {
        dma_heap_fd_ = open(path, O_RDWR | O_CLOEXEC);
        if (dma_heap_fd_ >= 0) break;
    }
    if (dma_heap_fd_ < 0) return false;

    for (unsigned int i = 0; i < count; i++) {
        struct dma_heap_allocation_data data = {};
        data.len = input_frame_bytes_;
        data.fd_flags = O_RDWR | O_CLOEXEC;
        if (ioctl(dma_heap_fd_, DMA_HEAP_IOCTL_ALLOC, &data) < 0 || data.fd < 0) {
            fprintf(stderr, "[V4l2M2mEncoder] dma_heap alloc failed: %s\n",
                    strerror(errno));
            return !fallback_pool_.empty();
        }
        void* start = mmap(nullptr, input_frame_bytes_, PROT_READ | PROT_WRITE,
                           MAP_SHARED, data.fd, 0);
        if (start == MAP_FAILED) {
            fprintf(stderr, "[V4l2M2mEncoder] dma_heap mmap failed: %s\n",
                    strerror(errno));
            close(data.fd);
            return !fallback_pool_.empty();
        }
        fallback_pool_.push_back({data.fd, start, input_frame_bytes_});
        free_pool_slots_.push_back(static_cast<int>(i));
    }
    return true;
}

void V4l2M2mEncoder::requestKeyframe() {
    force_idr.store(true);
}

// Runtime bitrate retune (adaptive bitrate control, issue #409): the
// MPEG_VIDEO_BITRATE control may be set at any point after streaming
// starts; the driver applies it to subsequently encoded frames.
bool V4l2M2mEncoder::setBitrate(uint32_t bps) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (fd < 0 || !running_.load()) return false;

    struct v4l2_control ctrl = {};
    ctrl.id = V4L2_CID_MPEG_VIDEO_BITRATE;
    ctrl.value = static_cast<int>(bps);
    if (ioctl(fd, VIDIOC_S_CTRL, &ctrl) < 0) {
        fprintf(stderr, "[V4l2M2mEncoder] setBitrate(%u) failed: %s\n", bps, strerror(errno));
        return false;
    }
    bitrate = bps;
    fprintf(stderr, "[V4l2M2mEncoder] bitrate -> %u bps\n", bps);
    return true;
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
    std::lock_guard<std::mutex> lock(mutex_);
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
    // Dmabuf mode: hand any capture buffers still held in the encoder back
    // to the capture backend (a no-op for it once streaming stopped), then
    // release the fallback pool.  Entries already completed via OUTPUT DQBUF
    // were popped at dequeue time, so each buffer is released exactly once.
    while (!in_flight_.empty()) {
        InFlightEntry entry = in_flight_.front();
        in_flight_.pop_front();
        if (entry.release) {
            entry.release(entry.release_ctx, entry.buffer_index);
        }
    }
    for (auto& pool : fallback_pool_) {
        if (pool.start && pool.start != MAP_FAILED) munmap(pool.start, pool.length);
        if (pool.fd >= 0) close(pool.fd);
    }
    fallback_pool_.clear();
    free_pool_slots_.clear();
    if (dma_heap_fd_ >= 0) {
        close(dma_heap_fd_);
        dma_heap_fd_ = -1;
    }
    input_mode_ = InputMode::Mmap;
    dmabuf_fallback_logged_ = false;
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
