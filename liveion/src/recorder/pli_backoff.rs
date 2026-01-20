use std::time::{Duration, Instant};

/// PLI (Picture Loss Indication) request backoff strategy
///
/// This module implements an intelligent backoff mechanism for requesting keyframes
/// during video recording/streaming to avoid overwhelming the encoder with requests
/// while ensuring timely keyframe delivery.
#[derive(Debug, Clone)]
pub struct PliBackoff {
    /// Initial timeout before first PLI request
    initial_timeout: Duration,

    /// Maximum timeout between PLI requests (cap for exponential backoff)
    max_timeout: Duration,

    /// Current timeout duration (grows with exponential backoff)
    current_timeout: Duration,

    /// Timestamp of last keyframe received
    last_keyframe_time: Option<Instant>,

    /// Timestamp of last PLI request sent
    last_request_time: Option<Instant>,

    /// Number of consecutive PLI requests sent without receiving a keyframe
    request_count: u32,

    /// Maximum number of PLI requests to send before giving up (0 = unlimited)
    max_requests: u32,

    /// Backoff multiplier (e.g., 2.0 for exponential backoff)
    backoff_multiplier: f64,

    /// Whether exponential backoff is enabled
    use_exponential: bool,

    /// Total number of PLI requests sent (statistics)
    total_requests: u64,

    /// Total number of successful keyframes received (statistics)
    total_keyframes: u64,
}

impl Default for PliBackoff {
    fn default() -> Self {
        Self::new(
            Duration::from_secs(5),  // Initial timeout: 5s
            Duration::from_secs(30), // Max timeout: 30s
            5,                       // Max 5 consecutive requests
            2.0,                     // Double timeout on each retry
            true,                    // Use exponential backoff
        )
    }
}

impl PliBackoff {
    /// Create a new PLI backoff strategy
    ///
    /// # Arguments
    /// * `initial_timeout` - Time to wait before first PLI request
    /// * `max_timeout` - Maximum timeout (cap for exponential growth)
    /// * `max_requests` - Max consecutive requests (0 = unlimited)
    /// * `backoff_multiplier` - Multiplier for exponential backoff (e.g., 2.0)
    /// * `use_exponential` - Enable exponential backoff (false = fixed retry)
    pub fn new(
        initial_timeout: Duration,
        max_timeout: Duration,
        max_requests: u32,
        backoff_multiplier: f64,
        use_exponential: bool,
    ) -> Self {
        Self {
            initial_timeout,
            max_timeout,
            current_timeout: initial_timeout,
            last_keyframe_time: None,
            last_request_time: None,
            request_count: 0,
            max_requests,
            backoff_multiplier,
            use_exponential,
            total_requests: 0,
            total_keyframes: 0,
        }
    }

    /// Check if we should send a PLI request now
    ///
    /// Returns `true` if:
    /// - No keyframe has been received yet, OR
    /// - Time since last keyframe exceeds current timeout, AND
    /// - We haven't exceeded max request count
    pub fn should_request(&self) -> bool {
        // If we've sent max requests without success, stop requesting
        if self.max_requests > 0 && self.request_count >= self.max_requests {
            return false;
        }

        match self.last_keyframe_time {
            // No keyframe received yet - always request (but respect request cooldown)
            None => match self.last_request_time {
                None => true, // Never requested, request now
                Some(last_req) => last_req.elapsed() >= self.current_timeout,
            },
            // Keyframe received - check if timeout elapsed
            Some(last_kf) => {
                let elapsed = last_kf.elapsed();
                elapsed >= self.current_timeout
            }
        }
    }

    /// Record that a PLI request was sent
    ///
    /// This updates internal state and applies backoff if needed
    pub fn record_request(&mut self) {
        self.last_request_time = Some(Instant::now());
        self.request_count += 1;
        self.total_requests += 1;

        // Apply backoff for next request
        if self.use_exponential && self.request_count > 0 {
            let new_timeout = Duration::from_secs_f64(
                self.current_timeout.as_secs_f64() * self.backoff_multiplier,
            );
            self.current_timeout = new_timeout.min(self.max_timeout);
        }
    }

    /// Record that a keyframe was received
    ///
    /// This resets the backoff state to initial values
    pub fn record_keyframe(&mut self) {
        self.last_keyframe_time = Some(Instant::now());
        self.total_keyframes += 1;

        // Reset backoff on successful keyframe
        self.reset_backoff();
    }

    /// Reset backoff to initial state (after successful keyframe)
    fn reset_backoff(&mut self) {
        self.current_timeout = self.initial_timeout;
        self.request_count = 0;
        self.last_request_time = None;
    }

    /// Get time since last keyframe (if any)
    pub fn time_since_keyframe(&self) -> Option<Duration> {
        self.last_keyframe_time.map(|t| t.elapsed())
    }

    /// Get time since last PLI request (if any)
    pub fn time_since_request(&self) -> Option<Duration> {
        self.last_request_time.map(|t| t.elapsed())
    }

    /// Get success rate (keyframes / requests)
    pub fn success_rate(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.total_keyframes as f64 / self.total_requests as f64
        }
    }

    /// Force reset the entire backoff state (useful for stream reconnection, etc.)
    pub fn hard_reset(&mut self) {
        self.last_keyframe_time = None;
        self.last_request_time = None;
        self.request_count = 0;
        self.current_timeout = self.initial_timeout;
    }

    /// Get a summary of current state for logging/debugging
    pub fn state_summary(&self) -> String {
        format!(
            "PLI Backoff State: requests={}/{}, timeout={:.1}s, success_rate={:.2}%, \
             time_since_kf={}, time_since_req={}",
            self.request_count,
            if self.max_requests == 0 {
                "âˆ?.to_string()
            } else {
                self.max_requests.to_string()
            },
            self.current_timeout.as_secs_f64(),
            self.success_rate() * 100.0,
            self.time_since_keyframe()
                .map(|d| format!("{:.1}s", d.as_secs_f64()))
                .unwrap_or_else(|| "N/A".to_string()),
            self.time_since_request()
                .map(|d| format!("{:.1}s", d.as_secs_f64()))
                .unwrap_or_else(|| "N/A".to_string()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn test_initial_request() {
        let backoff = PliBackoff::default();
        assert!(backoff.should_request(), "Should request initially");
    }

    #[test]
    fn test_exponential_backoff() {
        let mut backoff = PliBackoff::new(
            Duration::from_secs(1),
            Duration::from_secs(10),
            5,
            2.0,
            true,
        );

        assert_eq!(backoff.current_timeout, Duration::from_secs(1));

        backoff.record_request();
        assert_eq!(backoff.current_timeout, Duration::from_secs(2));

        backoff.record_request();
        assert_eq!(backoff.current_timeout, Duration::from_secs(4));

        backoff.record_request();
        assert_eq!(backoff.current_timeout, Duration::from_secs(8));

        backoff.record_request();
        // Should cap at max_timeout
        assert_eq!(backoff.current_timeout, Duration::from_secs(10));
    }

    #[test]
    fn test_keyframe_resets_backoff() {
        let mut backoff = PliBackoff::new(
            Duration::from_secs(1),
            Duration::from_secs(10),
            5,
            2.0,
            true,
        );

        backoff.record_request();
        backoff.record_request();
        assert_eq!(backoff.request_count, 2);
        assert_eq!(backoff.current_timeout, Duration::from_secs(4));

        backoff.record_keyframe();
        assert_eq!(backoff.request_count, 0);
        assert_eq!(backoff.current_timeout, Duration::from_secs(1));
    }

    #[test]
    fn test_max_requests() {
        let mut backoff = PliBackoff::new(
            Duration::from_millis(10),
            Duration::from_secs(10),
            3,
            2.0,
            true,
        );

        sleep(Duration::from_millis(15));
        assert!(backoff.should_request());
        backoff.record_request();

        sleep(Duration::from_millis(25));
        assert!(backoff.should_request());
        backoff.record_request();

        sleep(Duration::from_millis(50));
        assert!(backoff.should_request());
        backoff.record_request();

        sleep(Duration::from_millis(100));
        assert!(
            !backoff.should_request(),
            "Should not request after max attempts"
        );
        assert!(backoff.max_requests > 0 && backoff.request_count >= backoff.max_requests);
    }

    #[test]
    fn test_linear_backoff() {
        let mut backoff = PliBackoff::new(
            Duration::from_secs(2),
            Duration::from_secs(2),
            5,
            1.0,
            false,
        );
        assert_eq!(backoff.current_timeout, Duration::from_secs(2));
        backoff.record_request();
        assert_eq!(backoff.current_timeout, Duration::from_secs(2));
        backoff.record_request();
        assert_eq!(backoff.current_timeout, Duration::from_secs(2));
    }

    #[test]
    fn test_statistics() {
        let mut backoff = PliBackoff::default();

        backoff.record_request();
        backoff.record_request();
        backoff.record_keyframe();
        backoff.record_request();
        backoff.record_keyframe();

        assert_eq!(backoff.total_requests, 3);
        assert_eq!(backoff.total_keyframes, 2);
        assert!((backoff.success_rate() - 0.666).abs() < 0.01);
    }
}
