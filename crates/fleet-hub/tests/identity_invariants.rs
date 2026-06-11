//! Property / fuzz tests that lock the durable-identity invariants:
//!
//! 1. monotonic per-run `seq`, applied **idempotently** by `(durable_id, seq)`;
//! 2. **ordered replay** by `seq` (last-writer-by-seq), not by arrival;
//! 3. **expiry GC drops the state entry AND its buffered-delta queue atomically**.
//!
//! The fuzz driver feeds arbitrary interleavings of **duplicate**, **out-of-order**,
//! and **reconnect-vs-fresh** deltas through both the pure [`ReclaimTable`] and the
//! real [`StateStore`], and asserts: **no ghost** (never a duplicate entry) and
//! **no regression** (a stale delta never overwrites a newer one).

use std::collections::HashMap;
use std::time::Duration;

use fleet_hub::reclaim::{Decision, DurableId, ReclaimTable};
use fleet_hub::StateStore;
use fleet_protocol::{
    AgentKind, AgentRun, Confidence, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session, State, SCHEMA_VERSION,
};
use proptest::prelude::*;

// ── fixtures ────────────────────────────────────────────────────────────────

fn session(id: &str, updated: &str) -> Session {
    Session {
        schema_version: SCHEMA_VERSION,
        session_id: id.into(),
        title: "t".into(),
        location: Location {
            kind: LocationKind::Local,
            label: "l".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        },
        editor: None,
        server: Server {
            kind: ServerKind::Local,
            version: None,
            extra: Extra::new(),
        },
        runs: vec![],
        rollup_state: State::Idle,
        rollup_urgency: None,
        muted: false,
        soloed: false,
        unread: false,
        tags: vec![],
        policy: None,
        updated_at: updated.into(),
        extra: Extra::new(),
    }
}

/// A run whose `last_message` encodes its logical seq, so a state-regression is
/// observable: if a stale (lower-seq) delta ever wins, the projected run's
/// `last_message` would go *backwards*.
fn run(run_id: &str, native: &str, seq_marker: u64, state: State) -> AgentRun {
    let mut r = AgentRun::new(
        run_id,
        AgentKind::Codex,
        native,
        "/work",
        state,
        Confidence::High,
        "2026-06-08T00:00:00Z",
    );
    r.last_message = Some(format!("seq={seq_marker}"));
    r
}

// ── one fuzzed delta in a generated program ─────────────────────────────────

#[derive(Debug, Clone)]
enum Op {
    /// Deliver a run delta for durable id `d` at (epoch, seq).
    Deliver { d: u8, epoch: u64, seq: u64 },
    /// A fresh-start: bump epoch for `d` (the reporter chose a new identity).
    /// Modeled here as a Deliver with `epoch = current+1, seq = 1`.
    FreshStart { d: u8 },
}

fn state_of(n: u8) -> State {
    State::ALL[(n as usize) % State::ALL.len()]
}

// A strategy for a single op against a small pool of durable ids (so duplicates
// and reorders actually collide).
fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        8 => (0u8..4, 0u64..3, 1u64..8)
            .prop_map(|(d, epoch, seq)| Op::Deliver { d, epoch, seq }),
        1 => (0u8..4).prop_map(|d| Op::FreshStart { d }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    /// INVARIANT 1 + 2 on the pure table: replaying an arbitrary program of
    /// duplicate / out-of-order / fresh-start deltas, a per-(id,epoch) reference
    /// model of "highest seq ever applied" must exactly match the table's
    /// decisions — every applied delta strictly advances the series; every
    /// dropped delta is a true duplicate or stale-epoch.
    #[test]
    fn reclaim_table_matches_reference_model(ops in prop::collection::vec(op_strategy(), 0..200)) {
        let mut table = ReclaimTable::new();
        // Reference: per durable id, the current epoch and the high-water seq.
        let mut model: HashMap<u8, (u64, u64)> = HashMap::new(); // d -> (epoch, hwm)

        for op in ops {
            match op {
                Op::Deliver { d, epoch, seq } => {
                    let id = DurableId::new(format!("d{d}"));
                    let decision = table.admit(&id, epoch, seq);
                    let entry = model.get(&d).copied();
                    let expected_apply = match entry {
                        None => true,                       // first sighting
                        Some((e, _)) if epoch > e => true,  // fresh-start wipe
                        Some((e, _)) if epoch < e => false, // stale epoch
                        Some((_, hwm)) => seq > hwm,        // same epoch: strictly newer
                    };
                    prop_assert_eq!(decision.applies(), expected_apply,
                        "decision/model disagree for d{} epoch{} seq{}: {:?}", d, epoch, seq, decision);
                    if decision.applies() {
                        // Update the reference model exactly as the table should.
                        match model.get_mut(&d) {
                            Some((e, hwm)) if epoch > *e => { *e = epoch; *hwm = seq; }
                            Some((e, hwm)) if epoch == *e && seq > *hwm => { *hwm = seq; }
                            Some(_) => {}
                            None => { model.insert(d, (epoch, seq)); }
                        }
                    }
                    // Table must agree with the model's current marks.
                    if let Some((e, hwm)) = model.get(&d) {
                        prop_assert_eq!(table.epoch(&id), Some(*e));
                        prop_assert_eq!(table.high_seq(&id), Some(*hwm));
                    }
                }
                Op::FreshStart { d } => {
                    let id = DurableId::new(format!("d{d}"));
                    let next_epoch = model.get(&d).map(|(e, _)| e + 1).unwrap_or(0);
                    let decision = table.admit(&id, next_epoch, 1);
                    prop_assert!(decision.applies(), "a fresh-start must always apply");
                    model.insert(d, (next_epoch, 1));
                    prop_assert_eq!(table.epoch(&id), Some(next_epoch));
                    prop_assert_eq!(table.high_seq(&id), Some(1), "fresh-start resets the series");
                }
            }
        }

        // NO GHOST: the table tracks exactly the distinct durable ids the model saw.
        prop_assert_eq!(table.len(), model.len(), "no ghost / no lost durable id");
    }

    /// INVARIANT 1 + 2 end-to-end through the real StateStore: the projected
    /// run's seq-marker must equal the **highest applied seq**, regardless of the
    /// arrival order (duplicates + reorders folded in). A stale delta must never
    /// regress the projected `last_message`.
    #[test]
    fn statestore_projection_is_last_writer_by_seq(
        deliveries in prop::collection::vec((1u64..12, any::<u8>()), 1..60)
    ) {
        let mut store = StateStore::open_in_memory().unwrap();
        store.apply_session_upsert(session("s1", "2026-06-08T00:00:00Z")).unwrap();
        let did = DurableId::new("native-d");

        let mut max_applied: u64 = 0;
        for (seq, st) in &deliveries {
            let r = run("r1", "native-d", *seq, state_of(*st));
            let (decision, _evs) = store
                .apply_run_upsert_seq("s1", r, &did, 0, *seq)
                .unwrap();
            if *seq > max_applied {
                prop_assert!(decision.applies(), "strictly-newer seq {} must apply", seq);
                max_applied = *seq;
            } else {
                prop_assert!(decision.drops(), "seq {} <= hwm {} must drop", seq, max_applied);
            }
        }

        // The projection reflects exactly the highest-seq delivery (last-writer).
        let s = store.engine().session("s1").unwrap();
        prop_assert_eq!(s.runs.len(), 1, "no ghost run: exactly one run under the session");
        let marker = s.runs[0].last_message.clone().unwrap();
        prop_assert_eq!(marker, format!("seq={max_applied}"),
            "projection must equal the last-writer-by-seq, not by arrival");
        // The dedup HWM equals the highest applied seq.
        prop_assert_eq!(store.reclaim().high_seq(&did), Some(max_applied));
    }
}

// ── invariant 3: atomic entry + buffered-delta-queue drop (deterministic) ────

#[test]
fn invariant3_session_expiry_drops_entry_and_dedup_queue_atomically() {
    let mut store = StateStore::open_in_memory().unwrap();
    // A stale session with two runs (two durable ids), each with advanced seq.
    store
        .apply_session_upsert(session("stale", "2026-06-08T00:00:00Z"))
        .unwrap();
    let d1 = DurableId::new("d1");
    let d2 = DurableId::new("d2");
    for seq in 1..=5 {
        store
            .apply_run_upsert_seq("stale", run("r1", "d1", seq, State::Working), &d1, 0, seq)
            .unwrap();
        store
            .apply_run_upsert_seq("stale", run("r2", "d2", seq, State::Working), &d2, 0, seq)
            .unwrap();
    }
    // A fresh session that must survive the sweep.
    store
        .apply_session_upsert(session("fresh", "2026-06-08T11:30:00Z"))
        .unwrap();

    // Pre-sweep: both dedup marks exist.
    assert_eq!(store.reclaim().high_seq(&d1), Some(5));
    assert_eq!(store.reclaim().high_seq(&d2), Some(5));
    assert_eq!(store.durables_of("stale").len(), 2);

    // Sweep with a 1h TTL at 12:00 → cutoff 11:00. `stale` (00:00) expires.
    let evs = store
        .sweep_expired_sessions("2026-06-08T12:00:00Z", Duration::from_secs(3600))
        .unwrap();
    assert!(evs.iter().any(|e| e.type_name() == "session.removed"));

    // The state entry AND its dedup queue vanished TOGETHER (atomically): both
    // marks gone, the index pruned, the session gone — no lingering seq state.
    assert!(
        store.engine().session("stale").is_none(),
        "session entry dropped"
    );
    assert!(
        store.reclaim().high_seq(&d1).is_none(),
        "d1 dedup queue dropped"
    );
    assert!(
        store.reclaim().high_seq(&d2).is_none(),
        "d2 dedup queue dropped"
    );
    assert!(store.durables_of("stale").is_empty(), "index pruned");
    assert!(store.reclaim().is_empty(), "no orphan reclaim marks left");

    // CRITICAL consequence: because the dedup queue dropped with the entry, a
    // later genuinely-fresh delta for d1 (even at seq 1, below the old HWM of 5)
    // is admitted — it is NOT wrongly rejected as a stale duplicate.
    store
        .apply_session_upsert(session("stale", "2026-06-08T12:00:01Z"))
        .unwrap();
    let (decision, evs) = store
        .apply_run_upsert_seq("stale", run("r1", "d1", 1, State::Working), &d1, 0, 1)
        .unwrap();
    assert!(decision.applies(), "post-GC fresh delta must be admitted");
    assert!(!evs.is_empty());
}

#[test]
fn duplicate_and_out_of_order_yield_no_ghost_no_regression() {
    // A scripted adversarial sequence: the classic S6 demo. Inject a duplicate
    // and an out-of-order delta and assert no ghost run, no state regression.
    let mut store = StateStore::open_in_memory().unwrap();
    store
        .apply_session_upsert(session("s1", "2026-06-08T00:00:00Z"))
        .unwrap();
    let did = DurableId::new("native-d");

    // Normal progression: working(1) → waiting(2) → working(3).
    store
        .apply_run_upsert_seq("s1", run("r1", "native-d", 1, State::Working), &did, 0, 1)
        .unwrap();
    store
        .apply_run_upsert_seq("s1", run("r1", "native-d", 2, State::Waiting), &did, 0, 2)
        .unwrap();
    store
        .apply_run_upsert_seq("s1", run("r1", "native-d", 3, State::Working), &did, 0, 3)
        .unwrap();

    // DUPLICATE: re-deliver seq 2 (waiting). Must NOT resurrect the waiting state.
    let (dup_decision, dup_evs) = store
        .apply_run_upsert_seq("s1", run("r1", "native-d", 2, State::Waiting), &did, 0, 2)
        .unwrap();
    assert!(dup_decision.drops(), "duplicate seq 2 must drop");
    assert!(dup_evs.is_empty(), "duplicate produces no broadcast");

    // OUT-OF-ORDER: a stale seq 1 (working) arrives late. Must drop, no regression.
    let (ooo_decision, _) = store
        .apply_run_upsert_seq("s1", run("r1", "native-d", 1, State::Idle), &did, 0, 1)
        .unwrap();
    assert!(ooo_decision.drops(), "stale seq 1 must drop");

    // The projection is exactly the last-writer-by-seq: seq 3, working.
    let s = store.engine().session("s1").unwrap();
    assert_eq!(s.runs.len(), 1, "NO GHOST: still exactly one run");
    assert_eq!(
        s.runs[0].state,
        State::Working,
        "NO REGRESSION: state at seq 3"
    );
    assert_eq!(s.runs[0].last_message.as_deref(), Some("seq=3"));
}

#[test]
fn reconnect_reclaims_existing_entry_no_duplicate() {
    // Reconnect (same epoch) reclaims the existing entry; fresh-start (epoch+1)
    // wipes it. Neither produces a ghost duplicate.
    let mut store = StateStore::open_in_memory().unwrap();
    store
        .apply_session_upsert(session("s1", "2026-06-08T00:00:00Z"))
        .unwrap();
    let did = DurableId::new("native-d");

    store
        .apply_run_upsert_seq("s1", run("r1", "native-d", 1, State::Working), &did, 0, 1)
        .unwrap();
    store
        .apply_run_upsert_seq("s1", run("r1", "native-d", 2, State::Idle), &did, 0, 2)
        .unwrap();

    // Reporter bounced and reconnects (epoch unchanged) — continues at seq 3.
    let (d, _) = store
        .apply_run_upsert_seq("s1", run("r1", "native-d", 3, State::Done), &did, 0, 3)
        .unwrap();
    assert_eq!(d, Decision::Apply);
    let s = store.engine().session("s1").unwrap();
    assert_eq!(s.runs.len(), 1, "reconnect must RECLAIM, not duplicate");
    assert_eq!(s.runs[0].state, State::Done);

    // Fresh-start: a new run instance reuses the durable id (epoch bump). Even at
    // seq 1 it applies (the wipe) and there is still exactly one run.
    let (d2, _) = store
        .apply_run_upsert_seq("s1", run("r1", "native-d", 1, State::Working), &did, 1, 1)
        .unwrap();
    assert_eq!(
        d2,
        Decision::ApplyFresh,
        "fresh-start wipes the prior series"
    );
    let s = store.engine().session("s1").unwrap();
    assert_eq!(s.runs.len(), 1, "fresh-start must not create a ghost");
    assert_eq!(s.runs[0].state, State::Working);
    assert_eq!(store.reclaim().epoch(&did), Some(1));
}
