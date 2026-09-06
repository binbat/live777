#include "include/v4l2_capture_integrity.h"

#include <cstdint>
#include <cstdio>

#define CHECK(condition)                                                        \
    do {                                                                        \
        if (!(condition)) {                                                     \
            std::fprintf(stderr, "check failed at line %d: %s\n", __LINE__,   \
                         #condition);                                           \
            return 1;                                                           \
        }                                                                       \
    } while (false)

int main() {
    // --- plane_malformed --------------------------------------------------
    CHECK(plane_malformed(0, 100));          // driver error flag / empty
    CHECK(plane_malformed(101, 100));        // larger than the buffer
    CHECK(plane_malformed(89, 100));         // truncated below 90%
    CHECK(plane_malformed(1, 1000));         // tiny fraction of sizeimage
    CHECK(!plane_malformed(90, 100));        // 90% is tolerated (padding)
    CHECK(!plane_malformed(100, 100));       // exact match
    CHECK(!plane_malformed(9999, 0));        // want==0: nothing to validate
    CHECK(!plane_malformed(0, 0));

    // --- IntegrityTracker: sequence gaps ---------------------------------
    {
        IntegrityTracker t;
        t.record_sequence(10);
        t.record_sequence(11);               // in order: no gap
        t.record_sequence(20);               // forward jump: frames 12..19 lost
        CHECK(t.seq_gaps == 1);
        CHECK(t.seq_gap_frames == 8);
        t.record_sequence(2);                // driver re-arm (backwards):
        CHECK(t.seq_gaps == 1);              // re-sync, no wrapped ~2^32 gap
        CHECK(t.seq_gap_frames == 8);
        t.record_sequence(3);                // follows the re-sync point
        CHECK(t.seq_gaps == 1);
        CHECK(t.seq_gap_frames == 8);
    }

    // --- IntegrityTracker: bad-frame aggregation window ------------------
    {
        IntegrityTracker t;
        t.record_bad_frame(0, 100, 640);
        t.flush(1000000);                    // < 1 s since window start: hold
        CHECK(t.bad_frames == 1);
        t.flush(3000000);                    // >= 1 s: flush and reset
        CHECK(t.bad_frames == 0);
        CHECK(t.window_start_us == 3000000);
        // force flush with nothing pending is a no-op
        t.flush(4000000, true);
        CHECK(t.bad_frames == 0);
    }

    // --- ConsecutiveBadWindow: sustained failure escalates ----------------
    {
        constexpr uint64_t kBase = 1000000000ULL;
        ConsecutiveBadWindow w;
        CHECK(!w.on_bad_frame(kBase));               // 1st bad frame
        CHECK(!w.on_bad_frame(kBase + 500000));      // 0.5 s later
        CHECK(!w.on_bad_frame(kBase + 1000000));     // 1.0 s later
        CHECK(!w.on_bad_frame(kBase + 1500000));     // 1.5 s later
        CHECK(w.on_bad_frame(kBase + 2100000));      // 2.1 s: sustained => fatal
    }

    // --- ConsecutiveBadWindow: good frame resets --------------------------
    {
        constexpr uint64_t kBase = 2000000000ULL;
        ConsecutiveBadWindow w;
        w.on_bad_frame(kBase);
        w.on_good_frame();
        CHECK(w.frames == 0);
        CHECK(!w.on_bad_frame(kBase + 100));         // fresh window
        CHECK(!w.on_bad_frame(kBase + 100 + 2100000));  // 2.1 s span, but only
                                                       // 2 frames: not sustained
    }

    // --- ConsecutiveBadWindow: quiet gap restarts the window ---------------
    {
        constexpr uint64_t kBase = 3000000000ULL;
        ConsecutiveBadWindow w;
        w.on_bad_frame(kBase);                       // first bad frame
        // Second bad frame 3.1 s later: gap > kGapUs, so the window
        // restarts and the elapsed time since the FIRST frame must not
        // trip the escalation (would have under the old wall-time logic).
        CHECK(!w.on_bad_frame(kBase + 3100000));
        // A sustained run (<= 1 s gaps) inside the restarted window still
        // escalates once it spans kFatalUs.
        CHECK(!w.on_bad_frame(kBase + 3100000 + 700000));    // +0.7 s
        CHECK(!w.on_bad_frame(kBase + 3100000 + 1400000));   // +1.4 s
        CHECK(w.on_bad_frame(kBase + 3100000 + 2100000));    // +2.1 s: fatal
    }

    std::fprintf(stderr, "v4l2-integrity-test: all checks passed\n");
    return 0;
}
