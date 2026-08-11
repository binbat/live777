//! AIMD adaptive-bitrate controller (issue #409).
//!
//! Closes the loop between WHEP subscriber RTCP feedback and the native
//! encoder's runtime bitrate: subscriber loss (TWCC, falling back to
//! Receiver Report `fraction_lost`) is sampled once per second from
//! [`crate::forward::subscribe_quality`], and an AIMD policy retunes the
//! encoder through [`StreamSource::set_bitrate`].
//!
//! Subscriber-count semantics (one encoder is shared by every subscriber
//! of the stream):
//!
//! * **one subscriber** — the loop tracks that subscriber directly; its
//!   loss signal drives the encoder;
//! * **N subscribers** — worst-subscriber policy: the stream runs at the
//!   bitrate the *weakest* link can sustain.  A decrease requires two
//!   consecutive congested windows (debounce against retransmit-repairable
//!   bursts); recovery requires *every* eligible subscriber to stay clean
//!   for the full recovery window.  Lowering the shared encoder for one
//!   bad link penalizes the healthy subscribers too — that tradeoff is
//!   inherent to a single-encode SFU; simulcast would be the real answer;
//! * **zero eligible subscribers** — hold: no signal, no decision, and the
//!   debounce/recovery clocks reset so a (re)joining subscriber does not
//!   inherit stale state.

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

use super::StreamSource;
use crate::forward::PeerForward;
use crate::forward::bridge::SourceBridge;

/// Controller tick — one quality sample + decision per interval.
const TICK: Duration = Duration::from_secs(1);
/// Loss fraction above which a window counts as congested.
const LOSS_HIGH: f32 = 0.05;
/// Multiplicative decrease factor on confirmed congestion.
const DECREASE: f32 = 0.85;
/// Additive increase step per recovery window (fraction of the target).
const INCREASE_STEP: f32 = 0.05;
/// Sustained clean time before one additive-increase step.
const RECOVER_AFTER: Duration = Duration::from_secs(10);
/// Consecutive congested windows required before decreasing.
const DECREASE_WINDOWS: u32 = 2;
/// Minimum spacing between encoder retunes.
const MIN_CHANGE_INTERVAL: Duration = Duration::from_secs(2);
/// Subscribers younger than this are excluded — startup NACK/PLI bursts
/// are normal while a decoder primes.
const GRACE: Duration = Duration::from_secs(5);

/// Adaptive-bitrate bounds for one source.  `target` is the configured
/// encoder bitrate and acts as the ceiling; the controller only lowers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdaptiveBitrateConfig {
    pub target: u32,
    pub min: u32,
}

impl AdaptiveBitrateConfig {
    /// `min` defaults to `max(target / 8, 300 kbps)` and never exceeds the
    /// target.
    pub fn new(target: u32, min: Option<u32>) -> Self {
        let min = min.unwrap_or_else(|| (target / 8).max(300_000)).min(target);
        Self { target, min }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Hold,
    Set(u32),
}

/// Pure AIMD state machine — no I/O, unit-testable.  One instance per
/// stream (per source-bridge lifetime).
struct Aimd {
    cfg: AdaptiveBitrateConfig,
    current: u32,
    congested_windows: u32,
    clean_since: Option<Instant>,
    last_change: Option<Instant>,
}

impl Aimd {
    fn new(cfg: AdaptiveBitrateConfig) -> Self {
        Self {
            cfg,
            current: cfg.target,
            congested_windows: 0,
            clean_since: None,
            last_change: None,
        }
    }

    fn can_change(&self, now: Instant) -> bool {
        self.last_change
            .is_none_or(|t| now.duration_since(t) >= MIN_CHANGE_INTERVAL)
    }

    /// `worst_loss`: the highest loss fraction among eligible subscribers,
    /// or `None` when no subscriber is eligible this window.
    fn on_window(&mut self, worst_loss: Option<f32>, now: Instant) -> Decision {
        let Some(loss) = worst_loss else {
            self.congested_windows = 0;
            self.clean_since = None;
            return Decision::Hold;
        };

        if loss > LOSS_HIGH {
            self.clean_since = None;
            self.congested_windows += 1;
            if self.congested_windows >= DECREASE_WINDOWS && self.can_change(now) {
                let next = ((self.current as f32 * DECREASE) as u32).max(self.cfg.min);
                if next < self.current {
                    self.current = next;
                    self.last_change = Some(now);
                    self.congested_windows = 0;
                    return Decision::Set(next);
                }
            }
            return Decision::Hold;
        }

        self.congested_windows = 0;
        let clean_since = *self.clean_since.get_or_insert(now);
        if now.duration_since(clean_since) >= RECOVER_AFTER
            && self.current < self.cfg.target
            && self.can_change(now)
        {
            let step = ((self.cfg.target as f32 * INCREASE_STEP) as u32).max(1);
            let next = (self.current + step).min(self.cfg.target);
            self.current = next;
            self.last_change = Some(now);
            // Each step needs its own full clean window.
            self.clean_since = Some(now);
            return Decision::Set(next);
        }
        Decision::Hold
    }
}

/// Spawn the per-stream controller task.  Its lifetime follows the source
/// bridge: once the bridge is dropped (source stop / stream teardown), the
/// task exits on its next tick.
#[cfg(feature = "source")]
pub(crate) fn spawn(
    stream_id: String,
    forward: PeerForward,
    source: Arc<tokio::sync::Mutex<Box<dyn StreamSource>>>,
    bridge: Weak<tokio::sync::Mutex<SourceBridge>>,
    config: AdaptiveBitrateConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut aimd = Aimd::new(config);
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        info!(
            "[{}] adaptive bitrate: target={} min={}",
            stream_id, config.target, config.min
        );
        loop {
            ticker.tick().await;
            if bridge.upgrade().is_none() {
                info!(
                    "[{}] adaptive bitrate: bridge gone, controller exit",
                    stream_id
                );
                return;
            }

            let mut worst: Option<f32> = None;
            let mut subs = 0usize;
            for stats in forward.internal.subscribe_quality() {
                let q = stats.sample();
                subs += 1;
                debug!(
                    "[{}] adaptive bitrate: sub age={:.0}s loss={:.3} nack/s={:.0} pli={} twcc={}",
                    stream_id,
                    q.age.as_secs_f64(),
                    q.loss,
                    q.nack_per_sec,
                    q.pli_fir,
                    q.twcc
                );
                if q.age >= GRACE {
                    worst = Some(worst.map_or(q.loss, |w: f32| w.max(q.loss)));
                }
            }

            if let Decision::Set(bps) = aimd.on_window(worst, Instant::now()) {
                let applied = source.lock().await.set_bitrate(bps).await;
                if applied {
                    info!(
                        "[{}] adaptive bitrate: -> {} bps (worst loss {:.1}%, subs={})",
                        stream_id,
                        bps,
                        worst.unwrap_or(0.0) * 100.0,
                        subs
                    );
                } else {
                    // Fixed-bitrate backend — stop deciding entirely.
                    warn!(
                        "[{}] adaptive bitrate: encoder rejected {} bps, controller exit",
                        stream_id, bps
                    );
                    return;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AdaptiveBitrateConfig {
        AdaptiveBitrateConfig::new(2_000_000, None)
    }

    #[test]
    fn default_min_is_eighth_of_target() {
        let c = cfg();
        assert_eq!(c.min, 300_000); // 2_000_000 / 8 = 250_000 < 300k floor
        assert_eq!(AdaptiveBitrateConfig::new(8_000_000, None).min, 1_000_000);
        // Explicit min wins, clamped to target.
        assert_eq!(
            AdaptiveBitrateConfig::new(2_000_000, Some(500_000)).min,
            500_000
        );
        assert_eq!(
            AdaptiveBitrateConfig::new(2_000_000, Some(9_000_000)).min,
            2_000_000
        );
    }

    #[test]
    fn single_bad_window_does_not_decrease() {
        let mut a = Aimd::new(cfg());
        let now = Instant::now();
        assert_eq!(a.on_window(Some(0.5), now), Decision::Hold);
    }

    #[test]
    fn two_bad_windows_decrease_once() {
        let mut a = Aimd::new(cfg());
        let now = Instant::now();
        assert_eq!(a.on_window(Some(0.5), now), Decision::Hold);
        assert_eq!(
            a.on_window(Some(0.5), now + Duration::from_secs(1)),
            Decision::Set(1_700_000)
        );
        // Debounce restarted: the next decrease needs two more bad windows.
        assert_eq!(
            a.on_window(Some(0.5), now + Duration::from_secs(2)),
            Decision::Hold
        );
    }

    #[test]
    fn decrease_clamps_at_min() {
        let mut a = Aimd::new(cfg());
        a.current = 320_000;
        let now = Instant::now();
        a.on_window(Some(0.9), now);
        let d = a.on_window(Some(0.9), now + Duration::from_secs(1));
        assert_eq!(d, Decision::Set(300_000));
        // At the floor: further congestion holds.
        a.on_window(Some(0.9), now + Duration::from_secs(2));
        let d = a.on_window(Some(0.9), now + Duration::from_secs(3));
        assert_eq!(d, Decision::Hold);
    }

    #[test]
    fn recovery_needs_sustained_clean_window() {
        let mut a = Aimd::new(cfg());
        a.current = 1_000_000;
        let now = Instant::now();
        // First clean window only starts the clock.
        assert_eq!(a.on_window(Some(0.01), now), Decision::Hold);
        // A congested blip restarts the clean clock.
        a.on_window(Some(0.5), now + Duration::from_secs(9));
        assert_eq!(
            a.on_window(Some(0.01), now + Duration::from_secs(10)),
            Decision::Hold
        );
        // 10 s of sustained clean: +5% of target = +100k.
        assert_eq!(
            a.on_window(Some(0.01), now + Duration::from_secs(20)),
            Decision::Set(1_100_000)
        );
    }

    #[test]
    fn recovery_caps_at_target() {
        let mut a = Aimd::new(cfg());
        a.current = 1_999_000;
        let now = Instant::now();
        a.on_window(Some(0.0), now);
        assert_eq!(
            a.on_window(Some(0.0), now + RECOVER_AFTER),
            Decision::Set(2_000_000)
        );
        // At target: no further increase.
        assert_eq!(
            a.on_window(Some(0.0), now + RECOVER_AFTER * 2),
            Decision::Hold
        );
    }

    #[test]
    fn no_subscriber_holds_and_resets_clocks() {
        let mut a = Aimd::new(cfg());
        a.current = 1_000_000;
        let now = Instant::now();
        // Build up a clean clock, then lose all subscribers: the clock must
        // reset so rejoining does not trigger an immediate increase.
        a.on_window(Some(0.0), now);
        assert_eq!(
            a.on_window(None, now + Duration::from_secs(30)),
            Decision::Hold
        );
        assert_eq!(
            a.on_window(Some(0.0), now + Duration::from_secs(31)),
            Decision::Hold
        );
    }

    #[test]
    fn min_change_interval_rate_limits() {
        let mut a = Aimd::new(cfg());
        let now = Instant::now();
        a.on_window(Some(0.5), now);
        assert_eq!(
            a.on_window(Some(0.5), now + Duration::from_secs(1)),
            Decision::Set(1_700_000)
        );
        // Two more bad windows arrive, but only 2 s (not >= 2 s after the
        // last change at the second window) — debounce and rate limit stack.
        a.on_window(Some(0.5), now + Duration::from_secs(2));
        assert_eq!(
            a.on_window(Some(0.5), now + Duration::from_secs(3)),
            Decision::Set((1_700_000.0 * 0.85) as u32)
        );
    }
}
