//! Reconnect backoff policy (the engineering spec).
//!
//! A pure, deterministic exponential-backoff state machine the reporter uses to
//! decide how long to wait before re-attempting an outbound connection after a
//! disconnect. Determinism (no wall-clock, no RNG) is deliberate: it makes the
//! reconnect cadence exhaustively unit-testable.
//!
//! # Design
//! - Starts at `initial`.
//! - Each consecutive failure multiplies the delay by `factor`, capped at `max`.
//! - A successful connection resets the policy to `initial`.
//!
//! Jitter is intentionally **not** applied here. Jitter helps a *fleet* of
//! reporters avoid a thundering herd against a shared server; a single Fleet Hub
//! on localhost has exactly one reporter per agent, so the herd risk is nil and
//! determinism is worth more (testability + predictable reconnect UX). If a
//! future multi-tenant deployment needs it, jitter belongs in the I/O driver,
//! not in this policy.

use std::time::Duration;

/// Deterministic exponential-backoff policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backoff {
    initial: Duration,
    max: Duration,
    factor: u32,
    /// The delay the *next* failure will produce. Reset to `initial` on success.
    current: Duration,
    /// Number of consecutive failures since the last success/reset.
    consecutive_failures: u32,
}

impl Backoff {
    /// A policy with explicit parameters.
    ///
    /// `factor` is clamped to at least 1 (a factor of 0 would collapse the delay
    /// to zero and busy-loop). `max` is clamped to at least `initial`.
    pub fn new(initial: Duration, max: Duration, factor: u32) -> Self {
        let factor = factor.max(1);
        let max = max.max(initial);
        Backoff {
            initial,
            max,
            factor,
            current: initial,
            consecutive_failures: 0,
        }
    }

    /// The default reporter policy: 200 ms → 30 s, ×2.
    pub fn default_policy() -> Self {
        Backoff::new(Duration::from_millis(200), Duration::from_secs(30), 2)
    }

    /// The delay to wait before the next reconnect attempt.
    ///
    /// This is the delay that the *most recent* failure scheduled. It does not
    /// advance the policy; call [`Backoff::record_failure`] to advance.
    pub fn current_delay(&self) -> Duration {
        self.current
    }

    /// Number of consecutive failures since the last reset.
    pub fn failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Record a failed connection attempt and advance the backoff.
    ///
    /// Returns the delay to wait before the *next* attempt (i.e. the value
    /// [`Backoff::current_delay`] will now report). The first failure returns
    /// `initial`; subsequent failures multiply by `factor`, saturating at `max`.
    pub fn record_failure(&mut self) -> Duration {
        if self.consecutive_failures > 0 {
            // Advance: multiply, saturating at max. Compute in millis with u128
            // to avoid Duration overflow on pathological factors.
            let next_ms = (self.current.as_millis())
                .saturating_mul(self.factor as u128)
                .min(self.max.as_millis());
            self.current = Duration::from_millis(next_ms as u64);
        } else {
            // First failure since reset → the initial delay.
            self.current = self.initial.min(self.max);
        }
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.current
    }

    /// Record a successful connection: reset the policy to its initial delay.
    pub fn record_success(&mut self) {
        self.current = self.initial;
        self.consecutive_failures = 0;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff::default_policy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pol() -> Backoff {
        Backoff::new(Duration::from_millis(100), Duration::from_secs(10), 2)
    }

    #[test]
    fn starts_at_initial() {
        assert_eq!(pol().current_delay(), Duration::from_millis(100));
        assert_eq!(pol().failures(), 0);
    }

    #[test]
    fn first_failure_returns_initial() {
        let mut b = pol();
        assert_eq!(b.record_failure(), Duration::from_millis(100));
        assert_eq!(b.failures(), 1);
    }

    #[test]
    fn exponential_growth() {
        let mut b = pol();
        assert_eq!(b.record_failure(), Duration::from_millis(100)); // 1st
        assert_eq!(b.record_failure(), Duration::from_millis(200)); // ×2
        assert_eq!(b.record_failure(), Duration::from_millis(400)); // ×2
        assert_eq!(b.record_failure(), Duration::from_millis(800)); // ×2
        assert_eq!(b.failures(), 4);
    }

    #[test]
    fn saturates_at_max() {
        let mut b = pol(); // max 10s
        let mut last = Duration::ZERO;
        for _ in 0..50 {
            last = b.record_failure();
        }
        assert_eq!(last, Duration::from_secs(10), "must cap at max");
        // Further failures stay at max.
        assert_eq!(b.record_failure(), Duration::from_secs(10));
    }

    #[test]
    fn success_resets_to_initial() {
        let mut b = pol();
        b.record_failure();
        b.record_failure();
        b.record_failure();
        assert!(b.current_delay() > Duration::from_millis(100));
        b.record_success();
        assert_eq!(b.current_delay(), Duration::from_millis(100));
        assert_eq!(b.failures(), 0);
        // After reset, the cycle restarts from initial.
        assert_eq!(b.record_failure(), Duration::from_millis(100));
    }

    #[test]
    fn factor_zero_is_clamped_to_one() {
        let mut b = Backoff::new(Duration::from_millis(50), Duration::from_secs(1), 0);
        // factor clamped to 1 → no growth, never zero/busy-loop.
        assert_eq!(b.record_failure(), Duration::from_millis(50));
        assert_eq!(b.record_failure(), Duration::from_millis(50));
        assert_eq!(b.record_failure(), Duration::from_millis(50));
    }

    #[test]
    fn max_below_initial_is_clamped() {
        // max < initial → max raised to initial; delay never below initial.
        let mut b = Backoff::new(Duration::from_secs(5), Duration::from_secs(1), 2);
        assert_eq!(b.record_failure(), Duration::from_secs(5));
        assert_eq!(b.record_failure(), Duration::from_secs(5));
    }

    #[test]
    fn default_policy_parameters() {
        let b = Backoff::default_policy();
        assert_eq!(b.current_delay(), Duration::from_millis(200));
        let mut b = b;
        assert_eq!(b.record_failure(), Duration::from_millis(200));
        assert_eq!(b.record_failure(), Duration::from_millis(400));
    }

    #[test]
    fn pathological_factor_does_not_overflow() {
        let mut b = Backoff::new(Duration::from_millis(1), Duration::from_secs(60), u32::MAX);
        // Should saturate at max without panicking on overflow.
        for _ in 0..10 {
            let d = b.record_failure();
            assert!(d <= Duration::from_secs(60));
        }
        assert_eq!(b.current_delay(), Duration::from_secs(60));
    }
}
