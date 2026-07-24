//! Reconnect backoff policy shared by the reconnect-capable sources
//! (RTSP/WHEP) and the static WHIP push targets.

use std::time::Duration;

/// Delay before reconnect `attempt` (1-based): exponential backoff from a
/// 5 s base, capped at 60 s (5 s, 10 s, 20 s, 40 s, 60 s, …).
pub(crate) fn reconnect_delay(attempt: u32) -> Duration {
    const RECONNECT_BASE_MS: u64 = 5_000;
    const RECONNECT_MAX_MS: u64 = 60_000;
    let shift = attempt.saturating_sub(1).min(4);
    Duration::from_millis(
        RECONNECT_BASE_MS
            .saturating_mul(1u64 << shift)
            .min(RECONNECT_MAX_MS),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_delay_doubles_with_cap() {
        assert_eq!(reconnect_delay(1), Duration::from_millis(5_000));
        assert_eq!(reconnect_delay(2), Duration::from_millis(10_000));
        assert_eq!(reconnect_delay(3), Duration::from_millis(20_000));
        assert_eq!(reconnect_delay(4), Duration::from_millis(40_000));
        assert_eq!(reconnect_delay(5), Duration::from_millis(60_000));
        // Capped afterwards, and saturating on huge attempt counts.
        assert_eq!(reconnect_delay(6), Duration::from_millis(60_000));
        assert_eq!(reconnect_delay(u32::MAX), Duration::from_millis(60_000));
    }
}
