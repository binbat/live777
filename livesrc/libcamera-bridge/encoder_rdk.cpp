#include "encoder.h"
#include <cstdio>
#include <cstring>
#include <vector>
#include <arm_neon.h>

extern "C" {
    #include "hb_media_codec.h"
    #include "hb_media_error.h"
}

// 优化的 NEON YUYV -> NV12 转换函数
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

struct Encoder::Impl {
    media_codec_context_t* context = nullptr;
    CameraParams params;
    NALCallbackFFI p_callback = nullptr;
    void* user_data = nullptr;
    long frame_count = 0;

    ~Impl() {
        if (context) {
            hb_mm_mc_stop(context);
            hb_mm_mc_release(context);
            free(context);
        }
    }
};

Encoder::Encoder() : pImpl(std::make_unique<Impl>()) {}
Encoder::~Encoder() = default;

bool Encoder::init(const CameraParams& params) {
    pImpl->params = params;
    pImpl->context = (media_codec_context_t*)malloc(sizeof(media_codec_context_t));
    memset(pImpl->context, 0, sizeof(media_codec_context_t));

    auto* ctx = pImpl->context;
    ctx->encoder = true;
    ctx->codec_id = MEDIA_CODEC_ID_H264;

    auto* v_params = &ctx->video_enc_params;
    v_params->width = params.width;
    v_params->height = params.height;
    v_params->pix_fmt = MC_PIXEL_FORMAT_NV12;
    v_params->bitstream_buf_size = (params.width * params.height * 3 / 2 + 4095) & ~4095;
    v_params->frame_buf_count = 5;
    v_params->bitstream_buf_count = 5;
    v_params->gop_params.gop_preset_idx = 1;
    v_params->enable_user_pts = 1;

    v_params->rc_params.mode = MC_AV_RC_MODE_H264CBR;
    hb_mm_mc_get_rate_control_config(ctx, &v_params->rc_params);
    v_params->rc_params.h264_cbr_params.intra_period = 30;
    v_params->rc_params.h264_cbr_params.frame_rate = params.fps;
    v_params->rc_params.h264_cbr_params.bit_rate = params.bitrate / 1000;

    if (hb_mm_mc_initialize(ctx) != 0) return false;
    if (hb_mm_mc_configure(ctx) != 0) return false;

    mc_av_codec_startup_params_t startup_params;
    memset(&startup_params, 0, sizeof(startup_params));
    return hb_mm_mc_start(ctx, &startup_params) == 0;
}

void Encoder::encode(const uint8_t* data, size_t size, uint64_t timestamp) {
    if (!pImpl->context) return;

    media_codec_buffer_t input_buf;
    memset(&input_buf, 0, sizeof(media_codec_buffer_t));
    if (hb_mm_mc_dequeue_input_buffer(pImpl->context, &input_buf, 100) == 0) {
        yuyv_to_nv12_neon(data, input_buf.vframe_buf.vir_ptr[0], input_buf.vframe_buf.vir_ptr[1], pImpl->params.width, pImpl->params.height);
        input_buf.vframe_buf.pts = timestamp / 1000;
        hb_mm_mc_queue_input_buffer(pImpl->context, &input_buf, 100);
    }

    media_codec_buffer_t output_buf;
    memset(&output_buf, 0, sizeof(media_codec_buffer_t));
    if (hb_mm_mc_dequeue_output_buffer(pImpl->context, &output_buf, NULL, 0) == 0) {
        if (pImpl->p_callback) {
            uint8_t* out_data = (uint8_t*)output_buf.vstream_buf.vir_ptr;
            uint32_t out_len = output_buf.vstream_buf.size;
            
            // 关键帧检测逻辑：解析 H.264 NAL Unit Type
            // 查找起始码 00 00 00 01 (Annex-B)
            int is_kf = 0;
            if (out_len > 5) {
                for (uint32_t i = 0; i < out_len - 4; ++i) {
                    if (out_data[i] == 0 && out_data[i+1] == 0 && out_data[i+2] == 0 && out_data[i+3] == 1) {
                        int nal_type = out_data[i+4] & 0x1F;
                        if (nal_type == 5 || nal_type == 7 || nal_type == 8) { // IDR, SPS, or PPS
                            is_kf = 1;
                            break;
                        }
                    }
                }
            }

            pImpl->p_callback(out_data, out_len, is_kf, timestamp, pImpl->user_data);
        }
        hb_mm_mc_queue_output_buffer(pImpl->context, &output_buf, 100);

        if (++pImpl->frame_count % 150 == 0) {
            printf("[V4L2-RDK] Encoding flow stable. Frames: %ld\n", pImpl->frame_count);
        }
    }
}

void Encoder::encodeShared(int dma_fd, size_t size, uint64_t timestamp) {
    // 暂未实现
}

void Encoder::stop() {
    if (pImpl->context) {
        hb_mm_mc_stop(pImpl->context);
    }
}

void Encoder::setNALCallback(NALCallbackFFI cb, void* user_data) {
    pImpl->p_callback = cb;
    pImpl->user_data = user_data;
}

void Encoder::requestKeyframe() {
    // RDK 会根据 intra_period 自动下发
}

const char* Encoder::getError() const {
    return "Encoder error";
}
