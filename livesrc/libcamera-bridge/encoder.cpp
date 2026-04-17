#include "encoder.h"
#include <iostream>
#include <vector>
#include <cstring>
#include <fcntl.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <linux/videodev2.h>
#include <cstdio>
#include <errno.h>
#include <queue>

class Encoder::Impl {
public:
    int fd = -1;
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t fps = 0;
    uint32_t bitrate = 0;
    
    NALCallbackFFI nalCallback = nullptr;
    void* userData = nullptr;
    std::string errorMsg;
    
    struct Buffer {
        void* start;
        size_t length;
    };
    
    std::vector<Buffer> inputBuffers;
    std::vector<Buffer> outputBuffers;
    std::queue<int> freeInputIndices;
    bool force_idr = false;
    int frames_injected = 0;
    int frames_dropped = 0;

    bool setupV4L2(const CameraParams& params);
    void cleanup();
};

Encoder::Encoder() : pImpl(std::make_unique<Impl>()) {}

Encoder::~Encoder() {
    pImpl->cleanup();
}

void Encoder::Impl::cleanup() {
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
}

bool Encoder::init(const CameraParams& params) {
    return pImpl->setupV4L2(params);
}

bool Encoder::Impl::setupV4L2(const CameraParams& params) {
    fprintf(stderr, "[Encoder] Initializing V4L2 M2M (V14.9-TURBO)...\n");
    fd = open("/dev/video11", O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) return false;

    width = params.width;
    height = params.height;
    fps = params.fps;
    bitrate = params.bitrate;

    struct v4l2_format fmt = {};
    fmt.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    fmt.fmt.pix_mp.width = width;
    fmt.fmt.pix_mp.height = height;
    fmt.fmt.pix_mp.pixelformat = V4L2_PIX_FMT_YUV420;
    fmt.fmt.pix_mp.field = V4L2_FIELD_NONE;
    fmt.fmt.pix_mp.num_planes = 1;

    if (ioctl(fd, VIDIOC_S_FMT, &fmt) < 0) return false;

    fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    fmt.fmt.pix_mp.pixelformat = V4L2_PIX_FMT_H264;
    if (ioctl(fd, VIDIOC_S_FMT, &fmt) < 0) return false;

    struct v4l2_control ctrl = {};
    ctrl.id = V4L2_CID_MPEG_VIDEO_BITRATE;
    ctrl.value = bitrate;
    ioctl(fd, VIDIOC_S_CTRL, &ctrl);

    ctrl.id = V4L2_CID_MPEG_VIDEO_H264_I_PERIOD;
    ctrl.value = fps * 2;
    ioctl(fd, VIDIOC_S_CTRL, &ctrl);

    ctrl.id = V4L2_CID_MPEG_VIDEO_REPEAT_SEQ_HEADER;
    ctrl.value = 1;
    ioctl(fd, VIDIOC_S_CTRL, &ctrl);

    // V14.9 TURBO Depth: 8 (Essential for 30FPS H264 on Pi 5)
    struct v4l2_requestbuffers req = {};
    req.count = 8;
    req.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    req.memory = V4L2_MEMORY_MMAP;
    if (ioctl(fd, VIDIOC_REQBUFS, &req) < 0) return false;

    for (unsigned int i = 0; i < req.count; i++) {
        struct v4l2_buffer buf = {};
        struct v4l2_plane planes[1] = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        buf.length = 1;
        buf.m.planes = planes;
        if (ioctl(fd, VIDIOC_QUERYBUF, &buf) < 0) return false;
        void* start = mmap(NULL, planes[0].length, PROT_READ | PROT_WRITE, MAP_SHARED, fd, planes[0].m.mem_offset);
        inputBuffers.push_back({start, planes[0].length});
        freeInputIndices.push(i);
    }

    req.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    if (ioctl(fd, VIDIOC_REQBUFS, &req) < 0) return false;
    for (unsigned int i = 0; i < req.count; i++) {
        struct v4l2_buffer buf = {};
        struct v4l2_plane planes[1] = {};
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        buf.length = 1;
        buf.m.planes = planes;
        if (ioctl(fd, VIDIOC_QUERYBUF, &buf) < 0) return false;
        void* start = mmap(NULL, planes[0].length, PROT_READ | PROT_WRITE, MAP_SHARED, fd, planes[0].m.mem_offset);
        outputBuffers.push_back({start, planes[0].length});
        if (ioctl(fd, VIDIOC_QBUF, &buf) < 0) return false;
    }

    enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    ioctl(fd, VIDIOC_STREAMON, &type);
    type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    ioctl(fd, VIDIOC_STREAMON, &type);

    fprintf(stderr, "[Encoder] Turbo Pipeline Primed (Queue: 8)\n");
    return true;
}

void Encoder::encode(const uint8_t* data, size_t size, uint64_t timestamp) {
    if (pImpl->fd < 0) return;

    // 1. DRAIN OUTPUT POOL
    struct v4l2_buffer buf_out = {};
    struct v4l2_plane planes_out[1] = {};
    buf_out.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    buf_out.memory = V4L2_MEMORY_MMAP;
    buf_out.length = 1;
    buf_out.m.planes = planes_out;

    while (ioctl(pImpl->fd, VIDIOC_DQBUF, &buf_out) == 0) {
        if (pImpl->nalCallback) {
            uint8_t* encoded_data = static_cast<uint8_t*>(pImpl->outputBuffers[buf_out.index].start);
            size_t encoded_size = planes_out[0].bytesused;
            int is_keyframe = (buf_out.flags & V4L2_BUF_FLAG_KEYFRAME) ? 1 : 0;
            
            uint64_t ts_us = (uint64_t)buf_out.timestamp.tv_sec * 1000000 + buf_out.timestamp.tv_usec;
            pImpl->nalCallback(encoded_data, encoded_size, is_keyframe, ts_us, pImpl->userData);
        }
        ioctl(pImpl->fd, VIDIOC_QBUF, &buf_out);
    }

    // 2. RECLAIM INPUT POOL
    struct v4l2_buffer buf_in = {};
    struct v4l2_plane planes_in[1] = {};
    buf_in.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    buf_in.memory = V4L2_MEMORY_MMAP;
    buf_in.length = 1;
    buf_in.m.planes = planes_in;

    while (ioctl(pImpl->fd, VIDIOC_DQBUF, &buf_in) == 0) {
        pImpl->freeInputIndices.push(buf_in.index);
    }

    // 3. FEED INPUT (Prioritize Queue depth)
    if (!pImpl->freeInputIndices.empty()) {
        int target_index = pImpl->freeInputIndices.front();
        pImpl->freeInputIndices.pop();

        memcpy(pImpl->inputBuffers[target_index].start, data, size);
        
        buf_in.index = target_index;
        planes_in[0].bytesused = size;
        buf_in.timestamp.tv_sec = timestamp / 1000000;
        buf_in.timestamp.tv_usec = timestamp % 1000000;

        if (pImpl->force_idr) {
            struct v4l2_control ctrl = {};
            ctrl.id = V4L2_CID_MPEG_VIDEO_FORCE_KEY_FRAME;
            ctrl.value = 1;
            ioctl(pImpl->fd, VIDIOC_S_CTRL, &ctrl);
            pImpl->force_idr = false;
        }

        if (ioctl(pImpl->fd, VIDIOC_QBUF, &buf_in) == 0) {
            pImpl->frames_injected++;
            if (pImpl->frames_injected % 30 == 0) {
                fprintf(stderr, "[Encoder] IN: Frame %d (Turbo mode)\n", pImpl->frames_injected);
            }
        }
    } else {
        pImpl->frames_dropped++;
        if (pImpl->frames_dropped % 30 == 0) {
            fprintf(stderr, "[Encoder] WARNING: Buffer exhaustion, dropping frame %d!\n", pImpl->frames_dropped);
        }
    }
}


void Encoder::setNALCallback(NALCallbackFFI callback, void* user_data) {
    pImpl->nalCallback = callback;
    pImpl->userData = user_data;
}

void Encoder::requestKeyframe() {
    pImpl->force_idr = true;
}

const char* Encoder::getError() const {
    return pImpl->errorMsg.c_str();
}

void Encoder::stop() {
    pImpl->cleanup();
}
