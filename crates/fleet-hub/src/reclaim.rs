//! Durable-identity reclaim + per-run sequence gating.
//!
//! This is the Hub-side half of Fleet's **custom durable identity** (D4 — no
//! external broker). A reporter run declares a **fixed durable id** anchored on
//! its native agent id (Codex `thread.id` / Claude `session_id`, §7.5). The Hub
//! keeps a persistent session keyed by that id, and on a reporter reconnect it
//! **RECLAIMS** the existing entry — it never spawns a ghost duplicate — and
//! replays the buffered deltas across the gap in order.
//!
//! The three locked S6 invariants (MQTT5-derived) all live here, as a pure,
//! exhaustively-unit-testable state machine free of I/O:
//!
//! 1. **Monotonic per-run `seq`, applied idempotently by `(durable_id, seq)`.**
//!    Every reporter delta for a run carries a monotonic `seq`. The Hub records
//!    the highest `seq` it has applied per `durable_id`; a delta whose `seq` is
//!    `<=` the high-water mark is a **duplicate** and is dropped. Re-delivering
//!    the same `(durable_id, seq)` is therefore a no-op — exactly-once *effect*
//!    over an at-least-once channel.
//!
//! 2. **Ordered replay by `seq` (last-writer-by-`seq`), not by arrival.** Two
//!    deltas for the same run resolve by their `seq`, *whatever order they
//!    arrive in*. An out-of-order (stale, lower-`seq`) delta that arrives after a
//!    newer one is **rejected** so it can never regress the run's state. The Hub
//!    thus converges to the last-writer-by-`seq` regardless of network reorder.
//!
//! 3. **Expiry GC drops the state entry AND its buffered-delta queue
//!    ATOMICALLY.** When a session is swept (S6/S7 session-expiry GC) the
//!    reclaim bookkeeping for *every run under it* — the per-`durable_id`
//!    high-water marks, i.e. the "buffered-delta queue" dedup state — is dropped
//!    in the same operation. There is never a window in which the session is gone
//!    but its seq state lingers (which would wrongly reject a later fresh
//!    delta), nor one in which the seq state is gone but the session remains.
//!
//! ## Reconnect (reclaim) vs fresh-start (wipe)
//!
//! A run also declares an **epoch**. A *reconnect* keeps the same epoch: the Hub
//! reclaims the existing entry and continues the monotonic `seq` series across
//! the gap. A *fresh-start* (a genuinely new run that happens to reuse a durable
//! id — e.g. the agent relaunched and chose a new identity, `clean-start=true`)
//! bumps the epoch, which **wipes** the prior high-water mark so the new series
//! starts clean and is not mistaken for a stale duplicate. This is the Hub-side
//! mirror of [`crate`-external] `fleet_reporter::identity`.

use std::collections::HashMap;

/// A run's durable identity — a fixed id anchored on its native agent id (D4).
///
/// We keep it a newtype over `String` rather than a bare `String` so the
/// reclaim table's key is type-distinct from a `run_id`/`session_id` and can
/// never be mixed up at a call site.
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

/// The verdict for one inbound `(durable_id, epoch, seq)` delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// First time we've seen this durable id (or a bumped epoch wiped the prior
    /// state): apply and record the new high-water mark. This is a **reclaim**
    /// when an entry already existed under a *lower* epoch, otherwise a fresh
    /// registration — either way the delta is authoritative and applied.
    ApplyFresh,
    /// `seq` advances the existing series: apply and bump the high-water mark.
    Apply,
    /// `seq <= high-water` within the same epoch: a duplicate redelivery —
    /// **drop** (idempotency, invariant 1).
    DuplicateDrop,
    /// A *lower* epoch than the current one: a stale delta from a superseded
    /// run instance — **drop** (a fresh-start already wiped/replaced it).
    StaleEpochDrop,
}

impl Decision {
    /// Whether this decision means the delta should be applied to Hub state.
    pub fn applies(self) -> bool {
        matches!(self, Decision::ApplyFresh | Decision::Apply)
    }
    /// Whether this decision means the delta is dropped (idempotent / stale).
    pub fn drops(self) -> bool {
        !self.applies()
    }
}

/// Per-durable-id reclaim bookkeeping: the current epoch and the highest `seq`
/// applied within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mark {
    epoch: u64,
    high_seq: u64,
}

/// The Hub's reclaim table: the dedup / ordered-replay state for every live
/// durable id. This is the "buffered-delta queue" the invariant-3 atomic drop
/// refers to — it is the only state that must vanish with a session entry.
#[derive(Debug, Default, Clone)]
pub struct ReclaimTable {
    marks: HashMap<DurableId, Mark>,
}

impl ReclaimTable {
    /// A fresh, empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of durable ids currently tracked.
    pub fn len(&self) -> usize {
        self.marks.len()
    }

    /// Whether the table tracks no ids.
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }

    /// Whether a durable id is currently tracked.
    pub fn contains(&self, id: &DurableId) -> bool {
        self.marks.contains_key(id)
    }

    /// The current high-water `seq` for a durable id (within its current epoch),
    /// or `None` if untracked.
    pub fn high_seq(&self, id: &DurableId) -> Option<u64> {
        self.marks.get(id).map(|m| m.high_seq)
    }

    /// The current epoch for a durable id, or `None` if untracked.
    pub fn epoch(&self, id: &DurableId) -> Option<u64> {
        self.marks.get(id).map(|m| m.epoch)
    }

    /// Classify an inbound `(durable_id, epoch, seq)` delta **without** mutating
    /// the table. Pure — call [`Self::commit`] after a successful apply to record
    /// the new high-water mark, or use [`Self::admit`] to do both atomically.
    pub fn classify(&self, id: &DurableId, epoch: u64, seq: u64) -> Decision {
        match self.marks.get(id) {
            None => Decision::ApplyFresh,
            Some(m) if epoch > m.epoch => Decision::ApplyFresh, // fresh-start wipes prior series
            Some(m) if epoch < m.epoch => Decision::StaleEpochDrop,
            // same epoch:
            Some(m) if seq > m.high_seq => Decision::Apply,
            Some(_) => Decision::DuplicateDrop, // seq <= high_seq
        }
    }

    /// Record a successful apply: set the high-water mark for `(id, epoch)`.
    /// Idempotent for a duplicate (never lowers a high-water mark; never moves an
    /// epoch backward).
    fn commit(&mut self, id: &DurableId, epoch: u64, seq: u64) {
        self.marks
            .entry(id.clone())
            .and_modify(|m| {
                if epoch > m.epoch {
                    // Fresh-start: adopt the new epoch and reset the series to
                    // exactly this seq (the wipe — invariant for reconnect-vs-fresh).
                    m.epoch = epoch;
                    m.high_seq = seq;
                } else if epoch == m.epoch && seq > m.high_seq {
                    m.high_seq = seq;
                }
                // epoch < m.epoch or seq <= high_seq: ignore (never regress).
            })
            .or_insert(Mark {
                epoch,
                high_seq: seq,
            });
    }

    /// Classify **and**, if the delta is to be applied, atomically commit the new
    /// high-water mark. Returns the [`Decision`] so the caller can apply the
    /// delta to Hub state only when [`Decision::applies`].
    ///
    /// This is the single entry point reporters' deltas flow through: it is the
    /// idempotent (invariant 1) and ordered-replay / last-writer-by-seq
    /// (invariant 2) gate in one call.
    pub fn admit(&mut self, id: &DurableId, epoch: u64, seq: u64) -> Decision {
        let decision = self.classify(id, epoch, seq);
        if decision.applies() {
            self.commit(id, epoch, seq);
        }
        decision
    }

    /// Drop the reclaim bookkeeping for a single durable id. Used by run-level
    /// GC; the entry vanishes so a *later* genuinely-fresh delta for the same id
    /// is admitted from scratch (not wrongly rejected as a duplicate).
    pub fn drop_id(&mut self, id: &DurableId) -> bool {
        self.marks.remove(id).is_some()
    }

    /// Atomically drop the reclaim bookkeeping for **every** durable id in
    /// `ids`. This is the call the session-expiry GC makes so a swept session's
    /// state entry and its buffered-delta (dedup) queue disappear together —
    /// **invariant 3**. Returns how many marks were dropped.
    pub fn drop_ids<'a>(&mut self, ids: impl IntoIterator<Item = &'a DurableId>) -> usize {
        let mut dropped = 0;
        for id in ids {
            if self.marks.remove(id).is_some() {
                dropped += 1;
            }
        }
        dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> DurableId {
        DurableId::new(s)
    }

    // ── invariant 1: idempotent apply by (durable_id, seq) ────────────────────

    #[test]
    fn first_delta_is_apply_fresh() {
        let t = ReclaimTable::new();
        assert_eq!(t.classify(&id("d1"), 0, 1), Decision::ApplyFresh);
    }

    #[test]
    fn monotonic_seq_applies_in_order() {
        let mut t = ReclaimTable::new();
        assert_eq!(t.admit(&id("d1"), 0, 1), Decision::ApplyFresh);
        assert_eq!(t.admit(&id("d1"), 0, 2), Decision::Apply);
        assert_eq!(t.admit(&id("d1"), 0, 3), Decision::Apply);
        assert_eq!(t.high_seq(&id("d1")), Some(3));
    }

    #[test]
    fn exact_duplicate_seq_is_dropped() {
        let mut t = ReclaimTable::new();
        t.admit(&id("d1"), 0, 5);
        // Re-deliver the SAME (durable_id, seq): idempotent no-op.
        assert_eq!(t.admit(&id("d1"), 0, 5), Decision::DuplicateDrop);
        assert_eq!(
            t.high_seq(&id("d1")),
            Some(5),
            "duplicate does not move HWM"
        );
    }

    #[test]
    fn lower_seq_after_higher_is_dropped() {
        let mut t = ReclaimTable::new();
        t.admit(&id("d1"), 0, 1);
        t.admit(&id("d1"), 0, 10);
        // A stale/out-of-order seq that arrives late must not regress.
        assert_eq!(t.admit(&id("d1"), 0, 4), Decision::DuplicateDrop);
        assert_eq!(t.high_seq(&id("d1")), Some(10));
    }

    #[test]
    fn redelivering_a_whole_prefix_is_a_noop() {
        // Simulate an at-least-once channel redelivering an already-applied run.
        let mut t = ReclaimTable::new();
        for s in 1..=5 {
            t.admit(&id("d1"), 0, s);
        }
        // The reporter reconnects and replays 1..=5 again (it doesn't know which
        // landed). Every one is a duplicate; the HWM is unchanged.
        for s in 1..=5 {
            assert!(t.admit(&id("d1"), 0, s).drops(), "seq {s} must be a dup");
        }
        assert_eq!(t.high_seq(&id("d1")), Some(5));
    }

    // ── invariant 2: ordered replay by seq (last-writer-by-seq) ───────────────

    #[test]
    fn out_of_order_arrival_resolves_by_seq_not_arrival() {
        let mut t = ReclaimTable::new();
        // The newer delta (seq 7) arrives BEFORE the older (seq 3).
        assert_eq!(t.admit(&id("d1"), 0, 7), Decision::ApplyFresh);
        // The older one arriving later must be rejected — last-writer-by-seq.
        assert_eq!(t.admit(&id("d1"), 0, 3), Decision::DuplicateDrop);
        assert_eq!(t.high_seq(&id("d1")), Some(7), "winner is the highest seq");
    }

    // ── reconnect (reclaim) vs fresh-start (wipe) ─────────────────────────────

    #[test]
    fn reconnect_same_epoch_continues_series_and_reclaims() {
        let mut t = ReclaimTable::new();
        t.admit(&id("d1"), 0, 3);
        // Reporter bounced; reconnects with the SAME epoch and keeps numbering.
        assert_eq!(t.admit(&id("d1"), 0, 4), Decision::Apply);
        assert_eq!(t.epoch(&id("d1")), Some(0), "reconnect keeps the epoch");
        assert_eq!(t.len(), 1, "no ghost: still exactly one durable id");
    }

    #[test]
    fn fresh_start_bumps_epoch_and_wipes_series() {
        let mut t = ReclaimTable::new();
        t.admit(&id("d1"), 0, 9);
        // A genuine fresh-start (clean-start=true) bumps the epoch. Even though
        // its seq (1) is BELOW the old HWM (9), it must apply — the wipe.
        assert_eq!(t.admit(&id("d1"), 1, 1), Decision::ApplyFresh);
        assert_eq!(t.epoch(&id("d1")), Some(1));
        assert_eq!(
            t.high_seq(&id("d1")),
            Some(1),
            "epoch bump reset the series"
        );
    }

    #[test]
    fn stale_epoch_delta_after_fresh_start_is_dropped() {
        let mut t = ReclaimTable::new();
        t.admit(&id("d1"), 0, 9);
        t.admit(&id("d1"), 1, 2); // fresh-start
                                  // A straggler from the OLD instance (epoch 0) arrives late — drop it.
        assert_eq!(t.admit(&id("d1"), 0, 10), Decision::StaleEpochDrop);
        assert_eq!(t.epoch(&id("d1")), Some(1), "stale epoch never wins");
        assert_eq!(t.high_seq(&id("d1")), Some(2));
    }

    #[test]
    fn no_ghost_under_reconnect_storm() {
        // Many reconnects (same epoch) + one fresh-start must leave exactly ONE
        // tracked id — never a duplicate "ghost".
        let mut t = ReclaimTable::new();
        for s in 1..=20 {
            t.admit(&id("d1"), 0, s);
        }
        t.admit(&id("d1"), 1, 1); // fresh-start
        for s in 2..=10 {
            t.admit(&id("d1"), 1, s);
        }
        assert_eq!(t.len(), 1, "no ghost duplicate entry");
        assert_eq!(t.epoch(&id("d1")), Some(1));
        assert_eq!(t.high_seq(&id("d1")), Some(10));
    }

    // ── invariant 3: atomic drop of entry + dedup queue ───────────────────────

    #[test]
    fn drop_id_lets_later_fresh_delta_through() {
        let mut t = ReclaimTable::new();
        t.admit(&id("d1"), 0, 5);
        assert!(t.drop_id(&id("d1")));
        assert!(!t.contains(&id("d1")));
        // After the drop, even seq 1 (below the old HWM) is admitted fresh.
        assert_eq!(t.admit(&id("d1"), 0, 1), Decision::ApplyFresh);
    }

    #[test]
    fn drop_ids_drops_a_whole_session_atomically() {
        let mut t = ReclaimTable::new();
        t.admit(&id("d1"), 0, 1);
        t.admit(&id("d2"), 0, 1);
        t.admit(&id("other"), 0, 1);
        // Sweep a session whose runs are d1 + d2 — both vanish together; an
        // unrelated id is untouched.
        let ids = [id("d1"), id("d2")];
        let dropped = t.drop_ids(ids.iter());
        assert_eq!(dropped, 2);
        assert!(!t.contains(&id("d1")));
        assert!(!t.contains(&id("d2")));
        assert!(t.contains(&id("other")), "unrelated id survives");
    }

    #[test]
    fn drop_ids_is_idempotent_for_absent_ids() {
        let mut t = ReclaimTable::new();
        t.admit(&id("d1"), 0, 1);
        let ids = [id("d1"), id("ghost")];
        assert_eq!(t.drop_ids(ids.iter()), 1, "only the present id counts");
        assert!(t.is_empty());
    }

    // ── classify is pure (no mutation) ────────────────────────────────────────

    #[test]
    fn classify_does_not_mutate() {
        let mut t = ReclaimTable::new();
        t.admit(&id("d1"), 0, 3);
        let _ = t.classify(&id("d1"), 0, 9);
        assert_eq!(t.high_seq(&id("d1")), Some(3), "classify must not commit");
    }
}
