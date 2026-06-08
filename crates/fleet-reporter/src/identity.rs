//! Reporter-side durable identity (PLAN S6 / node IDENTITY).
//!
//! This is the agent-side half of Fleet's **custom durable identity** (D4 — no
//! external broker). Each run declares a **fixed durable id** anchored on its
//! native agent id (Codex `thread.id` / Claude `session_id`, §7.5), and every
//! run delta the reporter emits is stamped `(durable_id, epoch, seq)`:
//!
//! - **`durable_id`** — the run's fixed identity. The Hub keys its persistent
//!   session on it and **reclaims** the existing entry on reconnect (no ghost
//!   duplicate). See [`crate`-external] `fleet_hub::reclaim`.
//! - **`epoch`** — distinguishes a **reconnect** (same epoch → reclaim, continue
//!   the `seq` series across the gap) from a **fresh-start** (`clean_start =
//!   true` → bump the epoch, which makes the Hub *wipe* the prior series so the
//!   new run isn't mistaken for stale duplicates).
//! - **`seq`** — a **monotonic per-run sequence number**, assigned at the moment
//!   each delta is produced. It is the backbone of the Hub's idempotent
//!   `(durable_id, seq)` apply and its ordered-replay (last-writer-by-seq) gate.
//!
//! The whole module is pure and sync — no I/O, no async — so the monotonicity
//! and reconnect-vs-fresh logic is exhaustively unit-testable.

use std::collections::HashMap;

/// A run's fixed durable identity anchored on its native agent id (D4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DurableId(pub String);

impl DurableId {
    /// Wrap a durable-id string.
    pub fn new(s: impl Into<String>) -> Self {
        DurableId(s.into())
    }
    /// Borrow the underlying id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for DurableId {
    fn from(s: &str) -> Self {
        DurableId(s.to_string())
    }
}
impl From<String> for DurableId {
    fn from(s: String) -> Self {
        DurableId(s)
    }
}

/// One run's monotonic sequence state under a fixed durable id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunIdentity {
    durable_id: DurableId,
    epoch: u64,
    /// The seq most recently *assigned* (0 ⇒ none yet).
    last_seq: u64,
}

impl RunIdentity {
    /// Begin tracking a run under `durable_id` at `epoch`, with no seq yet.
    pub fn new(durable_id: impl Into<DurableId>, epoch: u64) -> Self {
        RunIdentity {
            durable_id: durable_id.into(),
            epoch,
            last_seq: 0,
        }
    }

    /// The run's durable id.
    pub fn durable_id(&self) -> &DurableId {
        &self.durable_id
    }

    /// The current epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The most recently assigned seq (`0` if none yet).
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// Assign the next monotonic seq for this run (starts at 1, never repeats,
    /// never resets within an epoch). This is invariant 1's "monotonic per-run
    /// seq".
    pub fn next_seq(&mut self) -> u64 {
        self.last_seq += 1;
        self.last_seq
    }
}

/// A stamp attached to one outbound run delta: the triple the Hub gates on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    /// The run's fixed durable id.
    pub durable_id: DurableId,
    /// Reconnect/fresh-start epoch.
    pub epoch: u64,
    /// Monotonic per-run seq.
    pub seq: u64,
}

/// The reporter's identity ledger: one [`RunIdentity`] per durable id, plus the
/// process-wide notion of *which epoch a freshly-started run gets*.
///
/// A reporter that is *reconnecting* (the same agent run, link bounced) keeps a
/// run's epoch. A reporter that observes a *genuinely new* run — a relaunch that
/// reuses a durable id (`clean_start`) — calls [`Self::fresh_start`], which bumps
/// the epoch so the Hub wipes the prior series. Distinguishing the two is what
/// PLAN S6 means by "distinguish reconnect (reclaim, clean-start=false) from
/// fresh-start (new id, wipe)".
#[derive(Debug, Default)]
pub struct IdentityLedger {
    runs: HashMap<DurableId, RunIdentity>,
}

impl IdentityLedger {
    /// A fresh, empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of runs tracked.
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    /// Whether the ledger tracks no runs.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Whether a durable id is tracked.
    pub fn contains(&self, id: &DurableId) -> bool {
        self.runs.contains_key(id)
    }

    /// The epoch currently assigned to a durable id, if tracked.
    pub fn epoch_of(&self, id: &DurableId) -> Option<u64> {
        self.runs.get(id).map(|r| r.epoch)
    }

    /// The last seq assigned for a durable id (`0`/`None` if untracked).
    pub fn last_seq_of(&self, id: &DurableId) -> Option<u64> {
        self.runs.get(id).map(|r| r.last_seq)
    }

    /// Register / **reclaim** a run under `durable_id` for a **reconnect**
    /// (`clean_start = false`): if the run is already tracked, its epoch and seq
    /// series are preserved (the series continues across the gap); if it is new,
    /// it starts at epoch 0. Returns the run's current epoch. This never bumps an
    /// epoch — a reconnect must not be mistaken for a fresh-start.
    pub fn reclaim(&mut self, durable_id: impl Into<DurableId>) -> u64 {
        let id = durable_id.into();
        self.runs
            .entry(id.clone())
            .or_insert_with(|| RunIdentity::new(id, 0))
            .epoch
    }

    /// Register a **fresh-start** under `durable_id` (`clean_start = true`): a
    /// genuinely new run instance. Bumps the epoch (so the Hub wipes any prior
    /// series for this durable id) and resets the seq series to 0. Returns the
    /// new epoch.
    pub fn fresh_start(&mut self, durable_id: impl Into<DurableId>) -> u64 {
        let id = durable_id.into();
        match self.runs.get_mut(&id) {
            Some(r) => {
                r.epoch += 1;
                r.last_seq = 0;
                r.epoch
            }
            None => {
                // First time we've ever seen this id: epoch 0 is already "fresh".
                self.runs.insert(id.clone(), RunIdentity::new(id, 0));
                0
            }
        }
    }

    /// Declare a run with explicit `clean_start`: `true` ⇒ [`Self::fresh_start`],
    /// `false` ⇒ [`Self::reclaim`]. Returns the resulting epoch.
    pub fn declare(&mut self, durable_id: impl Into<DurableId>, clean_start: bool) -> u64 {
        let id = durable_id.into();
        if clean_start {
            self.fresh_start(id)
        } else {
            self.reclaim(id)
        }
    }

    /// Produce the next [`Stamp`] for a run, assigning a fresh monotonic seq.
    /// The run is auto-reclaimed (registered at epoch 0) if not yet tracked, so a
    /// caller can stamp without an explicit `declare` for the common reconnect
    /// path. Returns `None` only if `durable_id` is empty (an un-anchored run
    /// cannot have a durable identity — confidence/identity honesty).
    pub fn stamp(&mut self, durable_id: impl Into<DurableId>) -> Option<Stamp> {
        let id = durable_id.into();
        if id.as_str().is_empty() {
            return None;
        }
        let run = self
            .runs
            .entry(id.clone())
            .or_insert_with(|| RunIdentity::new(id.clone(), 0));
        let seq = run.next_seq();
        Some(Stamp {
            durable_id: id,
            epoch: run.epoch,
            seq,
        })
    }

    /// Forget a run entirely (e.g. its session went away). A later `declare`/
    /// `stamp` starts it over at epoch 0.
    pub fn forget(&mut self, id: &DurableId) -> bool {
        self.runs.remove(id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> DurableId {
        DurableId::new(s)
    }

    // ── monotonic per-run seq (invariant 1, reporter side) ────────────────────

    #[test]
    fn seq_starts_at_one_and_is_monotonic() {
        let mut led = IdentityLedger::new();
        assert_eq!(led.stamp("d1").unwrap().seq, 1);
        assert_eq!(led.stamp("d1").unwrap().seq, 2);
        assert_eq!(led.stamp("d1").unwrap().seq, 3);
        assert_eq!(led.last_seq_of(&id("d1")), Some(3));
    }

    #[test]
    fn seq_is_per_run_independent() {
        let mut led = IdentityLedger::new();
        assert_eq!(led.stamp("d1").unwrap().seq, 1);
        assert_eq!(
            led.stamp("d2").unwrap().seq,
            1,
            "each run has its own series"
        );
        assert_eq!(led.stamp("d1").unwrap().seq, 2);
        assert_eq!(led.stamp("d2").unwrap().seq, 2);
        assert_eq!(led.len(), 2);
    }

    #[test]
    fn empty_durable_id_has_no_stamp() {
        // An un-anchored run cannot carry a durable identity.
        let mut led = IdentityLedger::new();
        assert!(led.stamp("").is_none());
        assert!(led.is_empty());
    }

    // ── reconnect (reclaim) vs fresh-start (wipe) ─────────────────────────────

    #[test]
    fn reconnect_preserves_epoch_and_continues_seq() {
        let mut led = IdentityLedger::new();
        led.stamp("d1"); // seq 1, epoch 0
        led.stamp("d1"); // seq 2
                         // Link bounced; reconnect (clean_start = false) — series continues.
        assert_eq!(led.declare("d1", false), 0, "reconnect keeps epoch 0");
        assert_eq!(led.stamp("d1").unwrap().seq, 3, "seq continues across gap");
        assert_eq!(led.len(), 1, "no ghost run");
    }

    #[test]
    fn fresh_start_bumps_epoch_and_resets_seq() {
        let mut led = IdentityLedger::new();
        led.stamp("d1"); // seq 1
        led.stamp("d1"); // seq 2, epoch 0
                         // Agent relaunched under the same durable id: fresh-start.
        assert_eq!(led.declare("d1", true), 1, "fresh-start bumps the epoch");
        let s = led.stamp("d1").unwrap();
        assert_eq!(s.epoch, 1);
        assert_eq!(s.seq, 1, "seq series reset on fresh-start");
    }

    #[test]
    fn first_sighting_is_epoch_zero_whether_clean_or_not() {
        let mut a = IdentityLedger::new();
        let mut b = IdentityLedger::new();
        assert_eq!(a.declare("d1", false), 0);
        assert_eq!(
            b.declare("d1", true),
            0,
            "first-ever fresh-start is epoch 0"
        );
    }

    #[test]
    fn repeated_fresh_starts_keep_bumping() {
        let mut led = IdentityLedger::new();
        led.stamp("d1");
        assert_eq!(led.fresh_start("d1"), 1);
        led.stamp("d1");
        assert_eq!(led.fresh_start("d1"), 2);
        assert_eq!(led.stamp("d1").unwrap().epoch, 2);
    }

    #[test]
    fn forget_resets_to_epoch_zero() {
        let mut led = IdentityLedger::new();
        led.fresh_start("d1"); // epoch 0
        led.fresh_start("d1"); // epoch 1
        assert!(led.forget(&id("d1")));
        assert!(!led.contains(&id("d1")));
        // A new declaration starts over.
        assert_eq!(led.declare("d1", false), 0);
        assert_eq!(led.stamp("d1").unwrap().seq, 1);
    }

    #[test]
    fn stamp_auto_reclaims_untracked_run() {
        let mut led = IdentityLedger::new();
        // No explicit declare — stamping a never-seen id registers it at epoch 0.
        let s = led.stamp("never-seen").unwrap();
        assert_eq!(s.epoch, 0);
        assert_eq!(s.seq, 1);
        assert_eq!(led.epoch_of(&id("never-seen")), Some(0));
    }

    // ── property: seq is strictly monotonic per (id, epoch); fresh-start resets ──

    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum LedgerOp {
        Stamp(u8),
        Reclaim(u8),
        Fresh(u8),
    }

    fn ledger_op() -> impl Strategy<Value = LedgerOp> {
        prop_oneof![
            6 => (0u8..3).prop_map(LedgerOp::Stamp),
            2 => (0u8..3).prop_map(LedgerOp::Reclaim),
            1 => (0u8..3).prop_map(LedgerOp::Fresh),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

        /// Within any program of stamp/reclaim/fresh-start ops, each per-(id,epoch)
        /// seq series is strictly increasing from 1, a reconnect never changes the
        /// epoch, and a fresh-start always bumps it and resets the series — so the
        /// reporter can never emit two deltas with the same (durable_id, epoch, seq).
        #[test]
        fn seq_strictly_increasing_within_epoch(ops in prop::collection::vec(ledger_op(), 0..120)) {
            let mut led = IdentityLedger::new();
            // Reference: per id, current epoch + last seq seen for that epoch, and
            // the set of (epoch, seq) pairs already emitted (must never repeat).
            let mut cur_epoch: std::collections::HashMap<u8, u64> = std::collections::HashMap::new();
            let mut last_seq: std::collections::HashMap<(u8, u64), u64> = std::collections::HashMap::new();
            let mut emitted: std::collections::HashSet<(u8, u64, u64)> = std::collections::HashSet::new();

            for op in ops {
                match op {
                    LedgerOp::Reclaim(d) => {
                        let before = cur_epoch.get(&d).copied();
                        let e = led.reclaim(format!("d{d}"));
                        if let Some(b) = before {
                            prop_assert_eq!(e, b, "reconnect must not change the epoch");
                        } else {
                            prop_assert_eq!(e, 0, "first sighting reclaims at epoch 0");
                            cur_epoch.insert(d, 0);
                        }
                    }
                    LedgerOp::Fresh(d) => {
                        let before = cur_epoch.get(&d).copied();
                        let e = led.fresh_start(format!("d{d}"));
                        let expected = before.map(|b| b + 1).unwrap_or(0);
                        prop_assert_eq!(e, expected, "fresh-start bumps the epoch");
                        cur_epoch.insert(d, e);
                        // The series for the new epoch starts clean.
                        last_seq.insert((d, e), 0);
                    }
                    LedgerOp::Stamp(d) => {
                        let s = led.stamp(format!("d{d}")).unwrap();
                        let e = *cur_epoch.entry(d).or_insert(0);
                        prop_assert_eq!(s.epoch, e, "stamp uses the current epoch");
                        let prev = *last_seq.get(&(d, e)).unwrap_or(&0);
                        prop_assert_eq!(s.seq, prev + 1, "seq is strictly +1 within an epoch");
                        last_seq.insert((d, e), s.seq);
                        // The (id, epoch, seq) triple must be globally unique.
                        prop_assert!(emitted.insert((d, e, s.seq)),
                            "no two deltas may share (durable_id, epoch, seq)");
                    }
                }
            }
        }
    }
}
