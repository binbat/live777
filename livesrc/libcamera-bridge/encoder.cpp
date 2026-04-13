#include "encoder.h"
#include <linux/videodev2.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <fcntl.h>
#include <unistd.h>
#include <cstring>
#include <iostream>
#include <vector>
#include <cerrno>

#define NUM_BUFFERS 4

class Encoder::Impl {
public:
    int fd = -1;
    int width = 0;
    int height = 0;
    int fps = 0;
    int bitrate = 0;
    NALCallback nalCallback;
    std::string errorMsg;
    
    struct Buffer {
        void* start = nullptr;
        size_t length = 0;
    };
    
    std::vector<Buffer> inputBuffers;
    std::vector<Buffer> outputBuffers;
    
    bool findH264Encoder();
    bool configureEncoder();
    bool allocateBuffers();
    bool startStreaming();
    void stopStreaming();
};

Encoder::Encoder() : pImpl(std::make_unique<Impl>()) {}

Encoder::~Encoder() {
    if (pImpl->fd >= 0) {
        pImpl->stopStreaming();
        
        // Unmap buffers
        for (auto& buf : pImpl->inputBuffers) {
            if (buf.start) {
                munmap(buf.start, buf.length);
            }
        }
        for (auto& buf : pImpl->outputBuffers) {
            if (buf.start) {
                munmap(buf.start, buf.length);
            }
        }
        
        close(pImpl->fd);
    }
}

bool Encoder::Impl::findH264Encoder() {
    // Try common encoder devices
    const char* devices[] = {
        "/dev/video11",  // bcm2835-codec on Raspberry Pi
        "/dev/video10",
        "/dev/video12",
    };
    
    for (const char* device : devices) {
        int test_fd = open(device, O_RDWR);
        if (test_fd < 0) {
            continue;
        }
        
        // Check if it's an M2M device with H.264 support
        struct v4l2_capability cap;
        if (ioctl(test_fd, VIDIOC_QUERYCAP, &cap) == 0) {
            // Check for M2M device
            if ((cap.device_caps & V4L2_CAP_VIDEO_M2M_MPLANE) ||
                (cap.device_caps & V4L2_CAP_VIDEO_M2M)) {
                
                // Check for H.264 encoder support
                struct v4l2_fmtdesc fmt;
                memset(&fmt, 0, sizeof(fmt));
                fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
                
                while (ioctl(test_fd, VIDIOC_ENUM_FMT, &fmt) == 0) {
                    if (fmt.pixelformat == V4L2_PIX_FMT_H264) {
                        fd = test_fd;
                        std::cerr << "Found H.264 encoder: " << device << "\n";
                        return true;
                    }
                    fmt.index++;
                }
            }
        }
        
        close(test_fd);
    }
    
    errorMsg = "No H.264 encoder found";
    return false;
}

bool Encoder::Impl::configureEncoder() {
    // Set OUTPUT (input) format - YUV420
    struct v4l2_format fmt_in;
    memset(&fmt_in, 0, sizeof(fmt_in));
    fmt_in.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    fmt_in.fmt.pix_mp.width = width;
    fmt_in.fmt.pix_mp.height = height;
    fmt_in.fmt.pix_mp.pixelformat = V4L2_PIX_FMT_YUV420;
    fmt_in.fmt.pix_mp.field = V4L2_FIELD_NONE;
    fmt_in.fmt.pix_mp.num_planes = 1;
    
    if (ioctl(fd, VIDIOC_S_FMT, &fmt_in) < 0) {
        errorMsg = "Failed to set input format";
        return false;
    }
    
    // Set CAPTURE (output) format - H.264
    struct v4l2_format fmt_out;
    memset(&fmt_out, 0, sizeof(fmt_out));
    fmt_out.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    fmt_out.fmt.pix_mp.width = width;
    fmt_out.fmt.pix_mp.height = height;
    fmt_out.fmt.pix_mp.pixelformat = V4L2_PIX_FMT_H264;
    fmt_out.fmt.pix_mp.field = V4L2_FIELD_NONE;
    fmt_out.fmt.pix_mp.num_planes = 1;
    
    if (ioctl(fd, VIDIOC_S_FMT, &fmt_out) < 0) {
        errorMsg = "Failed to set output format";
        return false;
    }
    
    // Set framerate
    struct v4l2_streamparm parm;
    memset(&parm, 0, sizeof(parm));
    parm.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    parm.parm.output.timeperframe.numerator = 1;
    parm.parm.output.timeperframe.denominator = fps;
    
    if (ioctl(fd, VIDIOC_S_PARM, &parm) < 0) {
        std::cerr << "Warning: Failed to set framerate\n";
    }
    
    // Set bitrate
    struct v4l2_control ctrl;
    ctrl.id = V4L2_CID_MPEG_VIDEO_BITRATE;
    ctrl.value = bitrate;
    
    if (ioctl(fd, VIDIOC_S_CTRL, &ctrl) < 0) {
        std::cerr << "Warning: Failed to set bitrate\n";
    }
    
    // Set H.264 profile
    ctrl.id = V4L2_CID_MPEG_VIDEO_H264_PROFILE;
    ctrl.value = V4L2_MPEG_VIDEO_H264_PROFILE_HIGH;
    ioctl(fd, VIDIOC_S_CTRL, &ctrl);  // Ignore errors
    
    // Set H.264 level
    ctrl.id = V4L2_CID_MPEG_VIDEO_H264_LEVEL;
    ctrl.value = V4L2_MPEG_VIDEO_H264_LEVEL_4_0;
    ioctl(fd, VIDIOC_S_CTRL, &ctrl);  // Ignore errors
    
    // Request repeat sequence headers
    ctrl.id = V4L2_CID_MPEG_VIDEO_REPEAT_SEQ_HEADER;
    ctrl.value = 1;
    ioctl(fd, VIDIOC_S_CTRL, &ctrl);  // Ignore errors
    
    return true;
}

bool Encoder::Impl::allocateBuffers() {
    // Request INPUT buffers
    struct v4l2_requestbuffers req_in;
    memset(&req_in, 0, sizeof(req_in));
    req_in.count = NUM_BUFFERS;
    req_in.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    req_in.memory = V4L2_MEMORY_MMAP;
    
    if (ioctl(fd, VIDIOC_REQBUFS, &req_in) < 0) {
        errorMsg = "Failed to request input buffers";
        return false;
    }
    
    // Map INPUT buffers
    inputBuffers.resize(NUM_BUFFERS);
    for (unsigned int i = 0; i < NUM_BUFFERS; i++) {
        struct v4l2_buffer buf;
        struct v4l2_plane planes[1];
        memset(&buf, 0, sizeof(buf));
        memset(planes, 0, sizeof(planes));
        
        buf.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        buf.m.planes = planes;
        buf.length = 1;
        
        if (ioctl(fd, VIDIOC_QUERYBUF, &buf) < 0) {
            errorMsg = "Failed to query input buffer";
            return false;
        }
        
        inputBuffers[i].length = planes[0].length;
        inputBuffers[i].start = mmap(nullptr, planes[0].length,
                                     PROT_READ | PROT_WRITE, MAP_SHARED,
                                     fd, planes[0].m.mem_offset);
        
        if (inputBuffers[i].start == MAP_FAILED) {
            errorMsg = "Failed to mmap input buffer";
            return false;
        }
    }
    
    // Request OUTPUT buffers
    struct v4l2_requestbuffers req_out;
    memset(&req_out, 0, sizeof(req_out));
    req_out.count = NUM_BUFFERS;
    req_out.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    req_out.memory = V4L2_MEMORY_MMAP;
    
    if (ioctl(fd, VIDIOC_REQBUFS, &req_out) < 0) {
        errorMsg = "Failed to request output buffers";
        return false;
    }
    
    // Map OUTPUT buffers
    outputBuffers.resize(NUM_BUFFERS);
    for (unsigned int i = 0; i < NUM_BUFFERS; i++) {
        struct v4l2_buffer buf;
        struct v4l2_plane planes[1];
        memset(&buf, 0, sizeof(buf));
        memset(planes, 0, sizeof(planes));
        
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        buf.m.planes = planes;
        buf.length = 1;
        
        if (ioctl(fd, VIDIOC_QUERYBUF, &buf) < 0) {
            errorMsg = "Failed to query output buffer";
            return false;
        }
        
        outputBuffers[i].length = planes[0].length;
        outputBuffers[i].start = mmap(nullptr, planes[0].length,
                                      PROT_READ | PROT_WRITE, MAP_SHARED,
                                      fd, planes[0].m.mem_offset);
        
        if (outputBuffers[i].start == MAP_FAILED) {
            errorMsg = "Failed to mmap output buffer";
            return false;
        }
    }
    
    return true;
}

bool Encoder::Impl::startStreaming() {
    // Queue all input buffers as empty (ready to receive data)
    for (unsigned int i = 0; i < NUM_BUFFERS; i++) {
        struct v4l2_buffer buf;
        struct v4l2_plane planes[1];
        memset(&buf, 0, sizeof(buf));
        memset(planes, 0, sizeof(planes));
        
        buf.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        buf.m.planes = planes;
        buf.length = 1;
        planes[0].bytesused = 0;  // Empty buffer initially
        
        if (ioctl(fd, VIDIOC_QBUF, &buf) < 0) {
            errorMsg = "Failed to queue input buffer";
            return false;
        }
    }
    
    // Queue all output buffers
    for (unsigned int i = 0; i < NUM_BUFFERS; i++) {
        struct v4l2_buffer buf;
        struct v4l2_plane planes[1];
        memset(&buf, 0, sizeof(buf));
        memset(planes, 0, sizeof(planes));
        
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        buf.m.planes = planes;
        buf.length = 1;
        
        if (ioctl(fd, VIDIOC_QBUF, &buf) < 0) {
            errorMsg = "Failed to queue output buffer";
            return false;
        }
    }
    
    // Start streaming
    int type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    if (ioctl(fd, VIDIOC_STREAMON, &type) < 0) {
        errorMsg = "Failed to start output streaming";
        return false;
    }
    
    type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    if (ioctl(fd, VIDIOC_STREAMON, &type) < 0) {
        errorMsg = "Failed to start capture streaming";
        return false;
    }
    
    std::cerr << "V4L2 M2M encoder streaming started\n";
    return true;
}

void Encoder::Impl::stopStreaming() {
    int type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    ioctl(fd, VIDIOC_STREAMOFF, &type);
    
    type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
    ioctl(fd, VIDIOC_STREAMOFF, &type);
}

bool Encoder::init(const EncoderParams& params) {
    pImpl->width = params.width;
    pImpl->height = params.height;
    pImpl->fps = params.fps;
    pImpl->bitrate = params.bitrate;
    
    if (!pImpl->findH264Encoder()) {
        return false;
    }
    
    if (!pImpl->configureEncoder()) {
        return false;
    }
    
    if (!pImpl->allocateBuffers()) {
        return false;
    }
    
    if (!pImpl->startStreaming()) {
        return false;
    }
    
    return true;
}

bool Encoder::encode(const uint8_t* data, size_t size, uint64_t timestamp) {
    (void)timestamp;  // TODO: Use timestamp
    
    static int frame_count = 0;
    frame_count++;
    
    if (frame_count == 1) {
        std::cerr << "Encoder: First frame, size=" << size << "\n";
    }
    
    // Step 1: Dequeue an input buffer (should be available from startup queuing)
    struct v4l2_buffer buf_in;
    struct v4l2_plane planes_in[1];
    memset(&buf_in, 0, sizeof(buf_in));
    memset(planes_in, 0, sizeof(planes_in));
    
    buf_in.type = V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE;
    buf_in.memory = V4L2_MEMORY_MMAP;
    buf_in.m.planes = planes_in;
    buf_in.length = 1;
    
    if (ioctl(pImpl->fd, VIDIOC_DQBUF, &buf_in) < 0) {
        if (frame_count <= 5) {
            std::cerr << "Encoder: DQBUF input failed on frame " << frame_count << "\n";
        }
        return false;
    }
    
    if (frame_count == 1) {
        std::cerr << "Encoder: Successfully dequeued input buffer\n";
    }
    
    // Step 2: Copy YUV data to the input buffer
    size_t copy_size = std::min(size, pImpl->inputBuffers[buf_in.index].length);
    memcpy(pImpl->inputBuffers[buf_in.index].start, data, copy_size);
    planes_in[0].bytesused = copy_size;
    
    // Step 3: Queue the filled input buffer back for encoding
    if (ioctl(pImpl->fd, VIDIOC_QBUF, &buf_in) < 0) {
        pImpl->errorMsg = "Failed to queue input buffer";
        std::cerr << "Encoder: QBUF input failed!\n";
        return false;
    }
    
    if (frame_count == 1) {
        std::cerr << "Encoder: Input buffer queued with " << copy_size << " bytes\n";
    }
    
    // Step 4: Try to dequeue encoded output (H.264)
    // An M2M encoder can produce multiple capture buffers from one input (e.g. SPS, PPS, IDR).
    // MUST loop DQBUF until EAGAIN, otherwise the capture queue fills up and stalls!
    int flags = fcntl(pImpl->fd, F_GETFL, 0);
    fcntl(pImpl->fd, F_SETFL, flags | O_NONBLOCK);

    while (true) {
        struct v4l2_buffer buf_out;
        struct v4l2_plane planes_out[1];
        memset(&buf_out, 0, sizeof(buf_out));
        memset(planes_out, 0, sizeof(planes_out));
        
        buf_out.type = V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE;
        buf_out.memory = V4L2_MEMORY_MMAP;
        buf_out.m.planes = planes_out;
        buf_out.length = 1;
        
        if (ioctl(pImpl->fd, VIDIOC_DQBUF, &buf_out) < 0) {
            // EAGAIN means we drained all available output buffers
            if (errno != EAGAIN && frame_count <= 10) {
                std::cerr << "Encoder: DQBUF output failed on frame " << frame_count 
                          << " (normal for first ~5 frames)\n";
            }
            break;
        }
        
        // Step 5: Process the encoded H.264 data
        uint8_t* encoded_data = static_cast<uint8_t*>(pImpl->outputBuffers[buf_out.index].start);
        size_t encoded_size = planes_out[0].bytesused;
        bool is_keyframe = (buf_out.flags & V4L2_BUF_FLAG_KEYFRAME) != 0;
        
        // Reduce log spam, only log large frames or keyframes
        if (is_keyframe || encoded_size > 1000) {
            std::cerr << "Encoder: Got H.264 buffer, size=" << encoded_size 
                      << ", keyframe=" << (is_keyframe ? "yes" : "no") << "\n";
        }
        
        // Step 6: Call the NAL callback to output H.264
        if (pImpl->nalCallback && encoded_size > 0) {
            pImpl->nalCallback(encoded_data, encoded_size, is_keyframe);
        } else if (encoded_size == 0) {
            std::cerr << "Encoder: WARNING - Encoded size is 0!\n";
        }
        
        // Step 7: Re-queue the output buffer for reuse
        if (ioctl(pImpl->fd, VIDIOC_QBUF, &buf_out) < 0) {
            pImpl->errorMsg = "Failed to requeue output buffer";
            std::cerr << "Encoder: Failed to requeue output buffer!\n";
            fcntl(pImpl->fd, F_SETFL, flags); // Restore before returning
            return false;
        }
    }
    
    // Restore blocking mode
    fcntl(pImpl->fd, F_SETFL, flags);
    
    return true;
}

void Encoder::setNALCallback(NALCallback callback) {
    pImpl->nalCallback = std::move(callback);
}

const char* Encoder::getError() const {
    return pImpl->errorMsg.c_str();
}

bool Encoder::forceKeyframe() {
    if (pImpl->fd < 0) {
        pImpl->errorMsg = "Encoder not initialized";
        return false;
    }
    
    struct v4l2_control ctrl;
    ctrl.id = V4L2_CID_MPEG_VIDEO_FORCE_KEY_FRAME;
    ctrl.value = 1;
    
    if (ioctl(pImpl->fd, VIDIOC_S_CTRL, &ctrl) < 0) {
        std::cerr << "Warning: Failed to force keyframe (device may not support this control)" << std::endl;
        // Don't treat this as fatal error
        return true;
    }
    
    std::cerr << "✓ Keyframe requested" << std::endl;
    return true;
}
