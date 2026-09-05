//! Rockchip MPP (Media Process Platform) hardware encoder backend.
//!
//! Targets Rockchip SoCs with an MPP encoder (RK3588, RK3588S, RK3568,
//! RV1126B, etc.) using the rockchip_mpp userspace library.
//!
//! Supported codecs:
//!   - H.264 (Baseline / Main / High)
//!   - H.265 (Main)
//!
//! Input: NV12 only (CPU copy path, or DMA-BUF zero-copy when both capture
//! and encoder run with prefer_dmabuf).
//! Profile/level/tier are resolved by Rust and passed as numeric values.
//!
//! Buffer model (persistent pool, partition aggregation):
//!   A small pool of persistent input buffers allocated at init and cycled
//!   each frame.  Each frame gets its own buffer slot.  MPP may produce
//!   multiple packets/partitions per frame — these are aggregated until EOI
//!   (end-of-image) and dispatched as a single complete Access Unit.
//!   This matches GStreamer mpph265enc behaviour and fixes screen tearing
//!   caused by per-partition dispatch.
//!
//! Zero-copy lifetime (deferred V4L2 requeue, issue #410):
//!   A DmaBuf input frame carries an armed release contract (see
//!   media_types.h).  submit() takes ownership of the capture buffer and
//!   pushes it onto `in_flight_` after a successful encode_put_frame().
//!   MPP drains a single encoder context in submission order, so every
//!   observed frame completion (EOI) releases the oldest held buffer back
//!   to the capture for requeue — the camera driver can never overwrite a
//!   buffer the encoder is still reading.  Every non-retaining path (input
//!   validation failure, copy fallback, put failure) releases immediately
//!   via an RAII guard, and stop() force-releases the remainder after
//!   mpi_->reset() (the hardware no longer reads inputs at that point).
//!   Invariant: in_flight_.size() == submitted - put_failures - completed.
//!
//! References:
//!   https://github.com/rockchip-linux/mpp

#include "include/encoder_backend.h"
#include "include/latency_stats.h"

#include <chrono>
#include <thread>
#include <unordered_map>

#include <unistd.h>

// ---------------------------------------------------------------------------
// Inline Annex-B NAL scanner
// ---------------------------------------------------------------------------

static inline size_t annex_b_start_code_len(const uint8_t* p, size_t size) {
    if (!p || size < 3) return 0;
    if (p[0] == 0x00 && p[1] == 0x00 && p[2] == 0x01) return 3;
    if (size >= 4 && p[0] == 0x00 && p[1] == 0x00
        && p[2] == 0x00 && p[3] == 0x01) return 4;
    return 0;
}

static inline int h264_nal_type(const uint8_t* p, size_t len) {
    if (!p || len < 4) return -1;
    size_t o = annex_b_start_code_len(p, len);
    if (o == 0 || o >= len) return -1;
    return p[o] & 0x1F;
}

struct NalFlags { bool keyframe; bool config; };

static inline NalFlags detect_h264_flags(const uint8_t* data, size_t size) {
    NalFlags f{false, false};
    for (size_t i = 0; i + 3 < size; ++i) {
        size_t sc = annex_b_start_code_len(data + i, size - i);
        if (!sc) continue;
        int t = h264_nal_type(data + i, size - i);
        if (t == 5) f.keyframe = true;
        else if (t == 7 || t == 8) f.config = true;
        i += sc;
    }
    return f;
}

static inline NalFlags detect_h265_flags(const uint8_t* data, size_t size) {
    NalFlags f{false, false};
    for (size_t i = 0; i + 4 < size; ++i) {
        size_t sc = annex_b_start_code_len(data + i, size - i);
        if (!sc) continue;
        if (i + sc + 2 > size) continue;
        int t = (data[i + sc] >> 1) & 0x3F;
        if (t >= 16 && t <= 21) f.keyframe = true;
        else if (t == 32 || t == 33 || t == 34) f.config = true;
        i += sc;
    }
    return f;
}

struct H264CodingTools { int cabac_en; int trans8x8; };

static inline bool resolve_h264_coding_tools(uint32_t pid, H264CodingTools& out) {
    switch (pid) {
    case 66:  out = {0, 0}; return true;
    case 77:  out = {1, 0}; return true;
    case 100: out = {1, 1}; return true;
    default: return false;
    }
}

#include <rockchip/rk_mpi.h>
#include <rockchip/mpp_frame.h>
#include <rockchip/mpp_packet.h>
#include <rockchip/mpp_buffer.h>
#include <rockchip/mpp_meta.h>
#include <rockchip/mpp_task.h>
#include <rockchip/mpp_err.h>
#include <rockchip/rk_type.h>

#ifndef MPP_ALIGN
#define MPP_ALIGN(x, a) (((x) + (a) - 1) & ~((a) - 1))
#endif

#include <cstdio>
#include <cstring>
#include <deque>
#include <mutex>
#include <atomic>
#include <memory>
#include <vector>
#include <algorithm>

// ---------------------------------------------------------------------------
// RkMppEncoder — persistent buffer pool, round-robin model
// ---------------------------------------------------------------------------

class RkMppEncoder : public EncoderBackend {
public:
    RkMppEncoder() = default;

    ~RkMppEncoder() override {
        stop();
    }

    bool init(const EncoderConfig& cfg, std::string* err) override {
        std::lock_guard<std::mutex> lock(mutex_);

        if (cfg.codec != VideoCodec::H264 && cfg.codec != VideoCodec::H265) {
            if (err) *err = "encoder-rkmpp: only H.264 and H.265 are supported";
            return false;
        }
        if (cfg.input_format != RawPixelFormat::Nv12) {
            if (err) *err = "encoder-rkmpp: only NV12 input is supported";
            return false;
        }

        width_ = cfg.width;
        prefer_dmabuf_ = cfg.prefer_dmabuf;
        height_ = cfg.height;
        fps_ = cfg.fps;
        bitrate_ = cfg.bitrate;
        codec_ = cfg.codec;

        // --- MPP context ---
        MPP_RET ret = mpp_create(&ctx_, &mpi_);
        if (ret != MPP_OK) {
            if (err) *err = "encoder-rkmpp: mpp_create failed (ret="
                + std::to_string(ret) + ")";
            return false;
        }

        MppCodingType coding = (cfg.codec == VideoCodec::H265)
            ? MPP_VIDEO_CodingHEVC : MPP_VIDEO_CodingAVC;
        ret = mpp_init(ctx_, MPP_CTX_ENC, coding);
        if (ret != MPP_OK) {
            mpp_destroy(ctx_); ctx_ = nullptr; mpi_ = nullptr;
            if (err) *err = "encoder-rkmpp: mpp_init failed (ret="
                + std::to_string(ret) + ")";
            return false;
        }

        // --- Encoder config ---
        MppEncCfg enc_cfg = nullptr;
        ret = mpp_enc_cfg_init(&enc_cfg);
        if (ret != MPP_OK) {
            if (err) *err = "encoder-rkmpp: mpp_enc_cfg_init failed";
            return false;
        }
        struct EncCfgGuard {
            MppEncCfg* ptr;
            ~EncCfgGuard() { if (*ptr) mpp_enc_cfg_deinit(*ptr); }
        } enc_guard{&enc_cfg};

        auto set_required = [&](const char* key, RK_S32 val) -> bool {
            MPP_RET r = mpp_enc_cfg_set_s32(enc_cfg, key, val);
            if (r != MPP_OK) {
                if (err) *err = std::string("encoder-rkmpp: ") + key
                    + " (ret=" + std::to_string(r) + ")";
                return false;
            }
            return true;
        };
        auto set_optional = [&](const char* key, RK_S32 val) {
            MPP_RET r = mpp_enc_cfg_set_s32(enc_cfg, key, val);
            if (r != MPP_OK) {
                std::fprintf(stderr,
                    "encoder-rkmpp: optional '%s' not supported (ret=%d)\n",
                    key, static_cast<int>(r));
            }
        };

        // Prep
        hor_stride_ = MPP_ALIGN(width_, 16);
        ver_stride_ = MPP_ALIGN(height_, 16);
        frame_size_ = static_cast<size_t>(hor_stride_) * ver_stride_ * 3 / 2;

        if (!set_required("prep:width", static_cast<RK_S32>(width_))) return false;
        if (!set_required("prep:height", static_cast<RK_S32>(height_))) return false;
        if (!set_required("prep:hor_stride", static_cast<RK_S32>(hor_stride_))) return false;
        if (!set_required("prep:ver_stride", static_cast<RK_S32>(ver_stride_))) return false;
        if (!set_required("prep:format", MPP_FMT_YUV420SP)) return false;

        // Rate control (CBR, tolerates older MPP versions)
        if (!set_required("rc:mode", MPP_ENC_RC_MODE_CBR)) return false;
        if (!set_required("rc:bps_target", static_cast<RK_S32>(bitrate_))) return false;
        if (!set_required("rc:gop", static_cast<RK_S32>(cfg.gop))) return false;
        if (!set_required("rc:fps_in_num", static_cast<RK_S32>(fps_))) return false;
        if (!set_required("rc:fps_out_num", static_cast<RK_S32>(fps_))) return false;
        // Older MPP versions reject denom/flex — make optional
        set_optional("rc:fps_in_flex", 0);
        set_optional("rc:fps_in_denorm", 1);
        set_optional("rc:fps_out_flex", 0);
        set_optional("rc:fps_out_denorm", 1);
        set_optional("rc:bps_max", static_cast<RK_S32>(bitrate_ * 2));
        set_optional("rc:bps_min", static_cast<RK_S32>(bitrate_ / 2));
        set_optional("rc:qp_init", 26);
        set_optional("rc:qp_min", 18);
        set_optional("rc:qp_max", 40);

        // Codec-specific
        if (cfg.codec == VideoCodec::H264) {
            H264CodingTools tools{};
            if (!resolve_h264_coding_tools(cfg.profile_idc, tools)) {
                if (err) *err = "encoder-rkmpp: unsupported H.264 profile_idc "
                    + std::to_string(cfg.profile_idc);
                return false;
            }
            if (!set_required("h264:profile", static_cast<RK_S32>(cfg.profile_idc))) return false;
            if (!set_required("h264:level",   static_cast<RK_S32>(cfg.level_idc)))   return false;
            if (!set_required("h264:cabac_en", tools.cabac_en)) return false;
            if (!set_required("h264:cabac_idc", 0)) return false;
            if (!set_required("h264:trans8x8", tools.trans8x8)) return false;
        } else {
            RK_S32 h265_level_val = static_cast<RK_S32>(cfg.level_idc);
            if (!set_required("h265:profile", static_cast<RK_S32>(cfg.profile_idc))) return false;
            if (!set_required("h265:level",   h265_level_val)) return false;
            set_optional("h265:tier", static_cast<RK_S32>(cfg.tier_flag));
            set_optional("h265:qp_init", 26);
            set_optional("h265:scaling_list", 0);
        }
        set_optional("split:mode", 0);
        set_optional("split:arg", 0);
        set_optional("split:out", 0);

        std::fprintf(stderr,
            "[RkMppEncoder] %dx%d fps=%d bps=%d gop=%d codec=%s "
            "profile=%d level=%d tier=%d\n",
            static_cast<int>(width_), static_cast<int>(height_),
            static_cast<int>(fps_), static_cast<int>(bitrate_),
            static_cast<int>(cfg.gop),
            (cfg.codec == VideoCodec::H265) ? "h265" : "h264",
            static_cast<int>(cfg.profile_idc),
            static_cast<int>(cfg.level_idc),
            static_cast<int>(cfg.tier_flag));

        ret = mpi_->control(ctx_, MPP_ENC_SET_CFG, enc_cfg);
        mpp_enc_cfg_deinit(enc_cfg);
        enc_cfg = nullptr; // guard released
        if (ret != MPP_OK) {
            if (err) *err = "encoder-rkmpp: MPP_ENC_SET_CFG failed (ret="
                + std::to_string(ret) + ")";
            return false;
        }

        MppEncHeaderMode header_mode = MPP_ENC_HEADER_MODE_EACH_IDR;
        ret = mpi_->control(ctx_, MPP_ENC_SET_HEADER_MODE, &header_mode);
        if (ret != MPP_OK) {
            if (err) *err = "encoder-rkmpp: MPP_ENC_SET_HEADER_MODE failed (ret="
                + std::to_string(ret) + ")";
            return false;
        }

        // --- Persistent input buffer pool (kInputPoolSize, round-robin) ---
        // Buffers allocated at init and cycled each frame.
        // Mirror mpi_enc_test: DRM + CACHABLE first (CMA-backed, known to
        // work with rkvenc2 on RV1126B vendor kernels); dma_heap system
        // buffers make the encoder fault with RKV_ENC_INT_BUS_WRITE_ERROR
        // (rk_iommu "no memory region mapped") on that platform.
        ret = mpp_buffer_group_get_internal(
            &buf_group_, MPP_BUFFER_TYPE_DRM | MPP_BUFFER_FLAGS_CACHABLE);
        if (ret != MPP_OK) {
            ret = mpp_buffer_group_get_internal(
                &buf_group_, MPP_BUFFER_TYPE_DRM);
        }
        if (ret != MPP_OK) {
            ret = mpp_buffer_group_get_internal(
                &buf_group_, MPP_BUFFER_TYPE_DMA_HEAP);
        }
        if (ret != MPP_OK) {
            ret = mpp_buffer_group_get_internal(
                &buf_group_, MPP_BUFFER_TYPE_ION);
        }
        if (ret != MPP_OK || buf_group_ == nullptr) {
            if (err) *err = "encoder-rkmpp: failed to create buffer group "
                "(tried DRM|CACHABLE, DRM, DMA_HEAP, ION)";
            return false;
        }

        ret = mpp_buffer_get(buf_group_, &input_bufs_[0], frame_size_);
        if (ret != MPP_OK || input_bufs_[0] == nullptr) {
            mpp_buffer_group_put(buf_group_);
            buf_group_ = nullptr;
            if (err) *err = "encoder-rkmpp: persistent buffer alloc failed";
            return false;
        }
        for (int i = 1; i < kInputPoolSize; i++) {
            ret = mpp_buffer_get(buf_group_, &input_bufs_[i], frame_size_);
            if (ret != MPP_OK || input_bufs_[i] == nullptr) {
                for (int j = 0; j < i; j++) {
                    mpp_buffer_put(input_bufs_[j]);
                    input_bufs_[j] = nullptr;
                }
                mpp_buffer_group_put(buf_group_);
                buf_group_ = nullptr;
                if (err) *err = "encoder-rkmpp: pool buffer alloc failed";
                return false;
            }
        }

        std::fprintf(stderr,
            "[RkMppEncoder] Persistent input buffer pool: %zu bytes x %d\n",
            frame_size_, kInputPoolSize);

        // --- Explicit output packet + motion-info buffers ---
        // Mirror mpi_enc_test: attach an app-supplied output packet (and a
        // motion-info buffer) to every frame's meta.  Without them the
        // encoder hal falls back to internal dma_heap buffers, which makes
        // vepu511/rkvenc2 fault (RKV_ENC_INT_BUS_WRITE_ERROR) on RV1126B.
        for (int i = 0; i < kPacketPoolSize; i++) {
            ret = mpp_buffer_get(buf_group_, &pkt_bufs_[i], frame_size_);
            if (ret != MPP_OK || pkt_bufs_[i] == nullptr) {
                if (err) *err = "encoder-rkmpp: packet buffer alloc failed";
                return false;
            }
        }
        // 64 KiB covers the RV1126B / RK3588 md_info formulas up to 1080p+.
        ret = mpp_buffer_get(buf_group_, &md_info_, 64 * 1024);
        if (ret != MPP_OK || md_info_ == nullptr) {
            if (err) *err = "encoder-rkmpp: md_info buffer alloc failed";
            return false;
        }

        // Pre-RTP debug file (uncomment to debug encoding output):
        // debug_file_ = std::fopen("/tmp/live777-pre-rtp.h265", "wb");

        running_ = true;
        initialized_ = true;

        // Independent drain driver: completions (and with them the zero-copy
        // capture-buffer releases) must not depend on new input arriving.
        // Without this, a transient encoder slowdown lets in_flight_ pin all
        // driver buffers; capture then starves, no new submit ever calls
        // drain_and_dispatch(), and the pipeline wedges permanently.
        drain_running_ = true;
        drain_thread_ = std::thread(&RkMppEncoder::drain_loop, this);
        return true;
    }

    // ------------------------------------------------------------------
    // submit — round-robin buffer pool model
    // ------------------------------------------------------------------
    bool submit(const RawFrame& frame, std::string* err) override {
        std::lock_guard<std::mutex> lock(mutex_);

        // Deferred-requeue contract: an armed (zero-copy) frame transfers
        // its capture buffer to us.  The guard hands it back on every early
        // return; only a successful encode_put_frame moves ownership into
        // in_flight_, released when the frame's EOI is observed.
        FrameReleaseGuard release_guard(frame);

        if (!initialized_ || !running_) {
            if (err) *err = "encoder-rkmpp: not running";
            return false;
        }

        // Input validation
        if (frame.format != RawPixelFormat::Nv12) {
            if (err) *err = "encoder-rkmpp: only NV12 input is supported";
            return false;
        }
        if (frame.kind != BufferKind::Cpu
            && !(frame.kind == BufferKind::DmaBuf && prefer_dmabuf_)) {
            if (err) *err = "encoder-rkmpp: only CPU buffers are supported"
                " (or dmabuf without prefer_dmabuf)";
            return false;
        }
        if (frame.plane_count < 2) {
            if (err) *err = "encoder-rkmpp: NV12 requires >= 2 planes";
            return false;
        }

        submitted_frames_++;

        // --- Drain pending output from previous frame ---
        // Aggregates partitions to EOI, dispatches one complete AU per frame.
        drain_and_dispatch(frame.pts_us);

        // --- Select buffer from pool (index advanced only on success) ---
        MppBuffer buf = input_bufs_[next_input_idx_];

        // Use V4L2-negotiated frame dimensions for copy (not TOML config).
        uint32_t src_w = frame.width;
        uint32_t src_h = frame.height;

        if (src_w != width_ || src_h != height_) {
            if (err) {
                *err = "encoder-rkmpp: V4L2 negotiated "
                    + std::to_string(src_w) + "x" + std::to_string(src_h)
                    + " but encoder configured for "
                    + std::to_string(width_) + "x" + std::to_string(height_)
                    + ". Update TOML capture width/height to match.";
            }
            return false;
        }

        // --- Zero-copy path: import the capture dmabuf directly ---
        // The capture owns the memory; we import once per fd and reuse.
        // Frame layout must be NV12 contiguous (single fd, UV at offset).
        uint32_t frame_hor_stride = hor_stride_;
        uint32_t frame_ver_stride = ver_stride_;
        bool use_dmabuf = false;
        if (prefer_dmabuf_ && frame.kind == BufferKind::DmaBuf
            && frame.planes[0].dma_fd >= 0 && frame.planes[1].offset > 0) {
            const int fd = frame.planes[0].dma_fd;
            auto it = dmabuf_cache_.find(fd);
            if (it == dmabuf_cache_.end()) {
                MppBufferInfo info{};
                info.type = MPP_BUFFER_TYPE_DRM;
                info.fd = fd;
                info.size = static_cast<size_t>(frame.planes[0].stride) * src_h
                            + static_cast<size_t>(frame.planes[0].stride) * src_h / 2;
                MppBuffer imported = nullptr;
                if (mpp_buffer_import(&imported, &info) == MPP_OK
                    && imported != nullptr) {
                    it = dmabuf_cache_.emplace(fd, imported).first;
                } else if (err) {
                    *err = "encoder-rkmpp: dmabuf import failed (fd="
                        + std::to_string(fd) + ")";
                }
            }
            if (it != dmabuf_cache_.end()) {
                buf = it->second;
                use_dmabuf = true;
                // Use the capture's own stride (may exceed width due to ISP
                // padding); the pool path normalises to configured strides.
                frame_hor_stride = frame.planes[0].stride;
                // ver_stride stays height-based: MPP expects UV at
                // hor_stride x ver_stride, which matches the capture's
                // planes[1].offset (= stride x height) exactly.
                frame_ver_stride = ver_stride_;
            }
        }

        if (use_dmabuf) {
            // Data was written by the ISP via DMA; guard CPU-side cache ops.
            mpp_buffer_sync_begin(buf);
            mpp_buffer_sync_end(buf);
        } else {
        // --- Copy frame data into the selected buffer ---
        // DMA buffer cache sync: CPU writes must be flushed so MPP's DMA
        // engine sees the data.
        mpp_buffer_sync_begin(buf);
        uint8_t* dst = static_cast<uint8_t*>(mpp_buffer_get_ptr(buf));
        if (!dst) {
            mpp_buffer_sync_end(buf);
            if (err) *err = "encoder-rkmpp: input buffer has no mapped ptr";
            return false;
        }

        // Y plane
        {
            const uint8_t* src = frame.planes[0].data;
            uint32_t src_stride = frame.planes[0].stride > 0
                ? frame.planes[0].stride : src_w;
            if (src_stride == hor_stride_) {
                // Fast path: contiguous copy when strides match
                std::memcpy(dst, src,
                    static_cast<size_t>(hor_stride_) * src_h);
            } else {
                for (uint32_t row = 0; row < src_h; ++row) {
                    std::memcpy(dst, src, src_w);
                    src += src_stride;
                    dst += hor_stride_;
                }
            }
        }

        // UV plane (NV12 interleaved)
        {
            uint8_t* uv_dst = static_cast<uint8_t*>(mpp_buffer_get_ptr(buf))
                + static_cast<size_t>(hor_stride_) * ver_stride_;
            const uint8_t* src = frame.planes[1].data;
            uint32_t src_stride = frame.planes[1].stride > 0
                ? frame.planes[1].stride : src_w;
            if (src_stride == hor_stride_) {
                // Fast path: contiguous copy when strides match
                std::memcpy(uv_dst, src,
                    static_cast<size_t>(hor_stride_) * src_h / 2);
            } else {
                for (uint32_t row = 0; row < src_h / 2; ++row) {
                    std::memcpy(uv_dst, src, src_w);
                    src += src_stride;
                    uv_dst += hor_stride_;
                }
            }
        }

        mpp_buffer_sync_end(buf);
        }

        // --- Build MppFrame ---
        MppFrame mpp_frame = nullptr;
        MPP_RET ret = mpp_frame_init(&mpp_frame);
        if (ret != MPP_OK) {
            if (err) *err = "encoder-rkmpp: mpp_frame_init failed (ret="
                + std::to_string(ret) + ")";
            return false;
        }

        mpp_frame_set_buffer(mpp_frame, buf);
        mpp_frame_set_width(mpp_frame, width_);
        mpp_frame_set_height(mpp_frame, height_);
        mpp_frame_set_hor_stride(mpp_frame, frame_hor_stride);
        mpp_frame_set_ver_stride(mpp_frame, frame_ver_stride);
        mpp_frame_set_fmt(mpp_frame, MPP_FMT_YUV420SP);
        mpp_frame_set_eos(mpp_frame, 0);

        mpp_frame_set_pts(mpp_frame, static_cast<RK_S64>(frame.pts_us));

        // Attach an explicit output packet and motion-info buffer (see init).
        // mpp returns this same packet object from encode_get_packet; the
        // drain path deinits it after copying the data out.
        {
            MppMeta meta = mpp_frame_get_meta(mpp_frame);
            MppPacket out_pkt = nullptr;
            if (mpp_packet_init_with_buffer(
                    &out_pkt, pkt_bufs_[next_pkt_idx_]) == MPP_OK) {
                // NOTE: output packet length must be cleared before use.
                mpp_packet_set_length(out_pkt, 0);
                mpp_meta_set_packet(meta, KEY_OUTPUT_PACKET, out_pkt);
                next_pkt_idx_ = (next_pkt_idx_ + 1) % kPacketPoolSize;
            }
            if (md_info_)
                mpp_meta_set_buffer(meta, KEY_MOTION_INFO, md_info_);
        }

        // --- Submit to encoder ---
        ret = mpi_->encode_put_frame(ctx_, mpp_frame);
        mpp_frame_deinit(&mpp_frame);

        if (ret != MPP_OK) {
            put_frame_failures_++;
            if (err) *err = "encoder-rkmpp: encode_put_frame failed (ret="
                + std::to_string(ret) + ")";
            return false;
        }

        // Track the frame until its EOI.  A zero-copy frame moves its
        // capture-buffer ownership into the queue (released at completion);
        // a pool-copy frame only keeps the 1:1 completion pairing — its
        // guard releases the capture buffer on return (copy finished above).
        in_flight_.push_back({use_dmabuf ? frame.release : nullptr,
                              use_dmabuf ? frame.release_ctx : nullptr,
                              frame.buffer_index});
        if (use_dmabuf) release_guard.disarm();

        // Advance slot index only on successful submit
        next_input_idx_ = (next_input_idx_ + 1) % kInputPoolSize;

        const int depth = inflight_depth();
        if (depth > peak_inflight_depth_)
            peak_inflight_depth_ = depth;

        // --- Drain and dispatch (this frame may not be ready yet); each
        // observed completion releases the oldest held capture buffer ---
        drain_and_dispatch(frame.pts_us);

        // Periodic stats
        print_stats_if_due();

        return true;
    }

    void requestKeyframe() override {
        std::lock_guard<std::mutex> lock(mutex_);
        if (ctx_ && mpi_ && running_) {
            mpi_->control(ctx_, MPP_ENC_SET_IDR_FRAME, nullptr);
        }
    }

    // Runtime bitrate retune (adaptive bitrate control, issue #409).
    // MPP applies an MppEncCfg as a partial update — only the keys present
    // are touched — so setting just the rc:bps_* triple is enough.  bps_max
    // and bps_min must move together with the target: they were derived from
    // the initial bitrate at init and would otherwise clamp the new target.
    bool setBitrate(uint32_t bps) override {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!ctx_ || !mpi_ || !running_) return false;

        MppEncCfg cfg = nullptr;
        if (mpp_enc_cfg_init(&cfg) != MPP_OK || !cfg) {
            std::fprintf(stderr,
                "[RkMppEncoder] setBitrate(%u): mpp_enc_cfg_init failed\n", bps);
            return false;
        }
        mpp_enc_cfg_set_s32(cfg, "rc:bps_target", static_cast<RK_S32>(bps));
        mpp_enc_cfg_set_s32(cfg, "rc:bps_max", static_cast<RK_S32>(bps * 2));
        mpp_enc_cfg_set_s32(cfg, "rc:bps_min", static_cast<RK_S32>(bps / 2));
        MPP_RET ret = mpi_->control(ctx_, MPP_ENC_SET_CFG, cfg);
        mpp_enc_cfg_deinit(cfg);
        if (ret != MPP_OK) {
            std::fprintf(stderr,
                "[RkMppEncoder] setBitrate(%u): MPP_ENC_SET_CFG failed"
                " (ret=%d)\n", bps, static_cast<int>(ret));
            return false;
        }
        bitrate_ = bps;
        std::fprintf(stderr, "[RkMppEncoder] bitrate -> %u bps\n", bps);
        return true;
    }

    void stop() override {
        // Stop the independent drain driver first — it acquires mutex_, so
        // joining must happen before this function takes the lock.
        drain_running_ = false;
        if (drain_thread_.joinable()) drain_thread_.join();

        std::lock_guard<std::mutex> lock(mutex_);
        running_ = false;

        if (ctx_ && mpi_) {
            // Drain in-flight frames so their capture buffers come back
            // through the normal completion path.  Bounded: a stalled
            // encoder must not hang stop(); the remainder is force-released
            // after reset below.
            for (int i = 0; i < 50 && !in_flight_.empty(); ++i) {
                drain_and_dispatch(0);
                if (!in_flight_.empty()) usleep(2000);
            }
            // Flush (raw packet loop: EOS bookkeeping must not touch the
            // in-flight completion pairing).
            MppFrame flush_frame = nullptr;
            if (mpp_frame_init(&flush_frame) == MPP_OK) {
                mpp_frame_set_eos(flush_frame, 1);
                mpi_->encode_put_frame(ctx_, flush_frame);
                mpp_frame_deinit(&flush_frame);
                MppPacket pkt = nullptr;
                while (mpi_->encode_get_packet(ctx_, &pkt) == MPP_OK
                       && pkt) {
                    mpp_packet_deinit(&pkt);
                    pkt = nullptr;
                }
            }
            mpi_->reset(ctx_);
            mpp_destroy(ctx_);
            ctx_ = nullptr;
            mpi_ = nullptr;
        }

        // After reset the hardware no longer reads any input buffer, so
        // anything still held is safe to hand back (only an encoder stall
        // should ever reach this).
        while (!in_flight_.empty()) {
            HeldInput held = in_flight_.front();
            in_flight_.pop_front();
            if (held.release) {
                std::fprintf(stderr,
                    "[RkMppEncoder] warn: force-releasing undrained input "
                    "buffer %u at stop\n", held.buffer_index);
                held.release(held.ctx, held.buffer_index);
            }
        }

        // Imported buffers wrap capture-owned dmabuf fds.  Release the MPP
        // handles here, but do not close the original fds.
        for (auto& entry : dmabuf_cache_) {
            if (entry.second) {
                mpp_buffer_put(entry.second);
            }
        }
        dmabuf_cache_.clear();

        for (int i = 0; i < kInputPoolSize; i++) {
            if (input_bufs_[i]) {
                mpp_buffer_put(input_bufs_[i]);
                input_bufs_[i] = nullptr;
            }
        }
        for (int i = 0; i < kPacketPoolSize; i++) {
            if (pkt_bufs_[i]) {
                mpp_buffer_put(pkt_bufs_[i]);
                pkt_bufs_[i] = nullptr;
            }
        }
        if (md_info_) {
            mpp_buffer_put(md_info_);
            md_info_ = nullptr;
        }
        if (buf_group_) {
            mpp_buffer_group_put(buf_group_);
            buf_group_ = nullptr;
        }
        if (debug_file_) {
            std::fclose(debug_file_);
            debug_file_ = nullptr;
        }

        std::fprintf(stderr,
            "[RkMppEncoder] Final: subm=%llu enc=%llu put_fail=%llu "
            "get_fail=%llu\n",
            static_cast<unsigned long long>(submitted_frames_),
            static_cast<unsigned long long>(encoded_frames_),
            static_cast<unsigned long long>(put_frame_failures_),
            static_cast<unsigned long long>(get_packet_failures_));

        initialized_ = false;
    }

    bool isRunning() const override { return running_; }

    void setCallback(EncodedPacketCallback cb) override {
        std::lock_guard<std::mutex> lock(mutex_);
        encoded_cb_ = std::move(cb);
    }

private:
    static constexpr size_t kMaxEncodedAccessUnitSize = 16 * 1024 * 1024;

    // ------------------------------------------------------------------
    // Drain output — aggregate partitions to EOI, dispatch one AU/frame
    // ------------------------------------------------------------------
    // Periodic drain driver (drain_thread_): pops completed frames and
    // releases their zero-copy capture buffers independently of new input.
    // Serialized with submit()/stop() via mutex_; cheap when idle because
    // it skips straight past an empty in_flight_ queue.
    void drain_loop() {
        while (drain_running_.load()) {
            {
                std::lock_guard<std::mutex> lock(mutex_);
                if (running_ && ctx_ && mpi_ && !in_flight_.empty()) {
                    drain_and_dispatch(0);
                }
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(2));
        }
    }

    int drain_and_dispatch(uint64_t input_pts_us) {        int dispatched = 0;
        std::vector<uint8_t> au;
        uint32_t au_flags = 0;
        size_t partition_count = 0;
        uint64_t au_pts_us = input_pts_us; // default, overridden by first pkt
        bool frame_complete = false;

        while (!frame_complete) {
            MppPacket packet = nullptr;
            MPP_RET ret = mpi_->encode_get_packet(ctx_, &packet);
            if (ret != MPP_OK) {
                if (packet) mpp_packet_deinit(&packet);
                get_packet_failures_++;
                au.clear();
                return dispatched;
            }
            if (!packet) break;

            const auto* pkt_data =
                static_cast<const uint8_t*>(mpp_packet_get_data(packet));
            size_t pkt_size = mpp_packet_get_length(packet);
            bool is_partition = mpp_packet_is_partition(packet);
            bool is_eoi = mpp_packet_is_eoi(packet);

            if (pkt_data && pkt_size > 0) {
                if (au.size() > kMaxEncodedAccessUnitSize - pkt_size) {
                    mpp_packet_deinit(&packet);
                    au.clear();
                    return dispatched;
                }
                // Use PTS from first MPP packet (actual encoded frame PTS, µs)
                if (au.empty())
                    au_pts_us = static_cast<uint64_t>(
                        mpp_packet_get_pts(packet));

                au.insert(au.end(), pkt_data, pkt_data + pkt_size);

                NalFlags nf = (codec_ == VideoCodec::H265)
                    ? detect_h265_flags(pkt_data, pkt_size)
                    : detect_h264_flags(pkt_data, pkt_size);
                if (nf.keyframe) au_flags |= EncodedKeyframe;
                if (nf.config)   au_flags |= EncodedConfig;
            }
            partition_count++;

            frame_complete = !is_partition || is_eoi;
            mpp_packet_deinit(&packet);
        }

        // If we got a partition without EOI and then ran out of packets,
        // buffer the partial AU for the next drain call.
        if (!frame_complete) {
            pending_au_ = std::move(au);
            pending_au_flags_ = au_flags;
            return dispatched;
        }

        // One completion per submitted frame, counted exactly once when its
        // EOI is observed — whether or not the AU carried any data.
        completed_frames_++;

        // Completions arrive in submission order (single encoder context):
        // pair this one with the oldest in-flight input and hand its capture
        // buffer back for requeue — only now may the camera reuse the
        // memory.  A stalled encoder simply holds buffers (capture keeps
        // running on the remaining pool); it can never corrupt them.
        if (!in_flight_.empty()) {
            HeldInput held = in_flight_.front();
            in_flight_.pop_front();
            if (held.release) held.release(held.ctx, held.buffer_index);
        }

        if (au.empty()) return dispatched;

        // Merge with any pending partial AU from a previous drain
        if (!pending_au_.empty()) {
            au.insert(au.begin(), pending_au_.begin(), pending_au_.end());
            au_flags |= pending_au_flags_;
            pending_au_.clear();
            pending_au_flags_ = 0;
        }

        if (debug_file_) {
            std::fwrite(au.data(), 1, au.size(), debug_file_);
            std::fflush(debug_file_);
        }

        // Per-frame AU log throttled to stats interval (not hot path)
        if (++au_log_count_ % 150 == 0) {
            std::fprintf(stderr,
                "rkmpp-au: pts=%llu size=%zu parts=%zu key=%d cfg=%d\n",
                static_cast<unsigned long long>(au_pts_us),
                au.size(), partition_count,
                (au_flags & EncodedKeyframe) != 0,
                (au_flags & EncodedConfig) != 0);
        }

        if (partition_count > max_partitions_per_frame_)
            max_partitions_per_frame_ = partition_count;

        // Includes the drain-on-next-submit queueing by design — that wait
        // is part of the pipeline cost this instrumentation exists to show.
        enc_stats_.sample(au_pts_us, monotonic_now_us());

        if (encoded_cb_) {
            EncodedPacket out;
            out.codec = codec_;
            out.data = au.data();
            out.size = au.size();
            out.pts_us = au_pts_us;      // from actual MPP packet, not caller
            out.dts_us = au_pts_us;
            out.flags = au_flags;
            encoded_cb_(out);
            dispatched = 1;
        }

        encoded_frames_++;
        return dispatched;
    }

    // ------------------------------------------------------------------
    // Stats
    // ------------------------------------------------------------------
    void print_stats_if_due() {
        constexpr uint64_t kInterval = 150;
        if (submitted_frames_ % kInterval != 0) return;

        RK_S32 group_unused = (buf_group_)
            ? mpp_buffer_group_unused(buf_group_) : -1;
        size_t group_usage = (buf_group_)
            ? mpp_buffer_group_usage(buf_group_) : 0;

        std::fprintf(stderr,
            "[RkMppEncoder] subm=%llu enc=%llu "
            "grp_unused=%d grp_usage=%zu "
            "put_fail=%llu get_fail=%llu "
            "depth=%d peak=%d max_parts=%zu\n",
            static_cast<unsigned long long>(submitted_frames_),
            static_cast<unsigned long long>(encoded_frames_),
            group_unused, group_usage,
            static_cast<unsigned long long>(put_frame_failures_),
            static_cast<unsigned long long>(get_packet_failures_),
            inflight_depth(), peak_inflight_depth_,
            max_partitions_per_frame_);
    }

    // ------------------------------------------------------------------
    // Members
    // ------------------------------------------------------------------
    MppCtx ctx_ = nullptr;
    MppApi* mpi_ = nullptr;

    uint32_t width_ = 0, height_ = 0, fps_ = 0, bitrate_ = 0;
    VideoCodec codec_ = VideoCodec::H264;
    uint32_t hor_stride_ = 0, ver_stride_ = 0;
    size_t frame_size_ = 0;

    bool prefer_dmabuf_ = false;

    // Persistent buffer pool: kInputPoolSize buffers cycled round-robin.
    // See the allocation site for why DRM+CACHABLE is preferred on RV1126B.
    static constexpr int kInputPoolSize = 3;
    MppBufferGroup buf_group_ = nullptr;
    MppBuffer input_bufs_[kInputPoolSize] = {};

    // Zero-copy path: imported MppBuffer per capture dmabuf fd, imported once
    // and reused (the capture owns the memory; fds are stable per V4L2 slot).
    std::unordered_map<int, MppBuffer> dmabuf_cache_;
    int next_input_idx_ = 0;

    // Submitted-but-not-yet-completed frames in submission order: one entry
    // per successful encode_put_frame, popped once its EOI is observed in
    // drain_and_dispatch().  Zero-copy frames carry their capture buffer
    // release contract; pool-copy frames push a null-release marker so the
    // completion pairing stays exactly 1:1.
    struct HeldInput {
        CaptureBufferReleaseFn release;
        void* ctx;
        uint32_t buffer_index;
    };
    std::deque<HeldInput> in_flight_;

    // Explicit output packet buffers (attached per-frame via meta) and a
    // motion-info buffer, mirroring mpi_enc_test; required on RV1126B.
    static constexpr int kPacketPoolSize = 4;
    MppBuffer pkt_bufs_[kPacketPoolSize] = {};
    int next_pkt_idx_ = 0;
    MppBuffer md_info_ = nullptr;

    // Partial AU buffered across drain calls (partition without EOI)
    std::vector<uint8_t> pending_au_;
    uint32_t pending_au_flags_ = 0;

    // Pre-RTP debug file
    FILE* debug_file_ = nullptr;

    // Stats
    uint64_t submitted_frames_ = 0;
    uint64_t encoded_frames_ = 0;
    uint64_t put_frame_failures_ = 0;
    uint64_t get_packet_failures_ = 0;
    // Frames whose EOI was observed in drain_and_dispatch(); each submitted
    // frame completes exactly once, so in-flight depth can be derived.
    uint64_t completed_frames_ = 0;
    uint64_t au_log_count_ = 0;
    int peak_inflight_depth_ = 0;
    size_t max_partitions_per_frame_ = 0;

    // [latency] "encode" stage: capture SOF → encoded AU retrieved.
    LatencyStats enc_stats_{"encode"};

    // In-flight depth derived from counters: submitted minus put-failures
    // minus completed.  Deriving it (instead of maintaining a mutable
    // counter across the drain exit paths) makes double-decrement bugs
    // impossible by construction.
    int inflight_depth() const {
        return static_cast<int>(
            submitted_frames_ - put_frame_failures_ - completed_frames_);
    }

    std::atomic<bool> running_{false};
    bool initialized_ = false;

    EncodedPacketCallback encoded_cb_;
    mutable std::mutex mutex_;

    // Independent drain driver (see drain_loop / init / stop).
    std::thread drain_thread_;
    std::atomic<bool> drain_running_{false};
};

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

std::shared_ptr<EncoderBackend> create_rkmpp_encoder_backend(
    const EncoderConfig& /*cfg*/) {
    return std::make_shared<RkMppEncoder>();
}
