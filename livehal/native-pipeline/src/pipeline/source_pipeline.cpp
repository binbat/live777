//! SourcePipeline — connects CaptureBackend → EncoderBackend → Rust FFI.
//!
//! Data flow (all C++ internal until the FFI boundary):
//!   CaptureBackend → RawFrame → EncoderBackend → EncodedPacket
//!
//! Only EncodedPacket crosses the FFI boundary to Rust via a pure-C callback.
//! RawFrame never crosses FFI.
//!
//! Concurrency model:
//!   * Capture and encoder backends run on their own driver threads.
//!   * When an encoded packet is produced, the encoder backend copies the
//!     payload and submits a CallbackJob to this SourcePipeline.
//!   * A single worker thread drains the queue and invokes the FFI callback.
//!   * This prevents encoder callbacks from holding encoder locks while calling
//!     user code, and gives stop() a well-defined drain point.

#include "include/capture_backend.h"
#include "include/encoder_backend.h"
#include "include/source_pipeline_ffi.h"
#include <atomic>
#include <condition_variable>
#include <cstdio>
#include <cstring>
#include <exception>
#include <memory>
#include <mutex>
#include <queue>
#include <string>
#include <thread>
#include <utility>
#include <vector>

// =========================================================================
// Callback job queued by encoder backends and consumed by the worker thread
// =========================================================================

struct CallbackJob {
    std::vector<uint8_t> payload;
    uint32_t codec = 0;
    uint64_t pts_us = 0;
    uint64_t dts_us = 0;
    uint32_t flags = 0;
};

// =========================================================================
// SourcePipeline (C++ internal)
// =========================================================================

class SourcePipeline {
public:
    ~SourcePipeline() { stop(); }

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

        // Encoder backends call this lambda synchronously from their own
        // context.  We copy the payload and enqueue it; the worker thread will
        // invoke the actual FFI callback.  This keeps encoder locks from being
        // held while user code runs.
        encoder_->setCallback([this](const EncodedPacket& pkt) {
            submit_job(pkt);
        });

        return true;
    }

    bool start(std::string* err) {
        if (!capture_ || !encoder_) {
            if (err) *err = "pipeline not initialised";
            return false;
        }

        // Start the callback worker thread first so it is ready before any
        // encoded packets are produced.
        {
            std::lock_guard<std::mutex> lock(queue_mutex_);
            worker_running_ = true;
        }
        worker_thread_ = std::thread(&SourcePipeline::worker_loop, this);

        if (!capture_->start(
                [this](const RawFrame& frame) {
                    std::string submit_err;
                    if (!encoder_->submit(frame, &submit_err)) {
                        fprintf(stderr, "[SourcePipeline] encoder submit failed: %s\n",
                                submit_err.c_str());
                    }
                },
                err)) {
            if (err) {
                fprintf(stderr, "[SourcePipeline] start failed: %s\n", err->c_str());
            }
            // Capture start failed.  Stop the worker thread so that the
            // destructor does not block waiting for it.
            stop();
            return false;
        }
        return true;
    }

    void stop() {
        // Ensure stop() is idempotent even if called from both the start()
        // failure path and the destructor.
        if (stopped_.exchange(true)) {
            return;
        }

        // Stop capture first so no new frames enter the encoder.
        if (capture_) capture_->stop();
        // Stop encoder so no new encoded packets are produced.
        if (encoder_) encoder_->stop();

        // Signal the worker thread to finish after draining the queue.
        {
            std::lock_guard<std::mutex> lock(queue_mutex_);
            worker_running_ = false;
        }
        queue_cv_.notify_all();

        if (worker_thread_.joinable()) {
            worker_thread_.join();
        }

        // Drop any jobs that were not processed (e.g. if stop raced with
        // submit_job()).  This is safe because the worker has joined.
        std::lock_guard<std::mutex> lock(queue_mutex_);
        while (!queue_.empty()) queue_.pop();
    }

    bool isRunning() const {
        return capture_ && capture_->isRunning();
    }

    void requestKeyframe() {
        if (encoder_) encoder_->requestKeyframe();
    }

    void setPacketCallback(EncodedPacketCallbackFFI cb, void* user_data) {
        std::lock_guard<std::mutex> lock(cb_mutex_);
        on_packet_ = cb;
        user_data_ = user_data;
    }

private:
    std::shared_ptr<CaptureBackend> capture_;
    std::shared_ptr<EncoderBackend> encoder_;

    // FFI callback and user_data are protected by cb_mutex_ so they can be
    // set on one thread while the worker thread reads them.
    std::mutex cb_mutex_;
    EncodedPacketCallbackFFI on_packet_ = nullptr;
    void* user_data_ = nullptr;

    // Job queue consumed by worker_thread_.
    std::mutex queue_mutex_;
    std::condition_variable queue_cv_;
    std::queue<CallbackJob> queue_;
    bool worker_running_ = false;
    std::thread worker_thread_;
    std::atomic<bool> stopped_{false};

    void submit_job(const EncodedPacket& pkt) {
        CallbackJob job;
        if (pkt.data && pkt.size > 0) {
            job.payload.assign(pkt.data, pkt.data + pkt.size);
        }
        job.codec = static_cast<uint32_t>(pkt.codec);
        job.pts_us = pkt.pts_us;
        job.dts_us = pkt.dts_us;
        job.flags = pkt.flags;

        {
            std::lock_guard<std::mutex> lock(queue_mutex_);
            if (!worker_running_) {
                // Pipeline is stopping/stopped; drop the job.
                return;
            }
            queue_.push(std::move(job));
        }
        queue_cv_.notify_one();
    }

    void worker_loop() {
        while (true) {
            CallbackJob job;
            {
                std::unique_lock<std::mutex> lock(queue_mutex_);
                queue_cv_.wait(lock, [&] {
                    return !queue_.empty() || !worker_running_;
                });

                if (!worker_running_ && queue_.empty()) {
                    return;
                }

                if (queue_.empty()) {
                    continue;
                }

                job = std::move(queue_.front());
                queue_.pop();
            }

            EncodedPacketCallbackFFI cb = nullptr;
            void* user_data = nullptr;
            {
                std::lock_guard<std::mutex> lock(cb_mutex_);
                cb = on_packet_;
                user_data = user_data_;
            }

            if (cb) {
                EncodedPacketFFI ffi_pkt{};
                ffi_pkt.codec = job.codec;
                ffi_pkt.data = job.payload.data();
                ffi_pkt.size = job.payload.size();
                ffi_pkt.pts_us = job.pts_us;
                ffi_pkt.dts_us = job.dts_us;
                ffi_pkt.flags = job.flags;
                cb(&ffi_pkt, user_data);
            }
        }
    }
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
    cfg.backend = ffi->backend ? ffi->backend : "v4l2-m2m";
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

static void write_ffi_error(char* errbuf, size_t errbuf_len, const char* msg) {
    if (errbuf && errbuf_len > 0) {
        snprintf(errbuf, errbuf_len, "%s", msg ? msg : "unknown error");
    }
}

SourcePipelineHandle* source_pipeline_create(
    const SourcePipelineConfigFFI* cfg,
    const SourcePipelineHooksFFI* hooks,
    char* errbuf, size_t errbuf_len) noexcept {
    if (!cfg) {
        write_ffi_error(errbuf, errbuf_len, "null config");
        return nullptr;
    }

    try {
        auto pipeline = std::make_unique<SourcePipeline>();
        std::string err;

        auto ccfg = to_capture_config(&cfg->capture);
        auto ecfg = to_encoder_config(&cfg->encoder);

        if (!pipeline->init(ccfg, ecfg, &err)) {
            write_ffi_error(errbuf, errbuf_len, err.c_str());
            return nullptr;
        }

        // Wire the FFI callback: C++ EncodedPacket → pure-C EncodedPacketFFI
        // The callback and user_data are stored inside the pipeline and read by
        // the worker thread under a mutex.
        if (hooks && hooks->on_packet) {
            pipeline->setPacketCallback(hooks->on_packet, hooks->user_data);
        }

        // Transfer ownership to opaque handle
        return reinterpret_cast<SourcePipelineHandle*>(pipeline.release());
    } catch (const std::exception& e) {
        write_ffi_error(errbuf, errbuf_len, e.what());
        return nullptr;
    } catch (...) {
        write_ffi_error(errbuf, errbuf_len, "source_pipeline_create: unknown exception");
        return nullptr;
    }
}

bool source_pipeline_start(SourcePipelineHandle* h,
                           char* errbuf,
                           size_t errbuf_len) noexcept {
    if (!h) {
        write_ffi_error(errbuf, errbuf_len, "null pipeline handle");
        return false;
    }
    try {
        auto* pipeline = reinterpret_cast<SourcePipeline*>(h);
        std::string err;
        if (!pipeline->start(&err)) {
            write_ffi_error(errbuf, errbuf_len, err.c_str());
            return false;
        }
        return true;
    } catch (const std::exception& e) {
        write_ffi_error(errbuf, errbuf_len, e.what());
        return false;
    } catch (...) {
        write_ffi_error(errbuf, errbuf_len, "source_pipeline_start: unknown exception");
        return false;
    }
}

void source_pipeline_stop(SourcePipelineHandle* h) noexcept {
    if (!h) return;
    try {
        reinterpret_cast<SourcePipeline*>(h)->stop();
    } catch (const std::exception& e) {
        fprintf(stderr, "[SourcePipeline] stop threw: %s\n", e.what());
    } catch (...) {
        fprintf(stderr, "[SourcePipeline] stop threw: unknown exception\n");
    }
}

bool source_pipeline_is_running(SourcePipelineHandle* h) noexcept {
    if (!h) return false;
    try {
        return reinterpret_cast<SourcePipeline*>(h)->isRunning();
    } catch (...) {
        return false;
    }
}

void source_pipeline_request_keyframe(SourcePipelineHandle* h) noexcept {
    if (!h) return;
    try {
        reinterpret_cast<SourcePipeline*>(h)->requestKeyframe();
    } catch (...) {
        // Best-effort keyframe request; ignore exceptions.
    }
}

void source_pipeline_free(SourcePipelineHandle* h) noexcept {
    if (!h) return;
    try {
        delete reinterpret_cast<SourcePipeline*>(h);
    } catch (const std::exception& e) {
        fprintf(stderr, "[SourcePipeline] free threw: %s\n", e.what());
    } catch (...) {
        fprintf(stderr, "[SourcePipeline] free threw: unknown exception\n");
    }
}

} // extern "C"
