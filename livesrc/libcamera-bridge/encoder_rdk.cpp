#include "include/encoder_backend.h"
#include <cstdio>
#include <cstring>
#include <utility>
#include <vector>

extern "C" {
#include "hb_media_codec.h"
#include "hb_media_error.h"
}

// ---------------------------------------------------------------------------
// YUYV -> NV12 colour-space conversion.
//
// The previous NEON version had alignment/stride bugs with non-multiple-of-16
// widths.  This scalar implementation is safe for any resolution and can be
// replaced with a verified NEON path later.
// ---------------------------------------------------------------------------
static void yuyv_to_nv12_scalar(const uint8_t* yuyv, uint8_t* nv12_y,
                                uint8_t* nv12_uv, int width, int height) {
    const int num_pixels = width * height;

    // Y plane: every other byte is Y.
    for (int i = 0; i < num_pixels; ++i) {
        nv12_y[i] = yuyv[i * 2];
    }

    // UV plane: subsample 2x2, interleave U and V.
    // Guard against odd dimensions: need a full 2x2 block.
    // NV12 UV stride is the same as the Y stride, which is rounded up to
    // an even number for odd widths. Using `width` directly would overwrite
    // by one byte per UV row when width is odd.
    const int uv_stride = (width + 1) & ~1;
    for (int row = 0; row + 1 < height; row += 2) {
        const uint8_t* src0 = yuyv + row * width * 2;
        const uint8_t* src1 = src0 + width * 2;
        uint8_t* dest_uv = nv12_uv + (row / 2) * uv_stride;

        for (int col = 0; col + 1 < width; col += 2) {
            // Average U and V from the 2x2 block.
            int u = (src0[col * 2 + 1] + src1[col * 2 + 1]) / 2;
            int v = (src0[col * 2 + 3] + src1[col * 2 + 3]) / 2;
            dest_uv[col] = static_cast<uint8_t>(u);
            dest_uv[col + 1] = static_cast<uint8_t>(v);
        }
    }
}

// ---------------------------------------------------------------------------
// H.264 / H.265 Annex-B NAL helpers.
// ---------------------------------------------------------------------------
static bool is_annex_b_start_code(const uint8_t* data, size_t pos, size_t len,
                                  size_t* start_len) {
    if (pos + 4 <= len && data[pos] == 0 && data[pos + 1] == 0 &&
        data[pos + 2] == 0 && data[pos + 3] == 1) {
        *start_len = 4;
        return true;
    }
    if (pos + 3 <= len && data[pos] == 0 && data[pos + 1] == 0 &&
        data[pos + 2] == 1) {
        *start_len = 3;
        return true;
    }
    return false;
}

static uint8_t h264_nal_type(const uint8_t* data, size_t nal_start) {
    return data[nal_start] & 0x1F;
}

static uint8_t h265_nal_type(const uint8_t* data, size_t nal_start) {
    return (data[nal_start] >> 1) & 0x3F;
}

static uint32_t detect_h264_flags(const uint8_t* data, size_t len) {
    uint32_t flags = 0;
    size_t pos = 0;
    while (pos < len) {
        size_t start_len = 0;
        if (!is_annex_b_start_code(data, pos, len, &start_len)) {
            ++pos;
            continue;
        }
        size_t nal_start = pos + start_len;
        if (nal_start >= len) break;

        uint8_t nal_type = h264_nal_type(data, nal_start);
        if (nal_type == 5)
            flags |= static_cast<uint32_t>(EncodedKeyframe);
        else if (nal_type == 7 || nal_type == 8)
            flags |= static_cast<uint32_t>(EncodedConfig);

        // Advance past this start code; the loop will find the next one.
        pos = nal_start;
    }
    return flags;
}

static uint32_t detect_h265_flags(const uint8_t* data, size_t len) {
    uint32_t flags = 0;
    size_t pos = 0;
    while (pos < len) {
        size_t start_len = 0;
        if (!is_annex_b_start_code(data, pos, len, &start_len)) {
            ++pos;
            continue;
        }
        size_t nal_start = pos + start_len;
        if (nal_start + 1 >= len) break;

        uint8_t nal_type = h265_nal_type(data, nal_start);
        // IDR_W_RADL (19), IDR_N_LP (20), CRA_NUT (21)
        if (nal_type == 19 || nal_type == 20 || nal_type == 21)
            flags |= static_cast<uint32_t>(EncodedKeyframe);
        // VPS (32), SPS (33), PPS (34)
        else if (nal_type == 32 || nal_type == 33 || nal_type == 34)
            flags |= static_cast<uint32_t>(EncodedConfig);

        pos = nal_start;
    }
    return flags;
}

// ---------------------------------------------------------------------------
// EncoderBackend implementation (RDK X5)
//
// Two paths:
//   1. CPU copy-path (STABLE, default):
//      RawFrame.data (YUYV) -> scalar CSC -> NV12 -> hb_mm_mc_queue_input_buffer
//   2. DMA-BUF zero-copy path (WIP, gated by prefer_dmabuf=true):
//      NOT YET IMPLEMENTED.  Do not enable in production.
// ---------------------------------------------------------------------------
class RdkX5Encoder : public EncoderBackend {
public:
    media_codec_context_t* context = nullptr;
    int width = 0;
    int height = 0;
    int fps = 0;
    int bitrate = 0;
    VideoCodec codec_ = VideoCodec::H264;
    long frame_count = 0;
    bool running_ = false;

    EncodedPacketCallback encoded_cb_;
    bool initialized_ = false;

    RdkX5Encoder() = default;
    ~RdkX5Encoder() override {
        if (context) {
            if (initialized_) {
                hb_mm_mc_stop(context);
                initialized_ = false;
            }
            hb_mm_mc_release(context);
            free(context);
            context = nullptr;
        }
    }

    // --- EncoderBackend overrides ---
    bool init(const EncoderConfig& cfg, std::string* err) override;
    bool submit(const RawFrame& frame, std::string* err) override;
    void requestKeyframe() override;
    void stop() override;
    bool isRunning() const override;
    void setCallback(EncodedPacketCallback cb) override;
};

bool RdkX5Encoder::init(const EncoderConfig& cfg, std::string* err) {
    (void)err;
    width = static_cast<int>(cfg.width);
    height = static_cast<int>(cfg.height);
    fps = static_cast<int>(cfg.fps);
    bitrate = static_cast<int>(cfg.bitrate);
    codec_ = cfg.codec;

    context = (media_codec_context_t*)malloc(sizeof(media_codec_context_t));
    memset(context, 0, sizeof(media_codec_context_t));

    auto* ctx = context;
    ctx->encoder = true;
    ctx->codec_id = (codec_ == VideoCodec::H265) ? MEDIA_CODEC_ID_H265
                                                 : MEDIA_CODEC_ID_H264;

    auto* v_params = &ctx->video_enc_params;
    v_params->width = width;
    v_params->height = height;
    v_params->pix_fmt = MC_PIXEL_FORMAT_NV12;
    v_params->bitstream_buf_size = (width * height * 3 / 2 + 4095) & ~4095;
    v_params->frame_buf_count = 5;
    v_params->bitstream_buf_count = 5;
    v_params->gop_params.gop_preset_idx = 1;
    v_params->enable_user_pts = 1;

    if (codec_ == VideoCodec::H265) {
        v_params->rc_params.mode = MC_AV_RC_MODE_H265CBR;
        hb_mm_mc_get_rate_control_config(ctx, &v_params->rc_params);
        v_params->rc_params.h265_cbr_params.intra_period =
            static_cast<int>(cfg.gop);
        v_params->rc_params.h265_cbr_params.frame_rate = fps;
        v_params->rc_params.h265_cbr_params.bit_rate = bitrate / 1000;
    } else {
        v_params->rc_params.mode = MC_AV_RC_MODE_H264CBR;
        hb_mm_mc_get_rate_control_config(ctx, &v_params->rc_params);
        v_params->rc_params.h264_cbr_params.intra_period =
            static_cast<int>(cfg.gop);
        v_params->rc_params.h264_cbr_params.frame_rate = fps;
        v_params->rc_params.h264_cbr_params.bit_rate = bitrate / 1000;
    }

    if (hb_mm_mc_initialize(ctx) != 0) return false;
    if (hb_mm_mc_configure(ctx) != 0) return false;

    mc_av_codec_startup_params_t startup_params;
    memset(&startup_params, 0, sizeof(startup_params));
    if (hb_mm_mc_start(ctx, &startup_params) != 0) return false;

    initialized_ = true;
    running_ = true;
    return true;
}

bool RdkX5Encoder::submit(const RawFrame& frame, std::string* err) {
    if (!context || !running_) return false;

    // DMA-BUF zero-copy path: NOT YET IMPLEMENTED
    if (frame.kind == BufferKind::DmaBuf) {
        if (err)
            *err = "DMA-BUF encode path not yet implemented; use CPU path";
        return false;
    }

    // CPU copy-path: YUYV -> NV12 via scalar CSC, then submit to hardware
    if (frame.planes[0].data == nullptr) {
        if (err) *err = "frame data is null";
        return false;
    }

    media_codec_buffer_t input_buf;
    memset(&input_buf, 0, sizeof(media_codec_buffer_t));
    if (hb_mm_mc_dequeue_input_buffer(context, &input_buf, 100) == 0) {
        if (!input_buf.vframe_buf.vir_ptr[0] || !input_buf.vframe_buf.vir_ptr[1]) {
            if (err) *err = "RDK encoder returned input buffer with null virtual address";
            hb_mm_mc_queue_input_buffer(context, &input_buf, 100);
            return false;
        }
        yuyv_to_nv12_scalar(frame.planes[0].data,
                            input_buf.vframe_buf.vir_ptr[0],
                            input_buf.vframe_buf.vir_ptr[1], width, height);
        input_buf.vframe_buf.pts = frame.pts_us / 1000;
        if (hb_mm_mc_queue_input_buffer(context, &input_buf, 100) != 0) {
            if (err) *err = "RDK encoder failed to queue input buffer";
            return false;
        }
    }

    media_codec_buffer_t output_buf;
    memset(&output_buf, 0, sizeof(media_codec_buffer_t));
    if (hb_mm_mc_dequeue_output_buffer(context, &output_buf, NULL, 0) == 0) {
        uint8_t* out_data = (uint8_t*)output_buf.vstream_buf.vir_ptr;
        uint32_t out_len = output_buf.vstream_buf.size;

        if (!out_data || out_len == 0 || out_len > 16 * 1024 * 1024) {
            if (err)
                *err = "RDK encoder returned invalid output buffer (null pointer or bad size)";
            hb_mm_mc_queue_output_buffer(context, &output_buf, 100);
            return false;
        }

        // Detect keyframe / config NALs based on codec.
        uint32_t flags = 0;
        if (codec_ == VideoCodec::H265) {
            flags = detect_h265_flags(out_data, out_len);
        } else {
            flags = detect_h264_flags(out_data, out_len);
        }

        // Dispatch via new callback
        if (encoded_cb_) {
            EncodedPacket pkt{};
            pkt.codec = codec_;
            pkt.data = out_data;
            pkt.size = out_len;
            pkt.pts_us = frame.pts_us;
            pkt.dts_us = frame.pts_us;
            pkt.flags = flags;
            encoded_cb_(pkt);
        }

        if (hb_mm_mc_queue_output_buffer(context, &output_buf, 100) != 0) {
            if (err) *err = "RDK encoder failed to queue output buffer";
            return false;
        }
    }
    return true;
}

void RdkX5Encoder::requestKeyframe() {
    // RDK X5 hardware issues IDR automatically based on intra_period.
    // A force-IDR control may be added in a future SDK update.
}

void RdkX5Encoder::stop() {
    running_ = false;
    if (context && initialized_) {
        hb_mm_mc_stop(context);
        initialized_ = false;
    }
}

bool RdkX5Encoder::isRunning() const {
    return running_;
}

void RdkX5Encoder::setCallback(EncodedPacketCallback cb) {
    encoded_cb_ = std::move(cb);
}

// ---------------------------------------------------------------------------
// Factory for EncoderBackend (RDK X5)
// ---------------------------------------------------------------------------
std::shared_ptr<EncoderBackend> create_rdk_x5_encoder_backend(const EncoderConfig& cfg) {
    (void)cfg;
    return std::make_shared<RdkX5Encoder>();
}
