//! Property test for the merge-engine rollup invariant (G0 gate criterion:
//! "Hub merge-engine property test `rollup = most-urgent across runs`").
//!
//! Strategy: generate an arbitrary set of runs with arbitrary states/urgencies,
//! feed them into the engine via the *same* delta path the wire uses
//! (`upsert_session` + `upsert_run`), and assert the session's stored
//! `rollup_state`/`rollup_urgency` equal the independently-recomputed max over
//! the runs — under *any* run set, including interleaved updates and removals.

use fleet_hub::merge::MergeEngine;
use fleet_protocol::{
    AgentKind, AgentRun, Confidence, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session, State, Urgency,
};
use proptest::prelude::*;

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
fn sess(id: &str) -> Session {
    Session::new(id, "t", loc(), srv(), State::Idle, "2026-06-08T00:00:00Z")
}

/// Independent reference rollup (deliberately NOT the engine's code path): the
/// max state and max urgency by the documented ranking. If this and the engine
/// ever disagree, the invariant is broken.
fn ref_state_rank(s: State) -> u8 {
    match s {
        State::Waiting => 5,
        State::Error => 4,
        State::Working => 3,
        State::Done => 2,
        State::Idle => 1,
        State::Dead => 0,
    }
}
fn ref_urgency_rank(u: Urgency) -> u8 {
    match u {
        Urgency::Approval => 3,
        Urgency::Question => 2,
        Urgency::IdleDone => 1,
        Urgency::None => 0,
    }
}

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
        Just(Some(Urgency::None)),
    ]
}

proptest! {
    /// For any non-empty set of runs, the engine's stored rollup equals the
    /// independently-computed most-urgent state/urgency across those runs.
    #[test]
    fn rollup_equals_max_over_runs(
        runs in proptest::collection::vec((arb_state(), arb_urgency()), 1..12)
    ) {
        let mut engine = MergeEngine::new();
        engine.upsert_session(sess("s1"));
        for (i, (state, urgency)) in runs.iter().enumerate() {
            let mut r = AgentRun::new(
                format!("r{i}"),
                AgentKind::Codex,
                "n",
                "/",
                *state,
                Confidence::High,
                "2026-06-08T00:00:00Z",
            );
            r.urgency = *urgency;
            engine.upsert_run("s1", r);
        }
        let session = engine.session("s1").unwrap();

        // Reference rollup state = the state with the highest rank.
        let expected_state = runs
            .iter()
            .map(|(s, _)| *s)
            .max_by_key(|&s| ref_state_rank(s))
            .unwrap();
        prop_assert_eq!(session.rollup_state, expected_state);

        // Reference rollup urgency = highest-rank urgency, normalized so that
        // Urgency::None ⇒ absence on the wire (Option None).
        let max_urg = runs
            .iter()
            .map(|(_, u)| u.unwrap_or(Urgency::None))
            .max_by_key(|&u| ref_urgency_rank(u))
            .unwrap();
        let expected_urgency = if max_urg == Urgency::None { None } else { Some(max_urg) };
        prop_assert_eq!(session.rollup_urgency, expected_urgency);

        // The engine's own invariant checker agrees.
        prop_assert!(MergeEngine::rollup_holds(session));
    }

    /// The invariant survives arbitrary interleavings of upsert and remove: we
    /// apply a random program of run mutations and assert the invariant holds
    /// at the end across every session.
    #[test]
    fn invariant_survives_arbitrary_program(
        ops in proptest::collection::vec(
            (0u8..3, 0u8..4, arb_state(), arb_urgency()),
            0..40
        )
    ) {
        let mut engine = MergeEngine::new();
        engine.upsert_session(sess("s1"));
        for (op, run_ix, state, urgency) in ops {
            let run_id = format!("r{run_ix}");
            match op {
                0 | 1 => {
                    // upsert (add or in-place update)
                    let mut r = AgentRun::new(
                        run_id,
                        AgentKind::Codex,
                        "n",
                        "/",
                        state,
                        Confidence::High,
                        "2026-06-08T00:00:00Z",
                    );
                    r.urgency = urgency;
                    engine.upsert_run("s1", r);
                }
                _ => {
                    // remove (possibly a run that isn't there — must be a no-op)
                    engine.remove_run("s1", &run_id);
                }
            }
            // Invariant must hold after EVERY step, not just at the end.
            prop_assert!(engine.all_rollups_hold());
        }
    }
}
