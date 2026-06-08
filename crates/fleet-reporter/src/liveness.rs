//! Liveness / "dead-decision" state machine (PLAN S5).
//!
//! A core REPCORE invariant (PLAN S5): a run is declared **`dead` only on a
//! confirmed exit or a heartbeat timeout** — *never* merely because the Hub
//! connection dropped. When the transport to the Hub goes down, the reporter is
//! still observing its agent; it **reconciles on reconnect** rather than
//! prematurely reporting the run dead. Conversely, when the *agent process*
//! exits (a confirmed signal) the run is dead immediately, regardless of
//! connection state.
//!
//! This module is the pure decision core. It takes liveness *events* and a
//! monotonic logical clock (caller-supplied — no wall-clock, for testability)
//! and answers one question: **is the agent run confirmed dead?**
//!
//! # The two — and only two — ways a run dies (PLAN S5 / §7.3)
//! 1. **Confirmed exit** — an authoritative signal that the agent process ended
//!    (a hook `SessionEnd`, an observed process exit). Immediate.
//! 2. **Heartbeat timeout** — no agent-liveness signal for longer than the
//!    timeout grace. This catches a reporter whose agent vanished without a
//!    clean exit (crash, SIGKILL).
//!
//! A dropped **Hub connection is neither** of these — it pauses reporting, it
//! does not kill the run.

use std::time::Duration;

/// Why a run is (or is not yet) considered dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// The agent is alive: a recent liveness signal, or within the grace window.
    Alive,
    /// The agent process exited cleanly/authoritatively (confirmed exit).
    DeadConfirmedExit,
    /// No liveness signal within the timeout grace — presumed dead.
    DeadTimeout,
}

impl Liveness {
    /// Whether this liveness verdict means the run is dead.
    pub fn is_dead(self) -> bool {
        matches!(self, Liveness::DeadConfirmedExit | Liveness::DeadTimeout)
    }
}

/// Tracks agent liveness against a heartbeat timeout. Time is a caller-supplied
/// monotonic [`Duration`] since some fixed start, so the whole machine is
/// deterministic and clock-free for tests.
#[derive(Debug, Clone)]
pub struct LivenessTracker {
    /// Max gap between liveness signals before a timeout is declared.
    timeout: Duration,
    /// Logical time of the last observed liveness signal.
    last_seen: Duration,
    /// Set once an authoritative exit is observed — sticky, never un-set.
    confirmed_exit: bool,
}

impl LivenessTracker {
    /// A tracker with the given heartbeat timeout, seeded "alive at t=0".
    pub fn new(timeout: Duration) -> Self {
        LivenessTracker {
            timeout,
            last_seen: Duration::ZERO,
            confirmed_exit: false,
        }
    }

    /// Record an agent-liveness signal (a heartbeat, a hook, observed output) at
    /// logical time `now`. Refreshes the timeout window.
    ///
    /// A liveness signal does **not** clear a prior confirmed exit — once an
    /// agent is confirmed exited it stays dead (a stray late signal cannot
    /// resurrect it; that would be a different run).
    pub fn observe_liveness(&mut self, now: Duration) {
        if now >= self.last_seen {
            self.last_seen = now;
        }
    }

    /// Record an authoritative agent-process exit. Sticky and immediate.
    pub fn observe_exit(&mut self) {
        self.confirmed_exit = true;
    }

    /// Evaluate liveness at logical time `now`.
    ///
    /// Confirmed exit wins immediately. Otherwise, a gap since `last_seen`
    /// strictly greater than `timeout` is a timeout death; anything within the
    /// grace is alive.
    pub fn evaluate(&self, now: Duration) -> Liveness {
        if self.confirmed_exit {
            return Liveness::DeadConfirmedExit;
        }
        let elapsed = now.saturating_sub(self.last_seen);
        if elapsed > self.timeout {
            Liveness::DeadTimeout
        } else {
            Liveness::Alive
        }
    }

    /// Convenience: is the run dead at logical time `now`?
    pub fn is_dead(&self, now: Duration) -> bool {
        self.evaluate(now).is_dead()
    }

    /// Logical time of the last observed liveness signal.
    pub fn last_seen(&self) -> Duration {
        self.last_seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(30);

    fn tracker() -> LivenessTracker {
        LivenessTracker::new(TIMEOUT)
    }

    #[test]
    fn fresh_tracker_is_alive_within_grace() {
        let t = tracker();
        assert_eq!(t.evaluate(Duration::from_secs(0)), Liveness::Alive);
        assert_eq!(t.evaluate(Duration::from_secs(29)), Liveness::Alive);
        assert_eq!(
            t.evaluate(TIMEOUT),
            Liveness::Alive,
            "exactly at timeout = still alive"
        );
    }

    #[test]
    fn timeout_declares_dead_only_past_grace() {
        let t = tracker();
        assert_eq!(t.evaluate(Duration::from_secs(31)), Liveness::DeadTimeout);
        assert!(t.is_dead(Duration::from_secs(31)));
    }

    #[test]
    fn liveness_signal_refreshes_window() {
        let mut t = tracker();
        // At t=20 we get a heartbeat → window resets.
        t.observe_liveness(Duration::from_secs(20));
        // t=49 is only 29s after last_seen → still alive.
        assert_eq!(t.evaluate(Duration::from_secs(49)), Liveness::Alive);
        // t=51 is 31s after → timeout.
        assert_eq!(t.evaluate(Duration::from_secs(51)), Liveness::DeadTimeout);
    }

    #[test]
    fn confirmed_exit_is_immediate_and_sticky() {
        let mut t = tracker();
        t.observe_exit();
        assert_eq!(
            t.evaluate(Duration::from_secs(0)),
            Liveness::DeadConfirmedExit
        );
        // Even a later liveness signal cannot resurrect a confirmed-exited run.
        t.observe_liveness(Duration::from_secs(5));
        assert_eq!(
            t.evaluate(Duration::from_secs(5)),
            Liveness::DeadConfirmedExit
        );
    }

    #[test]
    fn confirmed_exit_takes_priority_over_alive_window() {
        let mut t = tracker();
        t.observe_liveness(Duration::from_secs(10));
        t.observe_exit();
        // Within the alive window, but exit wins.
        assert_eq!(
            t.evaluate(Duration::from_secs(11)),
            Liveness::DeadConfirmedExit
        );
    }

    #[test]
    fn dropped_connection_is_not_a_death() {
        // This is the central invariant: liveness is independent of the Hub
        // connection. The tracker is only fed *agent* liveness, never Hub
        // connection state. As long as the agent keeps signaling, it is alive
        // even through arbitrarily many Hub disconnects.
        let mut t = tracker();
        for sec in (0..300).step_by(10) {
            // Agent keeps heartbeating every 10s; Hub may be down the whole time.
            t.observe_liveness(Duration::from_secs(sec));
            assert_eq!(
                t.evaluate(Duration::from_secs(sec)),
                Liveness::Alive,
                "agent that keeps signaling is never dead, regardless of Hub link"
            );
        }
    }

    #[test]
    fn out_of_order_liveness_does_not_regress_last_seen() {
        let mut t = tracker();
        t.observe_liveness(Duration::from_secs(50));
        // A late/stale signal timestamped earlier must not move last_seen back.
        t.observe_liveness(Duration::from_secs(10));
        assert_eq!(t.last_seen(), Duration::from_secs(50));
        assert_eq!(t.evaluate(Duration::from_secs(79)), Liveness::Alive);
    }

    #[test]
    fn is_dead_helper_matches_evaluate() {
        let mut t = tracker();
        assert!(!t.is_dead(Duration::from_secs(10)));
        t.observe_exit();
        assert!(t.is_dead(Duration::from_secs(10)));
    }

    #[test]
    fn liveness_is_dead_classifier() {
        assert!(!Liveness::Alive.is_dead());
        assert!(Liveness::DeadConfirmedExit.is_dead());
        assert!(Liveness::DeadTimeout.is_dead());
    }
}
