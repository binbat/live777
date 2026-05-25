//! SourcePipeline — connects CaptureBackend → EncoderBackend.
//!
//! Data flow (all C++ internal):
//!   CaptureBackend → RawFrame → EncoderBackend → EncodedPacket
//!
//! Only EncodedPacket crosses the FFI boundary to Rust via a pure-C callback.
//! RawFrame never crosses FFI.

#include "include/capture_backend.h"
#include "include/encoder_backend.h"
#include "include/source_pipeline_ffi.h"
#include <cstdio>
#include <cstring>
#include <memory>
#include <string>
#include <utility>

// =========================================================================
// SourcePipeline (C++ internal)
// =========================================================================

class SourcePipeline {
public:
    bool init(const CaptureConfig& ccfg, const EncoderConfig& ecfg,
              std::string* err) {
        capture_ = create_capture_backend(ccfg);
        if (!capture_) {
            if (err) *err = "failed to create capture backend";
            return false;
        }
        if (!capture_->init(ccfg, err)) return false;

        encoder_ = create_encoder_backend(ecfg);
        if (!encoder_) {
            if (err) *err = "failed to create encoder backend";
            return false;
        }
        if (!encoder_->init(ecfg, err)) return false;

        encoder_->setCallback([this](const EncodedPacket& pkt) {
            if (on_packet_) on_packet_(pkt);
        });

        return true;
    }

    bool start(std::string* err) {
        if (!capture_ || !encoder_) {
            if (err) *err = "pipeline not initialised";
            return false;
        }
        return capture_->start(
            [this](const RawFrame& frame) {
                std::string ignored_err;
                encoder_->submit(frame, &ignored_err);
            },
            err);
    }

    void stop() {
        if (capture_) capture_->stop();
        if (encoder_) encoder_->stop();
    }

    bool isRunning() const {
        return capture_ && capture_->isRunning();
    }

    void requestKeyframe() {
        if (encoder_) encoder_->requestKeyframe();
    }

    void setPacketCallback(EncodedPacketCallback cb) {
        on_packet_ = std::move(cb);
    }

private:
    std::unique_ptr<CaptureBackend> capture_;
    std::unique_ptr<EncoderBackend> encoder_;
    EncodedPacketCallback on_packet_;
};

// =========================================================================
// FFI struct → C++ struct conversion
// =========================================================================

static CaptureConfig to_capture_config(const CaptureConfigFFI* ffi) {
    CaptureConfig cfg;
    cfg.backend = ffi->backend ? ffi->backend : "v4l2";
    cfg.device = ffi->device ? ffi->device : "/dev/video0";
    cfg.width = ffi->width;
    cfg.height = ffi->height;
    cfg.fps = ffi->fps;
    cfg.pixel_format = static_cast<RawPixelFormat>(ffi->pixel_format);
    cfg.prefer_dmabuf = (ffi->prefer_dmabuf != 0);
    return cfg;
}

static EncoderConfig to_encoder_config(const EncoderConfigFFI* ffi) {
    EncoderConfig cfg;
    cfg.backend = ffi->backend ? ffi->backend : "v4l2_m2m";
    cfg.codec = static_cast<VideoCodec>(ffi->codec);
    cfg.width = ffi->width;
    cfg.height = ffi->height;
    cfg.fps = ffi->fps;
    cfg.bitrate = ffi->bitrate;
    cfg.profile = ffi->profile ? ffi->profile : "42001f";
    cfg.gop = ffi->gop;
    cfg.prefer_dmabuf = (ffi->prefer_dmabuf != 0);
    return cfg;
}

// =========================================================================
// FFI implementation
// =========================================================================

extern "C" {

SourcePipelineHandle* source_pipeline_create(
    const SourcePipelineConfigFFI* cfg,
    const SourcePipelineHooksFFI* hooks,
    char* errbuf, size_t errbuf_len) {
    if (!cfg) return nullptr;

    auto pipeline = std::make_unique<SourcePipeline>();
    std::string err;

    auto ccfg = to_capture_config(&cfg->capture);
    auto ecfg = to_encoder_config(&cfg->encoder);

    if (!pipeline->init(ccfg, ecfg, &err)) {
        if (errbuf && errbuf_len > 0) {
            snprintf(errbuf, errbuf_len, "%s", err.c_str());
        }
        return nullptr;
    }

    // Wire the FFI callback: C++ EncodedPacket → pure-C EncodedPacketFFI
    // Copy cb and user_data by value so the lambda owns them independently
    // of the stack-local *hooks pointer.
    if (hooks && hooks->on_packet) {
        auto cb = hooks->on_packet;
        auto ud = hooks->user_data;
        pipeline->setPacketCallback(
            [cb, ud](const EncodedPacket& pkt) {
                EncodedPacketFFI ffi_pkt{};
                ffi_pkt.codec = static_cast<uint32_t>(pkt.codec);
                ffi_pkt.data = pkt.data;
                ffi_pkt.size = pkt.size;
                ffi_pkt.pts_us = pkt.pts_us;
                ffi_pkt.dts_us = pkt.dts_us;
                ffi_pkt.flags = pkt.flags;
                cb(&ffi_pkt, ud);
            });
    }

    // Transfer ownership to opaque handle
    return reinterpret_cast<SourcePipelineHandle*>(pipeline.release());
}

bool source_pipeline_start(SourcePipelineHandle* h) {
    if (!h) return false;
    auto* pipeline = reinterpret_cast<SourcePipeline*>(h);
    std::string err;
    return pipeline->start(&err);
}

void source_pipeline_stop(SourcePipelineHandle* h) {
    if (!h) return;
    reinterpret_cast<SourcePipeline*>(h)->stop();
}

bool source_pipeline_is_running(SourcePipelineHandle* h) {
    if (!h) return false;
    return reinterpret_cast<SourcePipeline*>(h)->isRunning();
}

void source_pipeline_request_keyframe(SourcePipelineHandle* h) {
    if (!h) return;
    reinterpret_cast<SourcePipeline*>(h)->requestKeyframe();
}

void source_pipeline_free(SourcePipelineHandle* h) {
    if (!h) return;
    auto* pipeline = reinterpret_cast<SourcePipeline*>(h);
    pipeline->stop();
    delete pipeline;
}

} // extern "C"
