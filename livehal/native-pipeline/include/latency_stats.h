//! Per-stage pipeline latency instrumentation (P0 measurement).
//!
//! Every stage samples `now - frame_pts_us` — the frame's age since its
//! capture timestamp (V4L2 buffer SOF, or steady_clock at capture callback
//! for libcamera) — and prints a one-line aggregate once per second,
//! following the waybeam practice of sender-side capture-to-wire
//! measurement.  All clocks are CLOCK_MONOTONIC (steady_clock on Linux),
//! which is also the V4L2 buffer timestamp domain, so deltas are true
//! latencies on every capture backend.
//!
//! Stages (cumulative from capture):
//!   capture  — V4L2 DQBUF returned to userspace
//!   encode   — encoded access unit retrieved from the encoder
//!   dispatch — FFI callback about to fire (C++ worker queue drained)
//!   wire     — RTP packets broadcast to subscribers (liveion, Rust side)
//!
//! NOT thread-safe: give each thread its own instance.

#pragma once
#include <cstdint>
#include <cstdio>
#include <time.h>

inline uint64_t monotonic_now_us() {
    timespec ts{};
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return static_cast<uint64_t>(ts.tv_sec) * 1000000ULL
           + static_cast<uint64_t>(ts.tv_nsec) / 1000ULL;
}

class LatencyStats {
public:
    explicit LatencyStats(const char* stage) : stage_(stage) {}

    void sample(uint64_t frame_pts_us, uint64_t now_us) {
        if (frame_pts_us == 0) return; // no usable origin timestamp
        if (window_start_us_ == 0) window_start_us_ = now_us;
        const int64_t delta = static_cast<int64_t>(now_us) - static_cast<int64_t>(frame_pts_us);
        sum_us_ += delta;
        if (delta > max_us_) max_us_ = delta;
        ++count_;
        if (now_us - window_start_us_ >= 1000000ULL) {
            std::fprintf(stderr, "[latency] %s: n=%llu avg=%.2fms max=%.2fms\n",
                         stage_, static_cast<unsigned long long>(count_),
                         static_cast<double>(sum_us_) / 1000.0 / static_cast<double>(count_),
                         static_cast<double>(max_us_) / 1000.0);
            window_start_us_ = now_us;
            count_ = 0;
            sum_us_ = 0;
            max_us_ = 0;
        }
    }

private:
    const char* stage_;
    uint64_t window_start_us_ = 0;
    uint64_t count_ = 0;
    int64_t sum_us_ = 0;
    int64_t max_us_ = 0;
};
