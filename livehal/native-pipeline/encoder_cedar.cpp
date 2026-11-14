//! Allwinner cedar VE hardware H.264 encoder backend (A733 / sun60iw2).
//!
//! Uses the closed-source libvencoder.so C API. CPU-copy NV12 input path only
//! (no DMA-BUF zero-copy yet; "make it work first").
//!
//! The real SDK implementation is compiled only when SUNXI_HAS_CEDAR_SDK is
//! defined by CMake (i.e. SUNXI_SYSROOT supplies vencoder.h). Without the SDK
//! headers this translation unit falls back to a self-contained stub, so the
//! skeleton still builds on any Linux host (CI / no-sysroot checks). Phase 4
//! cross builds and the device always define the macro.
//!
//! Per-frame flow (synchronous, mirrors encoder_rdk.cpp):
//!   GetOneAllocInputBuffer -> copy Y/UV -> FlushCacheAllocInputBuffer ->
//!   AddOneInputBuffer -> VideoEncodeOneFrame -> drain bitstream ->
//!   AlreadyUsedInputBuffer/ReturnOneAllocInputBuffer.
//! The capture side (V4L2 + Allwinner ISP) supplies NV12M frames; see
//! sunxi_isp.cpp for the ISP lifecycle that must run before STREAMON.

#include "include/encoder_backend.h"

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <mutex>
#include <string>
#include <vector>

#if defined(SUNXI_HAS_CEDAR_SDK)
#include <vencoder.h>
#include <memoryAdapter.h>
#include <veAdapter.h>
#endif

namespace {

// ---------------------------------------------------------------------------
// H.264 Annex-B NAL flags (shared logic with encoder_rdk.cpp:53-98).
// ---------------------------------------------------------------------------
bool is_annex_b_start_code(const uint8_t* data, size_t pos, size_t len,
                            size_t* start_len) {
    if (pos + 4 <= len && data[pos] == 0 && data[pos + 1] == 0 &&
        data[pos + 2] == 0 && data[pos + 3] == 1) {
        *start_len = 4;
        return true;
    }
    if (pos + 3 <= len && data[pos] == 0 && data[pos + 1] == 0 && data[pos + 2] == 1) {
        *start_len = 3;
        return true;
    }
    return false;
}

uint32_t detect_h264_flags(const uint8_t* data, size_t len) {
    uint32_t flags = 0;
    size_t pos = 0;
    while (pos < len) {
        size_t start_len = 0;
        if (!is_annex_b_start_code(data, pos, len, &start_len)) {
            ++pos;
            continue;
        }
        const size_t nal_start = pos + start_len;
        if (nal_start >= len) break;
        const uint8_t nal_type = data[nal_start] & 0x1F;
        if (nal_type == 5) {
            flags |= static_cast<uint32_t>(EncodedKeyframe);
        } else if (nal_type == 7 || nal_type == 8) {
            flags |= static_cast<uint32_t>(EncodedConfig);
        }
        pos = nal_start;
    }
    return flags;
}

#if defined(SUNXI_HAS_CEDAR_SDK)
// Copy one NV12 plane row-by-row, tolerating a capture stride that differs
// from the encoder's 16-aligned stride.
void copy_plane(uint8_t* dst, uint32_t dst_stride, const uint8_t* src,
                uint32_t src_stride, uint32_t width, uint32_t rows) {
    for (uint32_t row = 0; row < rows; ++row) {
        std::memcpy(dst + static_cast<size_t>(row) * dst_stride,
                    src + static_cast<size_t>(row) * src_stride, width);
    }
}

class CedarEncoder : public EncoderBackend {
public:
    ~CedarEncoder() override { stop(); }

    bool init(const EncoderConfig& cfg, std::string* err) override {
        width_ = cfg.width;
        height_ = cfg.height;
        fps_ = cfg.fps;
        codec_ = cfg.codec;
        stride_ = (width_ + 15u) & ~15u;

        if (codec_ != VideoCodec::H264) {
            if (err) *err = "cedar backend currently supports H.264 only";
            return false;
        }
        if (cfg.input_format != RawPixelFormat::Nv12) {
            if (err) *err = "cedar backend expects NV12 input (RawPixelFormat::Nv12)";
            return false;
        }
        if ((width_ & 1u) || (height_ & 1u)) {
            if (err) *err = "cedar H.264 requires even width and height";
            return false;
        }

        enc_ = VideoEncCreate(VENC_CODEC_H264);
        if (!enc_) {
            if (err) *err = "cedar: VideoEncCreate(H264) failed";
            return false;
        }

        // cedar snapshots the H.264 parameters into SPS/PPS at VideoEncInit()
        // time. These MUST be set before init: with the zero/default values the
        // encoder emits a placeholder parameter set that does not match the
        // actual slices, so decoders fail at the first macroblock and conceal
        // the picture as green/gray. All-skip (flat) frames happen to decode,
        // which masks the problem in synthetic tests. Verified on A733: the
        // resulting out-of-band SPS/PPS (High/L5.1) makes the stream decode
        // with zero errors.
        VencH264Param h264{};
        h264.nFramerate = static_cast<int>(cfg.fps);
        h264.nSrcFramerate = static_cast<int>(cfg.fps);
        h264.nBitrate = static_cast<int>(cfg.bitrate);
        h264.nMaxKeyInterval = cfg.gop > 0 ? static_cast<int>(cfg.gop)
                                           : static_cast<int>(cfg.fps);
        h264.bEntropyCodingCABAC = 1;
        h264.sProfileLevel.nProfile = VENC_H264ProfileHigh;
        h264.sProfileLevel.nLevel = VENC_H264Level51;
        h264.sQPRange.nMaxqp = 51;
        h264.sQPRange.nMinqp = 10;
        h264.sQPRange.nMaxPqp = 51;
        h264.sQPRange.nMinPqp = 10;
        h264.sQPRange.nQpInit = 26;
        if (VideoEncSetParameter(enc_, VENC_IndexParamH264Param, &h264) != 0) {
            if (err) *err = "cedar: failed to set H264 params before init";
            destroy();
            return false;
        }

        VencBaseConfig base{};
        base.bEncH264Nalu = 0;  // Annex-B bytestream (start codes), default
        base.nInputWidth = width_;
        base.nInputHeight = height_;
        base.nDstWidth = width_;
        base.nDstHeight = height_;
        base.nStride = stride_;
        base.eInputFormat = VENC_PIXEL_YUV420SP;  // NV12
        base.memops = MemAdapterGetOpsS();        // libMemAdapter.so
        base.veOpsS = GetVeOpsS(0);              // libVE.so, generic VE ops
        base.pVeOpsSelf = nullptr;

        if (VideoEncInit(enc_, &base) != 0) {
            if (err) *err = "cedar: VideoEncInit failed";
            destroy();
            return false;
        }
        initialized_ = true;

        int br = static_cast<int>(cfg.bitrate);
        VideoEncSetParameter(enc_, VENC_IndexParamBitrate, &br);
        int fr = static_cast<int>(cfg.fps);
        VideoEncSetParameter(enc_, VENC_IndexParamFramerate, &fr);
        if (cfg.gop > 0) {
            int gop = static_cast<int>(cfg.gop);
            VideoEncSetParameter(enc_, VENC_IndexParamMaxKeyInterval, &gop);
        }

        VencAllocateBufferParam ab{};
        ab.nBufferNum = kInputBufferCount;
        ab.nSizeY = stride_ * height_;
        ab.nSizeC = stride_ * height_ / 2;
        if (AllocInputBuffer(enc_, &ab) != 0) {
            if (err) *err = "cedar: AllocInputBuffer failed";
            destroy();
            return false;
        }

        // cedar emits SPS/PPS out-of-band and does NOT prepend them to IDR
        // frames. Cache the Annex-B header and prepend it to every keyframe so
        // a WHEP subscriber can initialise its decoder at any join point.
        VencHeaderData header{};
        if (VideoEncGetParameter(enc_, VENC_IndexParamH264SPSPPS, &header) == 0
            && header.pBuffer && header.nLength > 0 && header.nLength <= 4096) {
            sps_pps_.assign(header.pBuffer, header.pBuffer + header.nLength);
            std::fprintf(stderr, "[CedarEncoder] cached SPS/PPS header: %zu bytes\n",
                         sps_pps_.size());
        } else {
            std::fprintf(stderr,
                         "[CedarEncoder] WARNING: could not read SPS/PPS header; "
                         "in-band keyframes will lack parameter sets\n");
        }

        running_.store(true);
        std::fprintf(stderr,
                     "[CedarEncoder] H264 %ux%u @%u fps, bitrate=%u, gop=%u, stride=%u\n",
                     width_, height_, fps_, cfg.bitrate, cfg.gop, stride_);
        return true;
    }

    bool submit(const RawFrame& frame, std::string* err) override {
        // CPU-copy path never retains the capture buffer; the guard honours the
        // zero-copy release contract on every exit path.
        FrameReleaseGuard release_guard(frame);
        std::lock_guard<std::mutex> lock(mutex_);

        if (!enc_ || !running_.load()) {
            if (err) *err = "cedar: encoder not running";
            return false;
        }
        if (frame.kind == BufferKind::DmaBuf) {
            if (err) *err = "cedar: DMA-BUF zero-copy not implemented; use CPU path";
            return false;
        }
        if (frame.format != RawPixelFormat::Nv12 || frame.plane_count < 2
            || !frame.planes[0].data || !frame.planes[1].data) {
            if (err) *err = "cedar: expected a 2-plane NV12 CPU frame";
            return false;
        }
        if (frame.width != width_ || frame.height != height_) {
            if (err) *err = "cedar: frame dimensions do not match encoder config";
            return false;
        }

        // A pending PLI/FIR forces a keyframe on the next submitted frame.
        if (force_keyframe_.exchange(false)) {
            int force = 1;
            VideoEncSetParameter(enc_, VENC_IndexParamForceKeyFrame, &force);
        }

        VencInputBuffer in{};
        if (GetOneAllocInputBuffer(enc_, &in) != 0 || !in.pAddrVirY || !in.pAddrVirC) {
            if (err) *err = "cedar: GetOneAllocInputBuffer failed / null addresses";
            return false;
        }

        const uint32_t y_stride = frame.planes[0].stride ? frame.planes[0].stride : width_;
        const uint32_t c_stride = frame.planes[1].stride ? frame.planes[1].stride : width_;
        copy_plane(in.pAddrVirY, stride_, frame.planes[0].data, y_stride, width_, height_);
        copy_plane(in.pAddrVirC, stride_, frame.planes[1].data, c_stride, width_, height_ / 2);
        in.nPts = static_cast<long long>(frame.pts_us);
        FlushCacheAllocInputBuffer(enc_, &in);

        if (AddOneInputBuffer(enc_, &in) != 0) {
            if (err) *err = "cedar: AddOneInputBuffer failed";
            ReturnOneAllocInputBuffer(enc_, &in);
            return false;
        }
        const int encode_rc = VideoEncodeOneFrame(enc_);

        // Recycle the consumed input buffer back into the SDK pool.
        VencInputBuffer used{};
        if (AlreadyUsedInputBuffer(enc_, &used) == 0) {
            ReturnOneAllocInputBuffer(enc_, &used);
        }
        if (encode_rc != 0) {
            if (err) *err = "cedar: VideoEncodeOneFrame failed";
            return false;
        }
        return drain(frame.pts_us, err);
    }

    void requestKeyframe() override { force_keyframe_.store(true); }

    bool setBitrate(uint32_t bps) override {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!enc_ || !running_.load()) return false;
        int br = static_cast<int>(bps);
        return VideoEncSetParameter(enc_, VENC_IndexParamBitrate, &br) == 0;
    }

    void stop() override {
        std::lock_guard<std::mutex> lock(mutex_);
        running_.store(false);
        destroy();
    }

    bool isRunning() const override { return running_.load(); }

    void setCallback(EncodedPacketCallback cb) override {
        std::lock_guard<std::mutex> lock(mutex_);
        encoded_cb_ = std::move(cb);
    }

private:
    static constexpr uint32_t kInputBufferCount = 4;
    static constexpr int kMaxDrainPerFrame = 16;

    // Pull every currently-available access unit and dispatch via the callback.
    // NOTE: cedar's VideoEncodeOneFrame is expected to complete synchronously;
    // Phase 5 must confirm on-device. If frames arrive late, a dedicated drain
    // thread (see encoder_rkmpp.cpp:959-1122) becomes necessary.
    bool drain(uint64_t pts_us, std::string* err) {
        for (int i = 0; i < kMaxDrainPerFrame; ++i) {
            if (ValidBitstreamFrameNum(enc_) <= 0) return true;
            VencOutputBuffer out{};
            if (GetOneBitstreamFrame(enc_, &out) != 0) return true;

            const bool keyframe = (out.nFlag & VENC_BUFFERFLAG_KEYFRAME) != 0;
            au_.clear();
            // cedar keeps SPS/PPS out-of-band: prepend the cached header to
            // every IDR so subscribers can decode after joining mid-stream.
            if (keyframe && !sps_pps_.empty()) {
                au_.insert(au_.end(), sps_pps_.begin(), sps_pps_.end());
            }
            if (out.pData0 && out.nSize0) {
                au_.insert(au_.end(), out.pData0, out.pData0 + out.nSize0);
            }
            if (out.pData1 && out.nSize1) {
                au_.insert(au_.end(), out.pData1, out.pData1 + out.nSize1);
            }
            if (out.pData2 && out.nSize2) {
                au_.insert(au_.end(), out.pData2, out.pData2 + out.nSize2);
            }
            if (!au_.empty() && au_.size() <= 16u * 1024u * 1024u && encoded_cb_) {
                uint32_t flags = detect_h264_flags(au_.data(), au_.size());
                if (keyframe) {
                    flags |= static_cast<uint32_t>(EncodedKeyframe);
                }
                EncodedPacket pkt{};
                pkt.codec = codec_;
                pkt.data = au_.data();  // valid only during the callback
                pkt.size = au_.size();
                pkt.pts_us = pts_us;
                pkt.dts_us = pts_us;
                pkt.flags = flags;
                encoded_cb_(pkt);
            }
            FreeOneBitStreamFrame(enc_, &out);
        }
        (void)err;
        return true;
    }

    // Caller holds mutex_. Idempotent teardown of the SDK encoder.
    void destroy() {
        if (!enc_) return;
        if (initialized_) {
            ReleaseAllocInputBuffer(enc_);
            VideoEncUnInit(enc_);
            initialized_ = false;
        }
        VideoEncDestroy(enc_);
        enc_ = nullptr;
    }

    VideoEncoder* enc_ = nullptr;  // VideoEncoder is void*, so this is void**
    uint32_t width_ = 0;
    uint32_t height_ = 0;
    uint32_t fps_ = 0;
    uint32_t stride_ = 0;
    VideoCodec codec_ = VideoCodec::H264;
    bool initialized_ = false;
    std::atomic<bool> running_{false};
    std::atomic<bool> force_keyframe_{false};
    std::mutex mutex_;
    EncodedPacketCallback encoded_cb_;
    std::vector<uint8_t> au_;       // scratch for assembling pData0/1/2 into one AU
    std::vector<uint8_t> sps_pps_;  // out-of-band Annex-B SPS/PPS, prepended to IDRs
};
#else
// ---------------------------------------------------------------------------
// Stub (no SUNXI_SYSROOT headers): keeps the skeleton compilable on any host.
// ---------------------------------------------------------------------------
class CedarEncoder : public EncoderBackend {
public:
    bool init(const EncoderConfig& cfg, std::string* err) override {
        (void)cfg;
        if (err) {
            *err = "cedar encoder backend: SDK headers not available at build "
                   "time (SUNXI_HAS_CEDAR_SDK not defined); rebuild with SUNXI_SYSROOT";
        }
        return false;
    }
    bool submit(const RawFrame& frame, std::string* err) override {
        (void)frame;
        if (err) *err = "cedar encoder backend is not available (stub build)";
        return false;
    }
    void requestKeyframe() override {}
    void stop() override {}
    bool isRunning() const override { return false; }
    void setCallback(EncodedPacketCallback cb) override { (void)cb; }
};
#endif

}  // namespace

std::shared_ptr<EncoderBackend> create_cedar_encoder_backend(const EncoderConfig& cfg) {
    (void)cfg;
    return std::make_shared<CedarEncoder>();
}
