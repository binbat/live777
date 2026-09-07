#include "include/encoder_stall_detector.h"

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
    constexpr uint64_t kB = 1000000000ULL;

    // --- progress restarts the window -------------------------------------
    {
        StallDetector d;
        CHECK(!d.observe(kB, 0));               // == initial: no progress yet
        CHECK(d.last_progress_us == 0);
        CHECK(d.observe(kB, 1));                // first real completion
        CHECK(d.last_progress_us == kB);        // baseline established
        CHECK(!d.due(kB + 500000, 3));          // 0.5 s of silence: not yet
        CHECK(d.observe(kB + 1000000, 2));      // completion arrives: restart
        CHECK(d.last_progress_us == kB + 1000000);
        CHECK(!d.due(kB + 2000000, 3));         // 1 s after progress: not yet
    }

    // --- sustained stall with frames in flight escalates -------------------
    {
        StallDetector d;
        d.observe(kB, 5);
        CHECK(!d.due(kB + 2999999, 2));
        CHECK(d.due(kB + 3000000, 2));          // exactly stall_us
        CHECK(!d.due(kB + 3000000, 0));         // nothing in flight: no stall
        CHECK(!d.due(kB + 3000000, -1));        // negative depth: no stall
    }

    // --- cooldown suppresses repeated resets ------------------------------
    {
        StallDetector d;
        d.observe(kB, 5);
        CHECK(d.due(kB + 3000000, 2));
        d.mark_reset(kB + 3000000);
        CHECK(d.in_cooldown(kB + 3000000));
        CHECK(!d.due(kB + 6000000, 2));         // 3 s later, still cooling
        CHECK(d.in_cooldown(kB + 6000000));
        CHECK(!d.due(kB + 7999999, 2));         // just before cooldown ends
        CHECK(d.due(kB + 8000000, 2));          // cooldown elapsed: due again
        CHECK(!d.in_cooldown(kB + 8000000));
    }

    // --- stall duration reporting ------------------------------------------
    {
        StallDetector d;
        d.observe(kB, 5);
        d.mark_reset(kB + 3000000);
        CHECK(d.stall_duration_us(kB + 4200000) == 4200000);
        d.observe(kB + 5000000, 6);             // recovery resets progress
        CHECK(d.stall_duration_us(kB + 5200000) == 200000);
    }

    // --- prime(): a never-completing context is stalled too -----------------
    {
        StallDetector d;
        d.prime(kB);                            // (re)open: no completion yet
        CHECK(!d.due(kB + 2999999, 2));
        CHECK(d.due(kB + 3000000, 2));          // silence since open == stall
        CHECK(!d.due(kB + 3000000, 0));         // still needs in-flight depth
        CHECK(d.stall_duration_us(kB + 4200000) == 4200000);
        CHECK(d.observe(kB + 4000000, 1));      // first completion re-baselines
        CHECK(d.last_progress_us == kB + 4000000);
        CHECK(!d.due(kB + 5000000, 2));
    }

    // --- prime() preserves the reset cooldown across a rebuild --------------
    {
        StallDetector d;
        d.observe(kB, 5);
        CHECK(d.due(kB + 3000000, 2));
        d.mark_reset(kB + 3000000);             // stall → reset → rebuild
        d.prime(kB + 4000000);                  // rebuilt context opens 1 s on
        CHECK(!d.due(kB + 7000000, 2));         // 3 s silent, but cooling down
        CHECK(d.due(kB + 8000000, 2));          // 5 s after the reset: due
    }

    std::fprintf(stderr, "encoder-stall-test: all checks passed\n");
    return 0;
}
