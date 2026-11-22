//! MockCaptureImpl — synthetic NV12 frame source for camera-free testing of
//! the hardware encoder backends (issue live777#519).
//!
//! Generates NV12 (two-plane, CPU buffers) frames on a pacing thread at the
//! configured rate.  No device is opened; the encoder path runs end-to-end
//! exactly as with a real capture backend, so encoder bring-up on a new SoC
//! can be validated with no camera attached.
//!
//! The background pattern is selectable (cfg.pattern), mirroring the names
//! of gst videotestsrc / ffmpeg testsrc:
//!   * "smpte75" (default) — static 75% SMPTE color bars (BT.601 limited
//!     range): validates color/matrix handling,
//!   * "snow"   — full-range luma white noise, new every frame: worst-case
//!     encoder stress,
//!   * "black" / "white" — solid frames: idle bitrate and keyframe floor,
//!   * "checkers" — a checkerboard drifting one pixel per frame: sharpness
//!     and sub-pixel motion,
//!   * "gradient" — a horizontal luma ramp: banding/quantization.
//! Every pattern carries the same verification overlays on top:
//!   * a white block bouncing around the frame — makes the delivered
//!     framerate and gaps visible,
//!   * a 32-bit binary frame counter strip at the bottom — increments by
//!     one every frame, so dropped/repeated frames are directly observable,
//!   * the decimal frame number burned into the top-left corner.
//! Frame numbering is 1-based, matching the V4L2 backend's RawFrame.seq.

#include "include/capture_backend.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <memory>
#include <string>
#include <thread>
#include <vector>

#if defined(__linux__)
#include <pthread.h>
#endif

namespace {

#if defined(__linux__)
void set_thread_name(const char* name) {
    pthread_setname_np(pthread_self(), name);
}
#else
void set_thread_name(const char*) {}
#endif

uint64_t monotonic_now_us() {
    return static_cast<uint64_t>(
        std::chrono::duration_cast<std::chrono::microseconds>(
            std::chrono::steady_clock::now().time_since_epoch())
            .count());
}

struct Yuv {
    uint8_t y;
    uint8_t u;
    uint8_t v;
};

// BT.601 limited range (studio swing), the same conversion libyuv uses.
Yuv rgb_to_ycbcr601(uint8_t r, uint8_t g, uint8_t b) {
    return {
        static_cast<uint8_t>(((66 * r + 129 * g + 25 * b + 128) >> 8) + 16),
        static_cast<uint8_t>(((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128),
        static_cast<uint8_t>(((112 * r - 94 * g - 18 * b + 128) >> 8) + 128),
    };
}

// Neutral overlay colours (U = V = 128), so every overlay region carries
// uniform chroma and never bleeds the pattern underneath.
constexpr Yuv kOverlayWhite{235, 128, 128};
constexpr Yuv kOverlayBlack{16, 128, 128};

// 5x7 pixel font for the frame-number burn-in; bit 4 (0x10) is the leftmost
// pixel of each row.
constexpr uint8_t kDigitFont[10][7] = {
    {0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E}, // 0
    {0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E}, // 1
    {0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F}, // 2
    {0x0E, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0E}, // 3
    {0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02}, // 4
    {0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E}, // 5
    {0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E}, // 6
    {0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08}, // 7
    {0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E}, // 8
    {0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C}, // 9
};

// The selectable background patterns.  These names are part of the
// user-facing configuration surface; liveion's CaptureSpec validation
// mirrors this table.
enum class MockPattern { Smpte75, Snow, Black, White, Checkers, Gradient };

// Single table driving both config parsing and the init log line, so the
// name list exists exactly once and the two can never disagree.
constexpr std::pair<MockPattern, const char*> kPatterns[] = {
    {MockPattern::Smpte75, "smpte75"},
    {MockPattern::Snow, "snow"},
    {MockPattern::Black, "black"},
    {MockPattern::White, "white"},
    {MockPattern::Checkers, "checkers"},
    {MockPattern::Gradient, "gradient"},
};

const char* pattern_name(MockPattern pattern) {
    for (const auto& [p, name] : kPatterns) {
        if (p == pattern) return name;
    }
    return "unknown";
}

std::string pattern_list() {
    std::string out;
    for (const auto& entry : kPatterns) {
        if (!out.empty()) out += ", ";
        out += entry.second;
    }
    return out;
}

bool parse_pattern(const std::string& s, MockPattern* out) {
    if (s.empty()) {
        *out = MockPattern::Smpte75;  // backend default
        return true;
    }
    for (const auto& [p, name] : kPatterns) {
        if (s == name) {
            *out = p;
            return true;
        }
    }
    return false;
}

// No real sensor exceeds this; it also keeps the pacing period >= 1 ms so
// the sleep-based scheduler cannot degenerate into a busy loop.
constexpr uint32_t kMaxFps = 1000;

} // namespace

struct MockCaptureImpl : public CaptureBackend {
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t fps = 0;
    MockPattern pattern = MockPattern::Smpte75;

    std::atomic<bool> running{false};
    std::atomic<bool> thread_alive{false};
    CaptureFrameCallback capture_cb_;
    std::thread produce_thread;

    // Static background for the non-animated patterns, rendered once in
    // init(); each frame starts from a copy of it and the verification
    // overlays are drawn on top.  Unused by the per-frame patterns
    // (snow, checkers), which render straight into frame_buf.
    std::vector<uint8_t> base_buf;
    std::vector<uint8_t> frame_buf;
    uint64_t seq = 0;
    uint32_t rng_state = 0x9E3779B9u;

    // Bouncing block state (even-aligned so the chroma blocks stay whole).
    int32_t block_x = 0;
    int32_t block_y = 0;
    int32_t block_dx = 4;
    int32_t block_dy = 2;

    ~MockCaptureImpl() override { stop(); }

    bool init(const CaptureConfig& cfg, std::string* err) override {
        if (cfg.width == 0 || cfg.height == 0 || cfg.fps == 0) {
            if (err) *err = "mock capture requires non-zero width/height/fps";
            return false;
        }
        if (cfg.fps > kMaxFps) {
            if (err) {
                *err = "mock capture fps must be at most "
                       + std::to_string(kMaxFps);
            }
            return false;
        }
        if ((cfg.width & 1U) != 0 || (cfg.height & 1U) != 0) {
            if (err) *err = "mock capture (NV12) requires even width and height";
            return false;
        }
        if (cfg.pixel_format != RawPixelFormat::Nv12) {
            if (err) *err = "mock capture generates NV12 only";
            return false;
        }
        if (!parse_pattern(cfg.pattern, &pattern)) {
            if (err) {
                *err = std::string("mock capture: unknown pattern '")
                       + cfg.pattern + "' (supported: " + pattern_list() + ")";
            }
            return false;
        }
        width = cfg.width;
        height = cfg.height;
        fps = cfg.fps;

        const size_t y_bytes = static_cast<size_t>(width) * height;
        base_buf.resize(y_bytes * 3 / 2);
        frame_buf.resize(base_buf.size());
        render_base();
        fprintf(stderr, "[MockCapture] generating %ux%u NV12 at %u fps, pattern=%s\n",
                width, height, fps, pattern_name(pattern));
        return true;
    }

    bool start(CaptureFrameCallback cb, std::string* err) override {
        if (width == 0) {
            if (err) *err = "capture backend is not initialised";
            return false;
        }
        if (running.load() || produce_thread.joinable()) {
            if (err) *err = "capture already running";
            return false;
        }
        capture_cb_ = std::move(cb);
        seq = 0;
        running.store(true);
        produce_thread = std::thread(&MockCaptureImpl::produce_loop, this);
        return true;
    }

    void stop() override {
        running.store(false);
        if (produce_thread.joinable()) produce_thread.join();
        capture_cb_ = nullptr;
    }

    bool isRunning() const override {
        return running.load() && thread_alive.load();
    }

    RawPixelFormat outputFormat() const override {
        return RawPixelFormat::Nv12;
    }

    // The mock never clamps: it generates exactly the configured size.
    std::pair<uint32_t, uint32_t> negotiatedSize() const override {
        return {0, 0};
    }

    // CPU buffers only; a dmabuf variant can be added later if needed.
    bool emitsDmabufFrames() const override { return false; }

    uint8_t* y_plane() { return frame_buf.data(); }
    uint8_t* uv_plane() { return frame_buf.data() + static_cast<size_t>(width) * height; }

    // Fill an even-aligned rectangle in both planes.  All callers pass
    // neutral colours, and the rect edges are rounded out to whole 2x2
    // chroma blocks.
    void fill_rect(int32_t x, int32_t y, uint32_t w, uint32_t h, Yuv color) {
        const int32_t x0 = std::max<int32_t>(x & ~1, 0);
        const int32_t y0 = std::max<int32_t>(y & ~1, 0);
        const int32_t x1 = std::min<int32_t>(
            (x + static_cast<int32_t>(w) + 1) & ~1, static_cast<int32_t>(width));
        const int32_t y1 = std::min<int32_t>(
            (y + static_cast<int32_t>(h) + 1) & ~1, static_cast<int32_t>(height));
        if (x1 <= x0 || y1 <= y0) return;
        for (int32_t row = y0; row < y1; ++row) {
            std::memset(y_plane() + static_cast<size_t>(row) * width + x0,
                        color.y, static_cast<size_t>(x1 - x0));
        }
        for (int32_t row = y0 / 2; row < y1 / 2; ++row) {
            uint8_t* uv = uv_plane() + static_cast<size_t>(row) * width + x0;
            for (int32_t col = 0; col < (x1 - x0) / 2; ++col) {
                uv[col * 2] = color.u;
                uv[col * 2 + 1] = color.v;
            }
        }
    }

    // Static 75% SMPTE bars: white, yellow, cyan, green, magenta, red,
    // blue, black.  Written once; the per-frame overlays are painted on a
    // copy of this buffer.
    void render_base() {
        switch (pattern) {
        case MockPattern::Smpte75:
            render_bars();
            return;
        case MockPattern::Black:
            render_solid(16);
            return;
        case MockPattern::White:
            render_solid(235);
            return;
        case MockPattern::Gradient:
            render_gradient();
            return;
        case MockPattern::Snow:
        case MockPattern::Checkers:
            // Animated patterns render per frame straight into frame_buf.
            return;
        }
    }

    void render_bars() {
        static const Yuv bars[8] = {
            rgb_to_ycbcr601(191, 191, 191), // white
            rgb_to_ycbcr601(191, 191, 0),   // yellow
            rgb_to_ycbcr601(0, 191, 191),   // cyan
            rgb_to_ycbcr601(0, 191, 0),     // green
            rgb_to_ycbcr601(191, 0, 191),   // magenta
            rgb_to_ycbcr601(191, 0, 0),     // red
            rgb_to_ycbcr601(0, 0, 191),     // blue
            rgb_to_ycbcr601(0, 0, 0),       // black
        };
        const uint32_t bar_w = std::max(width / 8, 1U);
        uint8_t* y = base_buf.data();
        uint8_t* uv = base_buf.data() + static_cast<size_t>(width) * height;
        for (uint32_t row = 0; row < height; ++row) {
            for (uint32_t x = 0; x < width; ++x) {
                y[static_cast<size_t>(row) * width + x] =
                    bars[std::min(x / bar_w, 7U)].y;
            }
        }
        for (uint32_t row = 0; row < height / 2; ++row) {
            for (uint32_t cx = 0; cx < width / 2; ++cx) {
                const Yuv& c = bars[std::min(cx * 2 / bar_w, 7U)];
                uv[static_cast<size_t>(row) * width + cx * 2] = c.u;
                uv[static_cast<size_t>(row) * width + cx * 2 + 1] = c.v;
            }
        }
    }

    // Solid frame at the given (neutral-chroma) luma level.
    void render_solid(uint8_t level) {
        std::memset(base_buf.data(), level, static_cast<size_t>(width) * height);
        std::memset(base_buf.data() + static_cast<size_t>(width) * height, 128,
                    static_cast<size_t>(width) * height / 2);
    }

    // Horizontal limited-range luma ramp, neutral chroma — banding test.
    void render_gradient() {
        uint8_t* y = base_buf.data();
        for (uint32_t x = 0; x < width; ++x) {
            y[x] = static_cast<uint8_t>(
                16 + (219 * x) / (width - 1));
        }
        for (uint32_t row = 1; row < height; ++row) {
            std::memcpy(y + static_cast<size_t>(row) * width, y, width);
        }
        std::memset(base_buf.data() + static_cast<size_t>(width) * height, 128,
                    static_cast<size_t>(width) * height / 2);
    }

    // Full-range luma white noise, new every frame (chroma neutral) — the
    // worst case for an encoder: nothing is predictable.
    void render_snow() {
        uint8_t* y = y_plane();
        const size_t y_bytes = static_cast<size_t>(width) * height;
        uint32_t s = rng_state;
        for (size_t i = 0; i < y_bytes; ++i) {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            y[i] = static_cast<uint8_t>(s & 0xFF);
        }
        rng_state = s;
        std::memset(uv_plane(), 128, y_bytes / 2);
    }

    // Checkerboard drifting one pixel down-right per frame (chroma
    // neutral) — sharp edges plus sub-pixel motion.  The shift wraps every
    // two cells, so the loop is seamless.
    void render_checkers(uint64_t frame_number) {
        const uint32_t cell = std::max((width / 16) & ~1U, 2U);
        const uint32_t off = static_cast<uint32_t>(frame_number % (2 * cell));
        std::memset(uv_plane(), 128, static_cast<size_t>(width) * height / 2);
        for (uint32_t row = 0; row < height; ++row) {
            const uint32_t row_parity = ((row + off) / cell) & 1U;
            uint8_t* y = y_plane() + static_cast<size_t>(row) * width;
            uint32_t x = 0;
            while (x < width) {
                const uint32_t cell_index = (x + off) / cell;
                uint32_t end = (cell_index + 1) * cell - off;
                end = std::min(end, width);
                const uint8_t v = ((cell_index & 1U) ^ row_parity) != 0 ? 235 : 16;
                std::memset(y + x, v, end - x);
                x = end;
            }
        }
    }

    // White block bouncing off the frame edges; its motion makes the actual
    // delivered framerate (and any stall) visible by eye.
    void draw_block() {
        const int32_t block_w = static_cast<int32_t>(std::max(width / 8, 2U));
        const int32_t block_h = static_cast<int32_t>(std::max(height / 8, 2U));
        block_x += block_dx;
        block_y += block_dy;
        if (block_x <= 0 || block_x + block_w >= static_cast<int32_t>(width)) {
            block_dx = -block_dx;
            block_x = std::clamp(block_x, 0, static_cast<int32_t>(width) - block_w);
        }
        if (block_y <= 0 || block_y + block_h >= static_cast<int32_t>(height)) {
            block_dy = -block_dy;
            block_y = std::clamp(block_y, 0, static_cast<int32_t>(height) - block_h);
        }
        fill_rect(block_x, block_y, static_cast<uint32_t>(block_w),
                  static_cast<uint32_t>(block_h), kOverlayWhite);
    }

    // 32-bit binary frame counter strip along the bottom: cell i shows bit
    // i of the frame number (LSB leftmost), white when set, black when
    // clear.  Decoded output can be checked frame-by-frame against it to
    // measure the exact delivered rate and spot dropped/repeated frames.
    // cell_w is kept even so adjacent cells never overlap after fill_rect's
    // chroma alignment.
    void draw_counter(uint64_t frame_number) {
        const uint32_t cell_w = std::max((width / 32) & ~1U, 2U);
        const uint32_t cell_h = std::max(height / 24, 2U);
        const int32_t top = static_cast<int32_t>(height - cell_h);
        fill_rect(0, top, width, cell_h, kOverlayBlack);
        for (uint32_t bit = 0; bit < 32; ++bit) {
            if (((frame_number >> bit) & 1U) == 0) continue;
            fill_rect(static_cast<int32_t>(bit * cell_w), top, cell_w, cell_h,
                      kOverlayWhite);
        }
    }

    // Decimal frame-number burn-in at the top-left, white 5x7 digits at 2x
    // scale on a black backing box.
    void draw_frame_text(uint64_t frame_number) {
        char text[24];
        std::snprintf(text, sizeof(text), "%llu",
                      static_cast<unsigned long long>(frame_number));
        constexpr int32_t kScale = 2;
        constexpr int32_t kDigitW = 5 * kScale;
        constexpr int32_t kDigitH = 7 * kScale;
        constexpr int32_t kSpacing = 2;
        const int32_t count = static_cast<int32_t>(std::strlen(text));
        const int32_t ox = 8;
        const int32_t oy = 8;
        fill_rect(ox - 4, oy - 4,
                  static_cast<uint32_t>(count * (kDigitW + kSpacing) - kSpacing + 8),
                  kDigitH + 8, kOverlayBlack);
        for (int32_t i = 0; i < count; ++i) {
            const uint8_t* glyph = kDigitFont[text[i] - '0'];
            const int32_t gx = ox + i * (kDigitW + kSpacing);
            for (int32_t row = 0; row < 7; ++row) {
                for (int32_t col = 0; col < 5; ++col) {
                    if ((glyph[row] >> (4 - col)) & 1U) {
                        fill_rect(gx + col * kScale, oy + row * kScale,
                                  kScale, kScale, kOverlayWhite);
                    }
                }
            }
        }
    }

    void render(uint64_t frame_number) {
        switch (pattern) {
        case MockPattern::Snow:
            render_snow();
            break;
        case MockPattern::Checkers:
            render_checkers(frame_number);
            break;
        default:
            std::memcpy(frame_buf.data(), base_buf.data(), frame_buf.size());
            break;
        }
        // Overlays always win over the background pattern, so the
        // verification elements stay readable on every pattern.
        draw_block();
        draw_counter(frame_number);
        draw_frame_text(frame_number);
    }

    void produce_loop() {
        set_thread_name("mock-capture");
        fprintf(stderr, "[MockCapture] produce thread started\n");
        thread_alive.store(true);

        // Rational schedule: frame k is due at base + (k - base_seq) * 1e6 /
        // fps microseconds — the per-frame rounding (<= 1 us) does not
        // accumulate, so the long-term average rate is exactly fps.
        const auto period = std::chrono::microseconds(1000000 / fps);
        auto base = std::chrono::steady_clock::now();
        uint64_t base_seq = 1;  // `base` is the due time of frame base_seq
        while (running.load()) {
            ++seq;
            render(seq);
            RawFrame frame{};
            frame.kind = BufferKind::Cpu;
            frame.format = RawPixelFormat::Nv12;
            frame.width = width;
            frame.height = height;
            frame.pts_us = monotonic_now_us();
            frame.seq = seq;
            frame.plane_count = 2;
            const uint32_t y_bytes =
                static_cast<uint32_t>(static_cast<size_t>(width) * height);
            frame.planes[0] = {y_plane(), width, y_bytes, -1, 0};
            frame.planes[1] = {uv_plane(), width, y_bytes / 2, -1, y_bytes};
            if (capture_cb_) capture_cb_(frame);

            auto next = base
                + std::chrono::microseconds((seq + 1 - base_seq) * 1000000 / fps);
            const auto now = std::chrono::steady_clock::now();
            if (next + period < now) {
                // The callback (encoder) overran by more than a full frame
                // period: resync the schedule on the next frame instead of
                // bursting a long backlog.  A shorter overrun emits one
                // immediate catch-up frame and keeps the schedule.
                base = now;
                base_seq = seq + 1;
                next = now;
            }
            std::this_thread::sleep_until(next);
        }

        running.store(false);
        thread_alive.store(false);
        fprintf(stderr, "[MockCapture] produce thread stopped\n");
    }
};

std::shared_ptr<CaptureBackend> create_mock_capture_backend(
    const CaptureConfig& cfg) {
    (void)cfg;
    return std::make_shared<MockCaptureImpl>();
}
