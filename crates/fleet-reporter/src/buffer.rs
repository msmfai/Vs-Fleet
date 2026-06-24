//! Ordered outbound delta buffer (the engineering spec).
//!
//! The reporter never blocks the agent it observes: every state change becomes a
//! [`Delta`] that is *enqueued* immediately. When the Hub connection is up the
//! driver drains the buffer in order; when it is down, deltas accumulate here and
//! are flushed — **in the same order they were produced** — the instant the
//! connection is re-established (the engineering spec "BUFFERS deltas while disconnected, …
//! flush-on-reconnect (ordered)").
//!
//! # Sequencing
//! Every delta is stamped with a monotonically increasing `seq` at enqueue time.
//! `seq` is the backbone of the S6 durable-identity invariants (idempotent apply
//! by `(durable_id, seq)`, ordered replay by `seq`); S5 establishes the
//! monotonic stamping + ordered FIFO drain that S6 builds on. The buffer here is
//! deliberately FIFO and never reorders — ordering by `seq` and ordering by
//! arrival are identical because `seq` is assigned at enqueue.
//!
//! # Capacity
//! The buffer is bounded. A pathological, permanently-disconnected reporter must
//! not grow memory without limit. On overflow we drop the **oldest** delta (the
//! Hub will reconcile from the newest full-object upsert anyway — §7.4 deltas are
//! whole objects), and record that a drop happened so the driver can log it.

use std::collections::VecDeque;

use fleet_hub::wire::ClientMessage;

/// A buffered outbound delta: a Hub-bound [`ClientMessage`] plus its monotonic
/// sequence number.
#[derive(Debug, Clone, PartialEq)]
pub struct Delta {
    /// Monotonic per-reporter sequence number, assigned at enqueue.
    pub seq: u64,
    /// The wire message to send to the Hub.
    pub msg: ClientMessage,
}

impl Delta {
    /// The wire `type` discriminator of the underlying message.
    pub fn type_name(&self) -> &'static str {
        self.msg.type_name()
    }
}

/// A bounded, FIFO, monotonically-sequenced outbound delta buffer.
#[derive(Debug)]
pub struct DeltaBuffer {
    queue: VecDeque<Delta>,
    next_seq: u64,
    capacity: usize,
    dropped: u64,
}

impl DeltaBuffer {
    /// Default buffer capacity. Generous — a reporter producing one delta per
    /// agent state change will not hit this in any realistic disconnect window.
    pub const DEFAULT_CAPACITY: usize = 4096;

    /// A buffer with the default capacity.
    pub fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }

    /// A buffer with an explicit capacity (clamped to at least 1).
    pub fn with_capacity(capacity: usize) -> Self {
        DeltaBuffer {
            queue: VecDeque::new(),
            next_seq: 1,
            capacity: capacity.max(1),
            dropped: 0,
        }
    }

    /// Enqueue a message, stamping it with the next monotonic `seq`.
    ///
    /// Returns the assigned `seq`. If the buffer is at capacity, the **oldest**
    /// queued delta is dropped first (and counted in [`DeltaBuffer::dropped`]).
    pub fn push(&mut self, msg: ClientMessage) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.queue.len() >= self.capacity {
            self.queue.pop_front();
            self.dropped += 1;
        }
        self.queue.push_back(Delta { seq, msg });
        seq
    }

    /// Re-insert a delta that already has a `seq`, **preserving** that seq (does
    /// not advance `next_seq`). Used by the reconnect path to put an un-flushed
    /// delta back without re-numbering it, so `(durable_id, seq)` stays stable.
    ///
    /// Maintains the FIFO/seq invariant by inserting at the position that keeps
    /// the queue sorted by `seq`. In practice re-queued deltas are always older
    /// than anything buffered after them, so this is an O(1) front-insert; the
    /// sorted insert is defensive. Capacity overflow drops the oldest.
    pub fn push_preserving(&mut self, seq: u64, msg: ClientMessage) {
        // Keep next_seq ahead of any seq we hold so future pushes never collide.
        if seq >= self.next_seq {
            self.next_seq = seq + 1;
        }
        if self.queue.len() >= self.capacity {
            self.queue.pop_front();
            self.dropped += 1;
        }
        // Find the first element with a strictly greater seq; insert before it.
        let pos = self
            .queue
            .iter()
            .position(|d| d.seq > seq)
            .unwrap_or(self.queue.len());
        self.queue.insert(pos, Delta { seq, msg });
    }

    /// Peek the oldest buffered delta without removing it.
    pub fn front(&self) -> Option<&Delta> {
        self.queue.front()
    }

    /// Remove and return the oldest buffered delta (FIFO order).
    pub fn pop(&mut self) -> Option<Delta> {
        self.queue.pop_front()
    }

    /// Drain **all** buffered deltas in order, leaving the buffer empty.
    ///
    /// This is the flush-on-reconnect path: the returned `Vec` is in strictly
    /// increasing `seq` order, ready to be replayed to the Hub.
    pub fn drain(&mut self) -> Vec<Delta> {
        self.queue.drain(..).collect()
    }

    /// Number of deltas currently buffered.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// The `seq` that the next [`DeltaBuffer::push`] will assign.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// The `seq` of the most recently enqueued delta (`0` if none yet).
    pub fn last_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    /// How many deltas have been dropped due to capacity overflow.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl Default for DeltaBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::{AgentKind, AgentRun, Confidence, State};

    fn run_upsert(run_id: &str) -> ClientMessage {
        ClientMessage::RunUpsert {
            session_id: "s1".into(),
            run: AgentRun::new(
                run_id,
                AgentKind::Codex,
                "native",
                "/",
                State::Working,
                Confidence::High,
                "2026-06-08T00:00:00Z",
            ),
            stamp: None,
        }
    }

    #[test]
    fn seq_starts_at_one_and_is_monotonic() {
        let mut b = DeltaBuffer::new();
        assert_eq!(b.next_seq(), 1);
        assert_eq!(b.push(run_upsert("r1")), 1);
        assert_eq!(b.push(run_upsert("r2")), 2);
        assert_eq!(b.push(run_upsert("r3")), 3);
        assert_eq!(b.last_seq(), 3);
        assert_eq!(b.next_seq(), 4);
    }

    #[test]
    fn drain_returns_in_seq_order_and_empties() {
        let mut b = DeltaBuffer::new();
        for i in 0..5 {
            b.push(run_upsert(&format!("r{i}")));
        }
        assert_eq!(b.len(), 5);
        let drained = b.drain();
        let seqs: Vec<u64> = drained.iter().map(|d| d.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5], "drain must be ordered by seq");
        assert!(b.is_empty(), "drain must empty the buffer");
    }

    #[test]
    fn pop_is_fifo() {
        let mut b = DeltaBuffer::new();
        b.push(run_upsert("a"));
        b.push(run_upsert("b"));
        assert_eq!(b.pop().unwrap().seq, 1);
        assert_eq!(b.pop().unwrap().seq, 2);
        assert!(b.pop().is_none());
    }

    #[test]
    fn front_peeks_without_removing() {
        let mut b = DeltaBuffer::new();
        b.push(run_upsert("a"));
        assert_eq!(b.front().unwrap().seq, 1);
        assert_eq!(b.len(), 1, "front must not consume");
    }

    #[test]
    fn capacity_overflow_drops_oldest_and_counts() {
        let mut b = DeltaBuffer::with_capacity(3);
        for i in 0..5 {
            b.push(run_upsert(&format!("r{i}")));
        }
        // Capacity 3: pushes 1..=5 → oldest (seq 1, 2) dropped, 3,4,5 remain.
        assert_eq!(b.len(), 3);
        assert_eq!(b.dropped(), 2);
        let drained = b.drain();
        let seqs: Vec<u64> = drained.iter().map(|d| d.seq).collect();
        assert_eq!(seqs, vec![3, 4, 5], "newest deltas survive overflow");
    }

    #[test]
    fn seq_keeps_increasing_across_drains() {
        // seq is per-reporter and never resets, even after a flush — this is
        // what makes the S6 (durable_id, seq) idempotency key globally ordered.
        let mut b = DeltaBuffer::new();
        b.push(run_upsert("a"));
        b.push(run_upsert("b"));
        b.drain();
        assert_eq!(b.push(run_upsert("c")), 3, "seq must not reset on drain");
        assert_eq!(b.last_seq(), 3);
    }

    #[test]
    fn zero_capacity_clamped_to_one() {
        let mut b = DeltaBuffer::with_capacity(0);
        b.push(run_upsert("a"));
        b.push(run_upsert("b"));
        assert_eq!(b.len(), 1, "capacity floor is 1");
        assert_eq!(b.front().unwrap().seq, 2, "kept the newest");
    }

    #[test]
    fn empty_buffer_drain_is_empty() {
        let mut b = DeltaBuffer::new();
        assert!(b.drain().is_empty());
        assert_eq!(b.last_seq(), 0);
    }

    #[test]
    fn push_preserving_keeps_seq_and_orders_ahead() {
        // Simulate a reconnect requeue: deltas 2,3 went unsent; delta 4 was
        // produced afterward. Requeuing 2,3 ahead of 4 must replay 2,3,4.
        let mut b = DeltaBuffer::new();
        b.push(run_upsert("a")); // seq 1
        b.push(run_upsert("b")); // seq 2
        b.push(run_upsert("c")); // seq 3
        let drained = b.drain(); // [1,2,3]
                                 // Pretend seq 1 was sent OK; 2,3 failed. Meanwhile seq 4 buffered.
        b.push(run_upsert("d")); // seq 4
        let new_tail = b.drain();
        // Requeue unsent [2,3] then the newer [4].
        for d in drained[1..].iter().chain(new_tail.iter()) {
            b.push_preserving(d.seq, d.msg.clone());
        }
        let seqs: Vec<u64> = b.drain().iter().map(|d| d.seq).collect();
        assert_eq!(seqs, vec![2, 3, 4], "requeued deltas replay in seq order");
    }

    #[test]
    fn push_preserving_advances_next_seq_to_avoid_collision() {
        let mut b = DeltaBuffer::with_capacity(16);
        b.push_preserving(10, run_upsert("x"));
        // A fresh push must not reuse seq <= 10.
        assert_eq!(b.push(run_upsert("y")), 11);
    }

    #[test]
    fn push_preserving_sorts_by_seq() {
        let mut b = DeltaBuffer::with_capacity(16);
        b.push_preserving(5, run_upsert("e"));
        b.push_preserving(2, run_upsert("b"));
        b.push_preserving(8, run_upsert("h"));
        let seqs: Vec<u64> = b.drain().iter().map(|d| d.seq).collect();
        assert_eq!(
            seqs,
            vec![2, 5, 8],
            "kept sorted by seq regardless of insert order"
        );
    }

    #[test]
    fn push_preserving_drops_oldest_on_overflow() {
        // Requeue more deltas than capacity: the oldest is evicted and counted,
        // exercising the capacity-overflow branch of push_preserving.
        let mut b = DeltaBuffer::with_capacity(2);
        b.push_preserving(1, run_upsert("a"));
        b.push_preserving(2, run_upsert("b"));
        b.push_preserving(3, run_upsert("c")); // over capacity → drop seq 1
        assert_eq!(b.len(), 2);
        assert_eq!(b.dropped(), 1);
        let seqs: Vec<u64> = b.drain().iter().map(|d| d.seq).collect();
        assert_eq!(seqs, vec![2, 3], "oldest requeued delta evicted on overflow");
    }

    #[test]
    fn default_matches_new() {
        // `DeltaBuffer::default()` must behave identically to `new()`.
        let mut d = DeltaBuffer::default();
        assert_eq!(d.next_seq(), 1);
        assert!(d.is_empty());
        assert_eq!(d.push(run_upsert("a")), 1);
    }
}
