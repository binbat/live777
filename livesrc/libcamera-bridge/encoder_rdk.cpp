#include "encoder.h"
#include "include/encoder_backend.h"
#include <cstdio>
#include <cstring>
#include <vector>
#include <utility>
#include <arm_neon.h>

extern "C" {
    #include "hb_media_codec.h"
    #include "hb_media_error.h"
}

// Optimized NEON YUYV -> NV12 colour-space conversion.
static void yuyv_to_nv12_neon(const uint8_t* yuyv, uint8_t* nv12_y, uint8_t* nv12_uv, int width, int height) {
    int num_pixels = width * height;
    for (int i = 0; i < num_pixels; i += 16) {
        uint8x16x2_t raw = vld2q_u8(yuyv + i * 2);
        vst1q_u8(nv12_y + i, raw.val[0]);
    }
    for (int i = 0; i < height; i += 2) {
        const uint8_t* line = yuyv + i * width * 2;
        uint8_t* dest_uv = nv12_uv + (i / 2) * width;
        for (int j = 0; j < width; j += 16) {
            uint8x16x4_t raw = vld4q_u8(line + j * 2);
            uint8x16x2_t uv;
            uv.val[0] = raw.val[1];
            uv.val[1] = raw.val[3];
            vst2q_u8(dest_uv + j, uv);
        }
    }
}

struct Encoder::Impl : public EncoderBackend {
    media_codec_context_t* context = nullptr;
    int width = 0;
    int height = 0;
    int fps = 0;
    int bitrate = 0;
    long frame_count = 0;
    bool running_ = false;

    EncodedPacketCallback encoded_cb_;

    ~Impl() {
        if (context) {
            hb_mm_mc_stop(context);
            hb_mm_mc_release(context);
            free(context);
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

// ---------------------------------------------------------------------------
// EncoderBackend implementation (RDK X5)
//
// Two paths:
//   1. CPU copy-path (STABLE, default):
//      RawFrame.data (YUYV) → NEON CSC → NV12 → hb_mm_mc_queue_input_buffer
//   2. DMA-BUF zero-copy path (WIP, gated by prefer_dmabuf=true):
//      NOT YET IMPLEMENTED.  Do not enable in production.
// ---------------------------------------------------------------------------
bool Encoder::Impl::init(const EncoderConfig& cfg, std::string* err) {
    (void)err;
    width = static_cast<int>(cfg.width);
    height = static_cast<int>(cfg.height);
    fps = static_cast<int>(cfg.fps);
    bitrate = static_cast<int>(cfg.bitrate);

    context = (media_codec_context_t*)malloc(sizeof(media_codec_context_t));
    memset(context, 0, sizeof(media_codec_context_t));

    auto* ctx = context;
    ctx->encoder = true;
    ctx->codec_id = (cfg.codec == VideoCodec::H265) ? MEDIA_CODEC_ID_H265
                                                    : MEDIA_CODEC_ID_H264;

    auto* v_params = &ctx->video_enc_params;
    v_params->width = width;
    v_params->height = height;
    v_params->pix_fmt = MC_PIXEL_FORMAT_NV12;
    v_params->bitstream_buf_size =
        (width * height * 3 / 2 + 4095) & ~4095;
    v_params->frame_buf_count = 5;
    v_params->bitstream_buf_count = 5;
    v_params->gop_params.gop_preset_idx = 1;
    v_params->enable_user_pts = 1;

    v_params->rc_params.mode = MC_AV_RC_MODE_H264CBR;
    hb_mm_mc_get_rate_control_config(ctx, &v_params->rc_params);
    v_params->rc_params.h264_cbr_params.intra_period =
        static_cast<int>(cfg.gop);
    v_params->rc_params.h264_cbr_params.frame_rate = fps;
    v_params->rc_params.h264_cbr_params.bit_rate = bitrate / 1000;

    if (hb_mm_mc_initialize(ctx) != 0) return false;
    if (hb_mm_mc_configure(ctx) != 0) return false;

    mc_av_codec_startup_params_t startup_params;
    memset(&startup_params, 0, sizeof(startup_params));
    if (hb_mm_mc_start(ctx, &startup_params) != 0) return false;

    running_ = true;
    return true;
}

bool Encoder::Impl::submit(const RawFrame& frame, std::string* err) {
    if (!context || !running_) return false;

    // DMA-BUF zero-copy path: NOT YET IMPLEMENTED
    if (frame.kind == BufferKind::DmaBuf) {
        if (err)
            *err = "DMA-BUF encode path not yet implemented; use CPU path";
        return false;
    }

    // CPU copy-path: YUYV → NV12 via NEON, then submit to hardware
    if (frame.planes[0].data == nullptr) {
        if (err) *err = "frame data is null";
        return false;
    }

    media_codec_buffer_t input_buf;
    memset(&input_buf, 0, sizeof(media_codec_buffer_t));
    if (hb_mm_mc_dequeue_input_buffer(context, &input_buf, 100) == 0) {
        yuyv_to_nv12_neon(frame.planes[0].data,
                          input_buf.vframe_buf.vir_ptr[0],
                          input_buf.vframe_buf.vir_ptr[1], width,
                          height);
        input_buf.vframe_buf.pts = frame.pts_us / 1000;
        hb_mm_mc_queue_input_buffer(context, &input_buf, 100);
    }

    media_codec_buffer_t output_buf;
    memset(&output_buf, 0, sizeof(media_codec_buffer_t));
    if (hb_mm_mc_dequeue_output_buffer(context, &output_buf, NULL, 0) == 0) {
        uint8_t* out_data = (uint8_t*)output_buf.vstream_buf.vir_ptr;
        uint32_t out_len = output_buf.vstream_buf.size;

        // Detect keyframe / config NALs
        uint32_t flags = 0;
        if (out_len > 5) {
            for (uint32_t i = 0; i < out_len - 4; ++i) {
                if (out_data[i] == 0 && out_data[i + 1] == 0
                    && out_data[i + 2] == 0 && out_data[i + 3] == 1) {
                    int nal_type = out_data[i + 4] & 0x1F;
                    if (nal_type == 5)
                        flags |= static_cast<uint32_t>(EncodedKeyframe);
                    else if (nal_type == 7 || nal_type == 8)
                        flags |= static_cast<uint32_t>(EncodedConfig);
                }
            }
        }

        // Dispatch via new callback
        if (encoded_cb_) {
            EncodedPacket pkt{};
            pkt.codec = VideoCodec::H264;
            pkt.data = out_data;
            pkt.size = out_len;
            pkt.pts_us = frame.pts_us;
            pkt.dts_us = frame.pts_us;
            pkt.flags = flags;
            encoded_cb_(pkt);
        }

        hb_mm_mc_queue_output_buffer(context, &output_buf, 100);

        if (++frame_count % 60 == 0) {
            fprintf(stderr,
                    "[RDK Encoder] submit frame=%ld kind=%s bytes=%u "
                    "encoded=%u keyframe=%d\n",
                    frame_count,
                    frame.kind == BufferKind::DmaBuf ? "DmaBuf" : "CPU",
                    frame.planes[0].bytes,
                    out_len,
                    (flags & static_cast<uint32_t>(EncodedKeyframe)) ? 1 : 0);
        }
    }
    return true;
}

void Encoder::Impl::requestKeyframe() {
    // RDK X5 hardware issues IDR automatically based on intra_period.
    // A force-IDR control may be added in a future SDK update.
}

void Encoder::Impl::stop() {
    running_ = false;
    if (context) {
        hb_mm_mc_stop(context);
    }
}

bool Encoder::Impl::isRunning() const {
    return running_;
}

void Encoder::Impl::setCallback(EncodedPacketCallback cb) {
    encoded_cb_ = std::move(cb);
}

Encoder::Encoder() : pImpl(std::make_unique<Impl>()) {}
Encoder::~Encoder() = default;

// ---------------------------------------------------------------------------
// Factory for EncoderBackend (RDK X5)
// ---------------------------------------------------------------------------

std::unique_ptr<EncoderBackend> Encoder::createRdkX5Backend() {
    return std::make_unique<Impl>();
}

std::unique_ptr<EncoderBackend> create_rdk_x5_encoder_backend(const EncoderConfig& cfg) {
    (void)cfg;
    return Encoder::createRdkX5Backend();
}
