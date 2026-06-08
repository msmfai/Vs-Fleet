//! Property tests for the inbox reducer (PLAN S19, WORK_GRAPH `◆G3`:
//! "UI reducer determinism (snapshot+delta→view)").
//!
//! These complement the in-module unit tests with randomized event sequences:
//! 1. **Determinism** — folding the *same* sequence twice yields identical views.
//! 2. **Rollup agreement** — every tab's reduced `state`/`urgency` equals the
//!    most-urgent over the session's runs, using the shared protocol ordering
//!    (so the GUI face agrees with the Hub and the CLI face).
//! 3. **No panics / no ghosts** — arbitrary (possibly dangling) deltas never
//!    panic and never invent a tab for an unknown session.

use fleet_host_core::{AgentIcon, InboxModel, TabState};
use fleet_protocol::{
    rollup::{rollup_state, rollup_urgency},
    AgentKind, AgentRun, Confidence, Event, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session, State, Urgency,
};
use proptest::prelude::*;

// ── Generators ────────────────────────────────────────────────────────────────

fn arb_state() -> impl Strategy<Value = State> {
    prop_oneof![
        Just(State::Working),
        Just(State::Waiting),
        Just(State::Idle),
        Just(State::Done),
        Just(State::Error),
        Just(State::Dead),
    ]
}

fn arb_urgency() -> impl Strategy<Value = Option<Urgency>> {
    prop_oneof![
        Just(None),
        Just(Some(Urgency::Approval)),
        Just(Some(Urgency::Question)),
        Just(Some(Urgency::IdleDone)),
    ]
}

fn arb_kind() -> impl Strategy<Value = AgentKind> {
    prop_oneof![
        Just(AgentKind::ClaudeCode),
        Just(AgentKind::Codex),
        Just(AgentKind::Other),
    ]
}

fn loc() -> Location {
    Location {
        kind: LocationKind::Local,
        label: "l".into(),
        glyph: LocationGlyph::Laptop,
        attach_hint: None,
        extra: Extra::new(),
    }
}
fn srv() -> Server {
    Server {
        kind: ServerKind::Local,
        version: None,
        extra: Extra::new(),
    }
}

/// A small alphabet of ids keeps sessions/runs colliding so upserts and removes
/// actually exercise the keyed paths.
fn arb_session_id() -> impl Strategy<Value = String> {
    prop_oneof![Just("s0"), Just("s1"), Just("s2")].prop_map(String::from)
}
fn arb_run_id() -> impl Strategy<Value = String> {
    prop_oneof![Just("r0"), Just("r1"), Just("r2")].prop_map(String::from)
}

fn arb_run() -> impl Strategy<Value = AgentRun> {
    (arb_run_id(), arb_kind(), arb_state(), arb_urgency()).prop_map(|(id, kind, state, urgency)| {
        let mut r = AgentRun::new(
            id,
            kind,
            "native",
            "/",
            state,
            Confidence::High,
            "2026-06-08T00:00:00Z",
        );
        r.urgency = urgency;
        r
    })
}

fn arb_event() -> impl Strategy<Value = Event> {
    prop_oneof![
        (arb_session_id(), arb_state()).prop_map(|(id, st)| Event::session_added(Session::new(
            id,
            "t",
            loc(),
            srv(),
            st,
            "2026-06-08T00:00:00Z"
        ))),
        arb_session_id().prop_map(Event::session_removed),
        (arb_session_id(), arb_run()).prop_map(|(sid, run)| Event::run_added(sid, run)),
        (arb_session_id(), arb_run()).prop_map(|(sid, run)| Event::run_updated(sid, run)),
        (arb_session_id(), arb_run_id()).prop_map(|(sid, rid)| Event::run_removed(sid, rid)),
    ]
}

fn arb_event_seq() -> impl Strategy<Value = Vec<Event>> {
    prop::collection::vec(arb_event(), 0..40)
}

// ── Properties ────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    /// Determinism: the same event sequence yields identical views — the core
    /// `◆G3` reducer-determinism criterion, independent of any window.
    #[test]
    fn reduce_is_deterministic(seq in arb_event_seq()) {
        let mut a = InboxModel::new();
        a.apply_all(seq.clone());
        let mut b = InboxModel::new();
        b.apply_all(seq);
        prop_assert_eq!(a.view(), b.view());
    }

    /// Rollup agreement: every tab's reduced state/urgency equals the
    /// most-urgent over the live runs (shared protocol ordering). When a session
    /// has runs, the tab state must match `rollup_state`; urgency normalizes
    /// `Urgency::None` to absence. This is what keeps the GUI face consistent
    /// with the Hub and CLI.
    #[test]
    fn tab_rollup_matches_runs(seq in arb_event_seq()) {
        let mut m = InboxModel::new();
        // Track runs per session alongside the model to recompute the expected
        // rollup independently.
        use std::collections::HashMap;
        let mut runs: HashMap<String, Vec<AgentRun>> = HashMap::new();
        let mut present: std::collections::HashSet<String> = Default::default();

        for ev in &seq {
            match ev {
                Event::SessionAdded { session, .. }
                | Event::SessionUpdated { session, .. } => {
                    // A session upsert replaces the whole object, runs included.
                    // Our generator only emits run-less sessions, so this resets
                    // the tracked run list — exactly as the model does.
                    present.insert(session.session_id.clone());
                    runs.insert(session.session_id.clone(), session.runs.clone());
                }
                Event::SessionRemoved { session_id, .. } => {
                    present.remove(session_id);
                    runs.remove(session_id);
                }
                Event::RunAdded { session_id, run, .. }
                | Event::RunUpdated { session_id, run, .. } => {
                    if present.contains(session_id) {
                        let v = runs.entry(session_id.clone()).or_default();
                        if let Some(p) = v.iter_mut().position(|r| r.run_id == run.run_id) {
                            v[p] = run.clone();
                        } else {
                            v.push(run.clone());
                        }
                    }
                }
                Event::RunRemoved { session_id, run_id, .. } => {
                    if let Some(v) = runs.get_mut(session_id) {
                        v.retain(|r| r.run_id != *run_id);
                    }
                }
                Event::Snapshot { .. } => {}
            }
            m.apply(ev.clone());
        }

        let view = m.view();
        // No ghost tabs: every tab corresponds to a present session.
        for tab in &view.tabs {
            prop_assert!(present.contains(&tab.session_id));
            let session_runs = runs.get(&tab.session_id).cloned().unwrap_or_default();

            // State agreement (only asserted when there are runs; an empty
            // session keeps whatever rollup_state it was created with).
            if let Some(expected_state) = rollup_state(&session_runs) {
                prop_assert_eq!(tab.state, TabState::from_state(expected_state));
            }
            // Urgency agreement.
            let expected_urgency = match rollup_urgency(&session_runs) {
                Some(Urgency::None) | None => None,
                Some(u) => Some(u),
            };
            prop_assert_eq!(tab.urgency, expected_urgency);

            // run_count agreement.
            prop_assert_eq!(tab.run_count, session_runs.len());

            // Confidence honesty: confidence is Some IFF some run is waiting.
            let any_waiting = session_runs.iter().any(|r| r.state == State::Waiting);
            prop_assert_eq!(tab.confidence.is_some(), any_waiting);

            // agent_icon is None IFF no runs.
            prop_assert_eq!(tab.agent_icon == AgentIcon::None, session_runs.is_empty());
        }

        // Tab count equals present-session count.
        prop_assert_eq!(view.tabs.len(), present.len());
    }

    /// Arbitrary deltas never panic and never create a ghost tab for a session
    /// that was never added (snapshot-free run deltas).
    #[test]
    fn dangling_run_deltas_are_noops(sid in arb_session_id(), run in arb_run()) {
        let mut m = InboxModel::new();
        m.apply(Event::run_added(sid.clone(), run.clone()));
        m.apply(Event::run_updated(sid.clone(), run));
        m.apply(Event::run_removed(sid, "r0"));
        prop_assert!(m.view().is_empty());
    }
}
