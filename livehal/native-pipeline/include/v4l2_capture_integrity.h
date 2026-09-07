//! V4L2 capture integrity helpers — platform-neutral, unit-testable.
//!
//! Extracted from v4l2_capture.cpp so the frame-integrity gate (malformed
//! plane detection, sustained-failure escalation) and the aggregated
//! bad-frame / sequence-gap logging can be unit-tested without a V4L2
//! device (see tests/v4l2_integrity_test.cpp).

#pragma once

#include <cstdint>
#include <cstdio>

// Malformed-frame test for the capture integrity gate.  A plane is
// malformed when bytesused is out of the expected range for the negotiated
// sizeimage: zero (driver error flag), larger than the buffer, or far
// shorter than sizeimage (truncated write — the MIPI CRC pattern observed
// on RV1126B).  Small shortfalls are tolerated because some drivers report
// bytesused excluding padding rows; want == 0 means the driver did not
// populate sizeimage, so nothing can be validated.
inline bool plane_malformed(uint32_t got, uint32_t want) {
    if (want == 0) return false;
    if (got == 0 || got > want) return true;
    return static_cast<uint64_t>(got)
        < static_cast<uint64_t>(want) * 9 / 10;
}

// Aggregated bad-frame / driver sequence-gap accounting, logged once per
// second (or forced at stop).  Owned by the capture thread only.
struct IntegrityTracker {
    uint64_t bad_frames = 0;
    uint64_t seq_gaps = 0;
    uint64_t seq_gap_frames = 0;
    uint32_t last_bad_plane = 0;
    uint32_t last_bad_got = 0;
    uint32_t last_bad_want = 0;
    uint32_t expected_seq = 0;
    bool seq_valid = false;
    uint64_t window_start_us = 0;

    void reset() {
        bad_frames = 0;
        seq_gaps = 0;
        seq_gap_frames = 0;
        last_bad_plane = 0;
        last_bad_got = 0;
        last_bad_want = 0;
        expected_seq = 0;
        seq_valid = false;
        window_start_us = 0;
    }

    void record_bad_frame(uint32_t plane, uint32_t got, uint32_t want) {
        ++bad_frames;
        last_bad_plane = plane;
        last_bad_got = got;
        last_bad_want = want;
    }

    void record_sequence(uint32_t sequence) {
        if (seq_valid && sequence != expected_seq) {
            if (sequence > expected_seq) {
                // Forward jump: the driver dropped frames.
                ++seq_gaps;
                seq_gap_frames += sequence - expected_seq;
            }
            // sequence < expected_seq: the driver re-armed/reset its
            // counter or returned buffers out of order.  Re-sync instead
            // of accumulating a wrapped ~2^32 frame "gap".
        }
        expected_seq = sequence + 1;
        seq_valid = true;
    }

    void flush(uint64_t now_us, bool force = false) {
        if (!bad_frames && !seq_gaps) return;
        if (window_start_us == 0) {
            window_start_us = now_us;
        }
        if (!force && now_us - window_start_us < 1000000ULL) return;

        fprintf(stderr,
                "[V4L2Capture] BAD-FRAME: count=%llu (last: plane%u "
                "bytesused=%u want=%u), driver seq gaps=%llu "
                "(~%llu frames lost)\n",
                static_cast<unsigned long long>(bad_frames),
                last_bad_plane, last_bad_got, last_bad_want,
                static_cast<unsigned long long>(seq_gaps),
                static_cast<unsigned long long>(seq_gap_frames));
        bad_frames = 0;
        seq_gaps = 0;
        seq_gap_frames = 0;
        last_bad_plane = 0;
        last_bad_got = 0;
        last_bad_want = 0;
        window_start_us = now_us;
    }
};

// Sustained all-malformed escalation.  If every frame is malformed for
// kFatalUs the sensor/driver link is dead and the caller should stop the
// capture thread so the pipeline/watchdog can recover.  A good frame, or a
// quiet gap longer than kGapUs between malformed frames, restarts the
// window — sparse glitches must not trip the escalation.
struct ConsecutiveBadWindow {
    static constexpr uint64_t kFatalUs = 2000000;  // 2 s of sustained failure
    static constexpr uint64_t kGapUs = 1000000;    // 1 s quiet gap restarts

    uint64_t frames = 0;
    uint64_t window_start_us = 0;  // start of the current sustained run
    uint64_t last_bad_us = 0;      // timestamp of the previous bad frame

    void reset() {
        frames = 0;
        window_start_us = 0;
        last_bad_us = 0;
    }

    void on_good_frame() { reset(); }

    /// Returns true once the sustained-failure threshold is reached.
    bool on_bad_frame(uint64_t now_us) {
        ++frames;
        // Start a fresh window on the first bad frame or whenever the gap
        // since the previous bad frame exceeds kGapUs (measured against the
        // previous frame, not the window start, so a high-rate sustained run
        // is not mistaken for a quiet gap).
        if (window_start_us == 0 || now_us - last_bad_us > kGapUs) {
            window_start_us = now_us;
        }
        last_bad_us = now_us;
        return now_us - window_start_us >= kFatalUs;
    }
};
