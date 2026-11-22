//! MockCaptureImpl — synthetic NV12 frame source for camera-free testing of
//! the hardware encoder backends (issue live777#519).
//!
//! Generates NV12 (two-plane, CPU buffers) frames on a pacing thread at the
//! configured rate.  No device is opened; the encoder path runs end-to-end
//! exactly as with a real capture backend, so encoder bring-up on a new SoC
//! can be validated with no camera attached.
//!
//! The pattern is designed for verification from the decoded output:
//!   * static 75% SMPTE color bars — validates color/matrix handling,
//!   * a white block bouncing around the frame — makes the delivered
//!     framerate and gaps visible,
//!   * a 32-bit binary frame counter strip at the bottom — increments by
//!     one every frame, so dropped/repeated frames are directly observable,
//!   * the decimal frame number burned into the top-left corner.

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
// uniform chroma and never bleeds the colour bars underneath.
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

} // namespace

struct MockCaptureImpl : public CaptureBackend {
    uint32_t width = 0;
    uint32_t height = 0;
    uint32_t fps = 0;

    std::atomic<bool> running{false};
    std::atomic<bool> thread_alive{false};
    CaptureFrameCallback capture_cb_;
    std::thread produce_thread;

    // Static colour-bar background, rendered once in init(); each frame
    // starts from a copy of it and the moving overlays are drawn on top.
    std::vector<uint8_t> base_buf;
    std::vector<uint8_t> frame_buf;
    uint64_t seq = 0;

    // Bouncing block state (even-aligned so the chroma blocks stay whole).
    int32_t box_x = 0;
    int32_t box_y = 0;
    int32_t box_dx = 4;
    int32_t box_dy = 2;

    ~MockCaptureImpl() override { stop(); }

    bool init(const CaptureConfig& cfg, std::string* err) override {
        if (cfg.width == 0 || cfg.height == 0 || cfg.fps == 0) {
            if (err) *err = "mock capture requires non-zero width/height/fps";
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
        width = cfg.width;
        height = cfg.height;
        fps = cfg.fps;

        const size_t y_bytes = static_cast<size_t>(width) * height;
        base_buf.resize(y_bytes * 3 / 2);
        frame_buf.resize(base_buf.size());
        render_base();
        fprintf(stderr, "[MockCapture] generating %ux%u NV12 at %u fps\n",
                width, height, fps);
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

    // White block bouncing off the frame edges; its motion makes the actual
    // delivered framerate (and any stall) visible by eye.
    void draw_box() {
        const int32_t box_w = static_cast<int32_t>(std::max(width / 8, 2U));
        const int32_t box_h = static_cast<int32_t>(std::max(height / 8, 2U));
        box_x += box_dx;
        box_y += box_dy;
        if (box_x <= 0 || box_x + box_w >= static_cast<int32_t>(width)) {
            box_dx = -box_dx;
            box_x = std::clamp(box_x, 0, static_cast<int32_t>(width) - box_w);
        }
        if (box_y <= 0 || box_y + box_h >= static_cast<int32_t>(height)) {
            box_dy = -box_dy;
            box_y = std::clamp(box_y, 0, static_cast<int32_t>(height) - box_h);
        }
        fill_rect(box_x, box_y, static_cast<uint32_t>(box_w),
                  static_cast<uint32_t>(box_h), kOverlayWhite);
    }

    // 32-bit binary frame counter strip along the bottom: cell i shows bit
    // i of the frame number (LSB leftmost), white when set, black when
    // clear.  Decoded output can be checked frame-by-frame against it to
    // measure the exact delivered rate and spot dropped/repeated frames.
    void draw_counter(uint64_t frame_number) {
        const uint32_t cell_w = std::max(width / 32, 2U);
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
        std::memcpy(frame_buf.data(), base_buf.data(), frame_buf.size());
        draw_box();
        draw_counter(frame_number);
        draw_frame_text(frame_number);
    }

    void produce_loop() {
        set_thread_name("mock-capture");
        fprintf(stderr, "[MockCapture] produce thread started\n");
        thread_alive.store(true);

        const auto period = std::chrono::microseconds(1000000 / fps);
        auto next = std::chrono::steady_clock::now();
        while (running.load()) {
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
            ++seq;

            next += period;
            const auto now = std::chrono::steady_clock::now();
            if (next + period < now) {
                // The callback (encoder) overran badly: resync instead of
                // bursting to catch up.
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
