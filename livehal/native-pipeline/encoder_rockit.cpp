//! Rockchip RKMPI/Rockit hardware encoder backend for RV110x (Luckfox Pico).
//!
//! Uses the RKMPI VENC API (RK_MPI_VENC_*), NOT standard Rockchip MPP.
//! Synchronous model: SendFrame blocks, GetStream blocks.
//!
//! References:
//!   Luckfox SDK: media/samples/simple_test/simple_vi_bind_venc.c

#include "include/encoder_backend.h"

// Inline Annex-B NAL scanner
static inline size_t nal_start_code_len(const uint8_t* p, size_t size) {
    if (!p || size < 3) return 0;
    if (p[0] == 0x00 && p[1] == 0x00 && p[2] == 0x01) return 3;
    if (size >= 4 && p[0] == 0x00 && p[1] == 0x00
        && p[2] == 0x00 && p[3] == 0x01) return 4;
    return 0;
}

struct NalFlags { bool keyframe; bool config; };

static inline NalFlags detect_h264_flags(const uint8_t* data, size_t size) {
    NalFlags f{false, false};
    for (size_t i = 0; i + 3 < size; ++i) {
        size_t sc = nal_start_code_len(data + i, size - i);
        if (!sc || i + sc >= size) continue;
        int t = (data[i + sc] & 0x1F);
        if (t == 5) f.keyframe = true;
        else if (t == 7 || t == 8) f.config = true;
        i += sc;
    }
    return f;
}

static inline NalFlags detect_h265_flags(const uint8_t* data, size_t size) {
    NalFlags f{false, false};
    for (size_t i = 0; i + 4 < size; ++i) {
        size_t sc = nal_start_code_len(data + i, size - i);
        if (!sc) continue;
        if (i + sc + 2 > size) continue;
        int t = (data[i + sc] >> 1) & 0x3F;
        if (t >= 16 && t <= 21) f.keyframe = true;
        else if (t == 32 || t == 33 || t == 34) f.config = true;
        i += sc;
    }
    return f;
}

#include <rk_mpi_sys.h>
#include <rk_mpi_venc.h>
#include <rk_mpi_mb.h>
#include <rk_mpi_mmz.h>
#include <rk_comm_venc.h>
#include <rk_comm_video.h>

#include <cstdio>
#include <cstring>
#include <mutex>
#include <atomic>
#include <memory>
#include <thread>
#include <unordered_map>
#include <vector>
#include <unistd.h>

// ---------------------------------------------------------------------------
// RkRockitEncoder — synchronous RKMPI VENC backend
// ---------------------------------------------------------------------------

class RkRockitEncoder : public EncoderBackend {
public:
    RkRockitEncoder() = default;

    ~RkRockitEncoder() override { stop(); }

    bool init(const EncoderConfig& cfg, std::string* err) override {
        std::lock_guard<std::mutex> lock(mutex_);

        if (cfg.codec != VideoCodec::H264 && cfg.codec != VideoCodec::H265) {
            if (err) *err = "encoder-rockit: only H.264 and H.265 supported";
            return false;
        }

        width_  = cfg.width;
        height_ = cfg.height;
        bitrate_ = cfg.bitrate;
        gop_    = cfg.gop;
        codec_  = cfg.codec;
        fps_    = cfg.fps;
        prefer_dmabuf_ = cfg.prefer_dmabuf;

        // Align to 16 for MPP hardware requirements
        vir_w_ = (width_  + 15) & ~15U;
        vir_h_ = (height_ + 15) & ~15U;
        frame_size_ = static_cast<size_t>(vir_w_) * vir_h_ * 3 / 2;

        // --- System init (before any VENC calls) ---
        RK_S32 sys_ret = RK_MPI_SYS_Init();
        std::fprintf(stderr,
            "[RkRockit] SYS_Init ret=0x%x\n",
            static_cast<unsigned>(sys_ret));

        // --- VENC channel attribute ---
        // Field set mirrors the SDK sample (sample_comm_venc.c): max pic
        // dims and RC frame-rate fields are mandatory there; leaving them
        // zero makes the VENC kernel ioctl fail on RV1106.
        VENC_CHN_ATTR_S attr;
        memset(&attr, 0, sizeof(attr));

        const bool is_h265 = (codec_ == VideoCodec::H265);
        attr.stVencAttr.enType = is_h265 ? RK_VIDEO_ID_HEVC : RK_VIDEO_ID_AVC;
        attr.stVencAttr.u32MaxPicWidth  = width_;
        attr.stVencAttr.u32MaxPicHeight = height_;
        attr.stVencAttr.enPixelFormat = RK_FMT_YUV420SP;
        if (!is_h265)
            attr.stVencAttr.u32Profile = H264E_PROFILE_HIGH;
        attr.stVencAttr.u32PicWidth  = width_;
        attr.stVencAttr.u32PicHeight = height_;
        attr.stVencAttr.u32VirWidth  = vir_w_;
        attr.stVencAttr.u32VirHeight = vir_h_;
        attr.stVencAttr.u32StreamBufCnt = 3;
        attr.stVencAttr.u32BufSize = frame_size_;

        // VENC_H265_CBR_S is a typedef of VENC_H264_CBR_S in this SDK.
        VENC_H264_CBR_S* cbr = is_h265
            ? &attr.stRcAttr.stH265Cbr
            : &attr.stRcAttr.stH264Cbr;
        attr.stRcAttr.enRcMode =
            is_h265 ? VENC_RC_MODE_H265CBR : VENC_RC_MODE_H264CBR;
        cbr->u32Gop = gop_;
        cbr->u32BitRate = bitrate_ / 1000;
        cbr->u32SrcFrameRateNum = fps_;
        cbr->u32SrcFrameRateDen = 1;
        cbr->fr32DstFrameRateNum = fps_;
        cbr->fr32DstFrameRateDen = 1;

        RK_S32 ret = RK_MPI_VENC_CreateChn(0, &attr);
        std::fprintf(stderr,
            "[RkRockit] CreateChn ret=0x%x\n",
            static_cast<unsigned>(ret));
        if (ret != RK_SUCCESS) {
            if (err) *err = "encoder-rockit: CreateChn failed ret="
                + std::to_string(ret);
            return false;
        }

        // QP bounds — same spirit as the rkmpp backend (qp 18..40) and
        // sample_comm_venc.c. Without them the RV1106 default max QP leaves
        // motion scenes visibly blurry at moderate bitrates.
        VENC_RC_PARAM_S rcParam;
        memset(&rcParam, 0, sizeof(rcParam));
        if (is_h265) {
            rcParam.stParamH265.u32MinQp  = 18;
            rcParam.stParamH265.u32MaxQp  = 40;
            rcParam.stParamH265.u32MinIQp = 18;
            rcParam.stParamH265.u32MaxIQp = 40;
            rcParam.stParamH265.u32FrmMinQp  = 28;
            rcParam.stParamH265.u32FrmMinIQp = 28;
        } else {
            rcParam.stParamH264.u32MinQp  = 18;
            rcParam.stParamH264.u32MaxQp  = 40;
            rcParam.stParamH264.u32MinIQp = 18;
            rcParam.stParamH264.u32MaxIQp = 40;
            rcParam.stParamH264.u32FrmMinQp  = 28;
            rcParam.stParamH264.u32FrmMinIQp = 28;
        }
        ret = RK_MPI_VENC_SetRcParam(0, &rcParam);
        if (ret != RK_SUCCESS) {
            RK_MPI_VENC_DestroyChn(0);
            if (err) *err = "encoder-rockit: SetRcParam failed ret="
                + std::to_string(ret);
            return false;
        }

        // Start receiving frames
        VENC_RECV_PIC_PARAM_S recvParam;
        memset(&recvParam, 0, sizeof(recvParam));
        recvParam.s32RecvPicNum = -1;
        ret = RK_MPI_VENC_StartRecvFrame(0, &recvParam);
        std::fprintf(stderr,
            "[RkRockit] StartRecvFrame ret=0x%x\n",
            static_cast<unsigned>(ret));
        if (ret != RK_SUCCESS) {
            RK_MPI_VENC_DestroyChn(0);
            if (err) *err = "encoder-rockit: StartRecvFrame failed ret="
                + std::to_string(ret);
            return false;
        }

        // Allocate input frame buffers straight from the system MMZ heap.
        // Matches the SDK samples that hand user frames to other modules
        // (simple_vi_get_frame_tde.c): VENC accepts MB_BLKs from
        // RK_MPI_SYS_MmzAlloc directly, while a private MB pool with
        // CMA/NOCACHE flags is rejected by the VENC kernel ioctl (EFAULT).
        for (int i = 0; i < kInputPoolSize; i++) {
            RK_S32 aret = RK_MPI_SYS_MmzAlloc(
                &blk_[i], RK_NULL, RK_NULL, frame_size_);
            if (aret != RK_SUCCESS || blk_[i] == RK_NULL) {
                for (int j = 0; j < i; j++)
                    RK_MPI_SYS_MmzFree(blk_[j]);
                RK_MPI_VENC_StopRecvFrame(0);
                RK_MPI_VENC_DestroyChn(0);
                if (err) *err = "encoder-rockit: MmzAlloc failed ret="
                    + std::to_string(aret);
                return false;
            }
        }

        std::fprintf(stderr,
            "[RkRockitEncoder] %dx%d fps=%d bps=%d gop=%d codec=%s "
            "stride=%dx%d mmz=%dx%zu\n",
            static_cast<int>(width_), static_cast<int>(height_),
            static_cast<int>(fps_), static_cast<int>(bitrate_),
            static_cast<int>(gop_),
            (codec_ == VideoCodec::H265) ? "h265" : "h264",
            static_cast<int>(vir_w_), static_cast<int>(vir_h_),
            kInputPoolSize, frame_size_);

        running_  = true;
        // Dedicated output thread (same model as sample_comm_venc.c):
        // GetStream blocks there, never on the capture/submit path.
        stream_thread_ = std::thread(&RkRockitEncoder::streamLoop, this);
        return true;
    }

    bool submit(const RawFrame& frame, std::string* err) override {
        std::lock_guard<std::mutex> lock(mutex_);

        if (!running_) {
            if (err) *err = "encoder-rockit: not running";
            return false;
        }
        if (frame.plane_count < 2) {
            if (err) *err = "encoder-rockit: NV12 requires >=2 planes";
            return false;
        }

        submitted_frames_++;

        // --- Zero-copy path: hand the capture dmabuf straight to VENC ---
        // Same model as encoder_rkmpp.cpp: import each capture fd once via
        // RK_MPI_MMZ_Fd2Handle and cache it.  The capture requeues the V4L2
        // buffer when submit() returns, so before reusing an fd we must be
        // sure VENC has consumed the frame we previously submitted on it.
        MB_BLK frame_blk = RK_NULL;
        uint32_t frame_vir_w = vir_w_;
        uint32_t frame_vir_h = vir_h_;
        if (prefer_dmabuf_ && frame.kind == BufferKind::DmaBuf
            && frame.planes[0].dma_fd >= 0 && frame.planes[0].stride > 0
            && frame.planes[1].offset > 0
            && frame.planes[1].offset % frame.planes[0].stride == 0) {
            const int fd = frame.planes[0].dma_fd;
            auto it = dmabuf_cache_.find(fd);
            if (it == dmabuf_cache_.end()) {
                MB_BLK blk = RK_MPI_MMZ_Fd2Handle(fd);
                if (blk != RK_NULL) {
                    it = dmabuf_cache_.emplace(fd, DmabufEntry{blk, 0}).first;
                    std::fprintf(stderr,
                        "[RkRockit] dmabuf import ok fd=%d (zero-copy)\n", fd);
                } else {
                    static int zc_errs = 0;
                    if (++zc_errs <= 3)
                        std::fprintf(stderr,
                            "[RkRockit] Fd2Handle failed fd=%d, "
                            "CPU copy fallback\n", fd);
                }
            }
            if (it != dmabuf_cache_.end()) {
                // Buffer-lifetime rule: this fd's previous frame must be
                // drained before the ISP may overwrite the buffer.
                DmabufEntry& entry = it->second;
                if (entry.last_submit_encoded > 0) {
                    for (int retry = 0;
                         retry < 3
                            && encoded_frames_.load() <= entry.last_submit_encoded;
                         ++retry)
                        usleep(5000);
                    if (encoded_frames_.load() <= entry.last_submit_encoded)
                        std::fprintf(stderr,
                            "[RkRockit] warn: dmabuf fd=%d still in use "
                            "by VENC\n", fd);
                }
                entry.last_submit_encoded = encoded_frames_.load();
                frame_blk = entry.blk;
                frame_vir_w = frame.planes[0].stride;
                // UV sits at planes[1].offset = stride * vir_h in the
                // capture's contiguous NV12 layout.
                frame_vir_h =
                    frame.planes[1].offset / frame.planes[0].stride;
                zc_frames_++;
            }
        }

        if (!frame_blk) {
            // --- CPU fallback: copy NV12 into the next MMZ buffer ---
            int idx = next_idx_;
            next_idx_ = (next_idx_ + 1) % kInputPoolSize;

            void* dst = RK_MPI_MB_Handle2VirAddr(blk_[idx]);
            if (!dst) {
                if (err) *err = "encoder-rockit: null buffer ptr";
                return false;
            }

            uint32_t src_stride = frame.planes[0].stride > 0
                ? frame.planes[0].stride : width_;
            uint8_t* dst_y = static_cast<uint8_t*>(dst);
            const uint8_t* src_y = frame.planes[0].data;

            // Y plane
            if (src_stride == vir_w_) {
                memcpy(dst_y, src_y, vir_w_ * height_);
            } else {
                for (uint32_t r = 0; r < height_; r++) {
                    memcpy(dst_y, src_y, width_);
                    src_y += src_stride;
                    dst_y += vir_w_;
                }
            }

            // UV plane (NV12 interleaved)
            dst_y = static_cast<uint8_t*>(dst) + vir_w_ * vir_h_;
            src_y = frame.planes[1].data;
            src_stride = frame.planes[1].stride > 0
                ? frame.planes[1].stride : width_;
            if (src_stride == vir_w_) {
                memcpy(dst_y, src_y, vir_w_ * height_ / 2);
            } else {
                for (uint32_t r = 0; r < height_ / 2; r++) {
                    memcpy(dst_y, src_y, width_);
                    src_y += src_stride;
                    dst_y += vir_w_;
                }
            }

            // CPU wrote the frame: flush the dma-buf so VENC sees the data
            // (sample_comm_venc.c does this before every SendFrame).
            RK_MPI_SYS_MmzFlushCache(blk_[idx], RK_FALSE);
            frame_blk = blk_[idx];
        }

        // Force a keyframe when requested (new subscribers send PLI/FIR).
        // RKMPI needs the explicit API; VIDEO_FRAME flags do nothing.
        if (force_idr_.exchange(false)) {
            std::fprintf(stderr, "[RkRockit] RequestIDR (subscriber PLI)\n");
            RK_MPI_VENC_RequestIDR(0, RK_TRUE);
        }

        // --- Send frame to encoder ---
        VIDEO_FRAME_INFO_S frameInfo;
        memset(&frameInfo, 0, sizeof(frameInfo));
        frameInfo.stVFrame.u32Width  = width_;
        frameInfo.stVFrame.u32Height = height_;
        frameInfo.stVFrame.u32VirWidth  = frame_vir_w;
        frameInfo.stVFrame.u32VirHeight = frame_vir_h;
        frameInfo.stVFrame.enPixelFormat = RK_FMT_YUV420SP;
        frameInfo.stVFrame.enCompressMode = COMPRESS_MODE_NONE;
        frameInfo.stVFrame.pMbBlk = frame_blk;
        frameInfo.stVFrame.u64PTS = frame.pts_us;

        RK_S32 ret = RK_MPI_VENC_SendFrame(0, &frameInfo, -1);
        if (ret != RK_SUCCESS) {
            send_failures_++;
            static int sf_errs = 0;
            if (++sf_errs <= 3)
                std::fprintf(stderr,
                    "[RkRockit] SendFrame err ret=0x%x\n",
                    static_cast<unsigned>(ret));
            if (err) *err = "encoder-rockit: SendFrame failed ret="
                + std::to_string(ret);
            return false;
        }

        return true;
    }

    void requestKeyframe() override {
        std::lock_guard<std::mutex> lock(mutex_);
        if (running_) force_idr_ = true;
    }

    void stop() override {
        {
            std::lock_guard<std::mutex> lock(mutex_);
            if (!running_) return;
            running_ = false;
        }
        // Stop the output thread before tearing down the channel.
        if (stream_thread_.joinable()) stream_thread_.join();

        std::lock_guard<std::mutex> lock(mutex_);

        RK_MPI_VENC_StopRecvFrame(0);
        RK_MPI_VENC_DestroyChn(0);

        for (int i = 0; i < kInputPoolSize; i++) {
            if (blk_[i]) { RK_MPI_SYS_MmzFree(blk_[i]); blk_[i] = RK_NULL; }
        }

        RK_MPI_SYS_Exit();

        std::fprintf(stderr,
            "[RkRockitEncoder] Final: subm=%llu enc=%llu fail=%llu\n",
            static_cast<unsigned long long>(submitted_frames_),
            static_cast<unsigned long long>(encoded_frames_),
            static_cast<unsigned long long>(send_failures_));
    }

    bool isRunning() const override { return running_.load(); }

    void setCallback(EncodedPacketCallback cb) override {
        std::lock_guard<std::mutex> lock(mutex_);
        encoded_cb_ = std::move(cb);
    }

private:
    static constexpr int kInputPoolSize = 2;

    // Output pump running on its own thread. RKMPI allows SendFrame and
    // GetStream from different threads (that is how the SDK samples work).
    void streamLoop() {
        VENC_STREAM_S stream;
        memset(&stream, 0, sizeof(stream));

        VENC_PACK_S pack;
        memset(&pack, 0, sizeof(pack));
        stream.pstPack = &pack;

        bool first = true;
        while (running_) {
            RK_S32 ret = RK_MPI_VENC_GetStream(0, &stream, 100);
            if (ret != RK_SUCCESS) {
                if (first) {
                    first = false;
                    VENC_CHN_STATUS_S st;
                    memset(&st, 0, sizeof(st));
                    RK_MPI_VENC_QueryStatus(0, &st);
                    std::fprintf(stderr,
                        "[RkRockit] chn status leftPics=%d "
                        "leftStream=%d curPacks=%d GetStream=0x%x\n",
                        static_cast<int>(st.u32LeftPics),
                        static_cast<int>(st.u32LeftStreamFrames),
                        static_cast<int>(st.u32CurPacks),
                        static_cast<unsigned>(ret));
                }
                continue;
            }

            if (stream.u32PackCount > 0 && stream.pstPack->u32Len > 0) {
                void* data =
                    RK_MPI_MB_Handle2VirAddr(stream.pstPack->pMbBlk);
                uint32_t size = stream.pstPack->u32Len;

                if (data && size && encoded_cb_) {
                    const auto* raw =
                        static_cast<const uint8_t*>(data);
                    NalFlags nf = (codec_ == VideoCodec::H265)
                        ? detect_h265_flags(raw, size)
                        : detect_h264_flags(raw, size);

                    EncodedPacket out;
                    out.codec = codec_;
                    out.data   = raw;
                    out.size   = size;
                    out.pts_us = stream.pstPack->u64PTS;
                    out.dts_us = out.pts_us;
                    out.flags  = 0;
                    if (nf.keyframe) out.flags |= EncodedKeyframe;
                    if (nf.config)   out.flags |= EncodedConfig;
                    encoded_cb_(out);
                    encoded_frames_++;

                    // Periodic stats
                    if (encoded_frames_.load() % 150 == 0) {
                        std::fprintf(stderr,
                            "[RkRockitEncoder] subm=%llu enc=%llu "
                            "fail=%llu zc=%llu\n",
                            static_cast<unsigned long long>(
                                submitted_frames_.load()),
                            static_cast<unsigned long long>(
                                encoded_frames_.load()),
                            static_cast<unsigned long long>(
                                send_failures_),
                            static_cast<unsigned long long>(zc_frames_));
                    }
                }
            }
            RK_MPI_VENC_ReleaseStream(0, &stream);
        }
    }

    uint32_t width_ = 0, height_ = 0, fps_ = 0, bitrate_ = 0;
    uint32_t gop_ = 60;
    VideoCodec codec_ = VideoCodec::H264;
    uint32_t vir_w_ = 0, vir_h_ = 0;
    size_t frame_size_ = 0;
    bool prefer_dmabuf_ = false;

    // RKMPI memory (system MMZ heap, one MB_BLK per slot)
    MB_BLK  blk_[kInputPoolSize] = {};
    int     next_idx_ = 0;

    // Zero-copy path: imported MB_BLK per capture dmabuf fd. The capture
    // owns the memory; last_submit_encoded is the encoded-frame counter
    // snapshot at this fd's previous SendFrame, used to prove VENC has
    // consumed that frame before the ISP may reuse the buffer.
    struct DmabufEntry {
        MB_BLK blk;
        uint64_t last_submit_encoded;
    };
    std::unordered_map<int, DmabufEntry> dmabuf_cache_;

    std::atomic<bool> running_{false};
    std::atomic<bool> force_idr_{false};
    std::thread stream_thread_;

    std::atomic<uint64_t> submitted_frames_{0};
    std::atomic<uint64_t> encoded_frames_{0};
    uint64_t send_failures_    = 0;
    uint64_t zc_frames_        = 0;

    EncodedPacketCallback encoded_cb_;
    mutable std::mutex mutex_;
};

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

std::shared_ptr<EncoderBackend> create_rockit_encoder_backend(
    const EncoderConfig& /*cfg*/) {
    return std::make_shared<RkRockitEncoder>();
}
