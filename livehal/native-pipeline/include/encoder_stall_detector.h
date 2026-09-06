//! Stall-detection state machine for hardware encoders — platform-neutral
//! and unit-testable (see tests/encoder_stall_test.cpp).
//!
//! Semantics (mirrors the original inline logic in encoder_rkmpp.cpp):
//! an encoder is considered stalled when frames are in flight but no
//! completion has been observed for `stall_us`, and no reset happened
//! within the last `cooldown_us`.  All state is owned by one monitor
//! thread (plus the stall_us/cooldown_us setup performed under the same
//! mutex the monitor runs under), so no atomics are needed.

#pragma once

#include <cstdint>

struct StallDetector {
    // Window (µs) without a completion that declares a stall.  Scaled to
    // the configured framerate by the encoder (a fixed window false-
    // positives on low-fps sources).
    uint64_t stall_us = 3000000;      // 3 s default
    uint64_t cooldown_us = 5000000;   // 5 s between resets

    // Observation state.
    uint64_t last_completed = 0;      // last seen completion counter
    uint64_t last_progress_us = 0;    // when the last completion was seen
    uint64_t last_reset_us = 0;       // when the last reset was issued

    void reset() {
        last_completed = 0;
        last_progress_us = 0;
        last_reset_us = 0;
    }

    /// Feed one monitor tick.  Returns true when the completion counter
    /// advanced (progress — the stall window restarts).
    bool observe(uint64_t now_us, uint64_t completed) {
        if (completed != last_completed) {
            last_completed = completed;
            last_progress_us = now_us;
            return true;
        }
        return false;
    }

    /// True when a reset is warranted: no progress for >= stall_us and the
    /// reset cooldown has elapsed.  No side effects.
    bool due(uint64_t now_us, int64_t in_flight_depth) const {
        if (last_progress_us == 0 || in_flight_depth <= 0) return false;
        if (now_us - last_progress_us < stall_us) return false;
        if (now_us - last_reset_us < cooldown_us) return false;
        return true;
    }

    /// True between resets (used to rate-limit "stall persists" logging).
    bool in_cooldown(uint64_t now_us) const {
        return now_us - last_reset_us < cooldown_us;
    }

    void mark_reset(uint64_t now_us) { last_reset_us = now_us; }

    /// How long the encoder has been without a completion (for logging).
    uint64_t stall_duration_us(uint64_t now_us) const {
        return last_progress_us ? now_us - last_progress_us : 0;
    }
};
