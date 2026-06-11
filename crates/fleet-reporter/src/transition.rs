//! Pure, sync scripted-transition generator for the fake reporter (the engineering spec).
//!
//! This module is completely free of async, network I/O, and hub-crate
//! dependencies. It operates solely on [`fleet_protocol`] types — the shared
//! contract — and exposes a [`ScriptedStep`] enum that the [`crate::fake`]
//! module maps to hub wire messages.
//!
//! Being pure makes it exhaustively unit-testable without standing up a Hub or
//! tokio runtime: the tests here are the "heavy unit tests" the the engineering spec node
//! requires for the scripted-transition generator.
//!
//! ## Scripted lifecycle
//!
//! The spec (the engineering spec) requires:
//! ```text
//! working → waiting(approval) → working → dead
//! ```
//!
//! Concretely the generator emits the following ordered steps:
//!
//! | Step | Variant | Notes |
//! |---|---|---|
//! | 0 | `RegisterSession` | registers the session shell (no runs yet) |
//! | 1 | `UpsertRun` (state=working) | run appears, rollup → working |
//! | 2 | `UpsertRun` (state=waiting, urgency=approval, confidence=high) | approval gate |
//! | 3 | `UpsertRun` (state=working) | approval answered, back to working |
//! | 4 | `UpsertRun` (state=dead) | agent process exited |
//! | 5 | `RemoveSession` | session gone after grace |

use fleet_protocol::{
    AgentKind, AgentRun, Confidence, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session, State, Urgency, SCHEMA_VERSION,
};

/// A single scripted step — a reporter-native description of what to send.
#[derive(Debug, Clone)]
pub enum ScriptedStep {
    /// Register the session (no runs). Hub sees a `session.upsert`.
    RegisterSession {
        session: Session,
        label: &'static str,
    },
    /// Upsert (add or update) a run. Hub sees a `run.upsert`.
    UpsertRun {
        session_id: String,
        run: AgentRun,
        label: &'static str,
    },
    /// Remove the session. Hub sees a `session.remove`.
    RemoveSession {
        session_id: String,
        label: &'static str,
    },
}

impl ScriptedStep {
    /// Human-readable label for this step.
    pub fn label(&self) -> &'static str {
        match self {
            ScriptedStep::RegisterSession { label, .. } => label,
            ScriptedStep::UpsertRun { label, .. } => label,
            ScriptedStep::RemoveSession { label, .. } => label,
        }
    }

    /// Wire type name that this step will produce on the hub.
    pub fn wire_type(&self) -> &'static str {
        match self {
            ScriptedStep::RegisterSession { .. } => "session.upsert",
            ScriptedStep::UpsertRun { .. } => "run.upsert",
            ScriptedStep::RemoveSession { .. } => "session.remove",
        }
    }

    /// If this is a `UpsertRun`, return references to the session_id and run.
    pub fn as_run(&self) -> Option<(&str, &AgentRun)> {
        match self {
            ScriptedStep::UpsertRun {
                session_id, run, ..
            } => Some((session_id.as_str(), run)),
            _ => None,
        }
    }

    /// If this is a `RegisterSession`, return a reference to the session.
    pub fn as_session(&self) -> Option<&Session> {
        match self {
            ScriptedStep::RegisterSession { session, .. } => Some(session),
            _ => None,
        }
    }

    /// If this is a `RemoveSession`, return the session_id.
    pub fn as_remove(&self) -> Option<&str> {
        match self {
            ScriptedStep::RemoveSession { session_id, .. } => Some(session_id.as_str()),
            _ => None,
        }
    }
}

/// The full scripted transition sequence for a fake session+run.
///
/// # Example
/// ```
/// use fleet_reporter::transition::TransitionScript;
/// use fleet_protocol::State;
///
/// let script = TransitionScript::new("sess-fake-1", "run-fake-1");
/// let steps = script.generate("2026-06-08T00:00:00Z");
/// // Step 0: session registration
/// assert_eq!(steps[0].wire_type(), "session.upsert");
/// // Step 1: working
/// let (_, run) = steps[1].as_run().unwrap();
/// assert_eq!(run.state, State::Working);
/// // Step 4: dead
/// let (_, run) = steps[4].as_run().unwrap();
/// assert_eq!(run.state, State::Dead);
/// ```
#[derive(Debug, Clone)]
pub struct TransitionScript {
    pub session_id: String,
    pub run_id: String,
}

impl TransitionScript {
    /// Create a new script for the given hardcoded session+run ids.
    pub fn new(session_id: impl Into<String>, run_id: impl Into<String>) -> Self {
        TransitionScript {
            session_id: session_id.into(),
            run_id: run_id.into(),
        }
    }

    /// Generate the full ordered sequence of [`ScriptedStep`]s.
    ///
    /// `ts` is used as the `updated_at` timestamp for every object (ISO-8601
    /// string). The generator is pure: same inputs → same output.
    pub fn generate(&self, ts: &str) -> Vec<ScriptedStep> {
        vec![
            // Step 0 — register session shell (no runs yet)
            ScriptedStep::RegisterSession {
                session: self.make_session(ts),
                label: "session.upsert (register)",
            },
            // Step 1 — run appears: working
            ScriptedStep::UpsertRun {
                session_id: self.session_id.clone(),
                run: self.make_run(State::Working, None, ts),
                label: "run.upsert (working)",
            },
            // Step 2 — waiting for approval (high confidence — authoritative signal)
            ScriptedStep::UpsertRun {
                session_id: self.session_id.clone(),
                run: self.make_run_waiting(ts),
                label: "run.upsert (waiting/approval)",
            },
            // Step 3 — back to working (approval answered)
            ScriptedStep::UpsertRun {
                session_id: self.session_id.clone(),
                run: self.make_run(State::Working, None, ts),
                label: "run.upsert (working again)",
            },
            // Step 4 — dead (process exited)
            ScriptedStep::UpsertRun {
                session_id: self.session_id.clone(),
                run: self.make_run(State::Dead, None, ts),
                label: "run.upsert (dead)",
            },
            // Step 5 — session removed (after grace)
            ScriptedStep::RemoveSession {
                session_id: self.session_id.clone(),
                label: "session.remove",
            },
        ]
    }

    // ---- helpers ----

    fn make_session(&self, ts: &str) -> Session {
        Session {
            schema_version: SCHEMA_VERSION,
            session_id: self.session_id.clone(),
            title: "fake-session (--fake mode)".into(),
            location: Location {
                kind: LocationKind::Local,
                label: "fake-laptop".into(),
                glyph: LocationGlyph::Laptop,
                attach_hint: None,
                extra: Extra::new(),
            },
            editor: None,
            server: Server {
                kind: ServerKind::Local,
                version: Some("fake-0.1".into()),
                extra: Extra::new(),
            },
            runs: Vec::new(),
            rollup_state: State::Idle,
            rollup_urgency: None,
            muted: false,
            soloed: false,
            unread: false,
            tags: vec!["fake".into()],
            policy: None,
            updated_at: ts.into(),
            extra: Extra::new(),
        }
    }

    fn make_run(&self, state: State, urgency: Option<Urgency>, ts: &str) -> AgentRun {
        AgentRun {
            schema_version: SCHEMA_VERSION,
            run_id: self.run_id.clone(),
            agent_kind: AgentKind::ClaudeCode,
            native_id: "fake-native-session-id-0001".into(),
            cwd: "/fake/work".into(),
            state,
            urgency,
            last_message: match state {
                State::Working => Some("Thinking…".into()),
                State::Dead => Some("Process exited (fake).".into()),
                _ => None,
            },
            waiting_since: None,
            // Non-waiting states: Inferred (invariant 5 — they're not from an
            // authoritative signal for confidence purposes).
            confidence: Confidence::Inferred,
            diff_summary: None,
            updated_at: ts.into(),
            extra: Extra::new(),
        }
    }

    fn make_run_waiting(&self, ts: &str) -> AgentRun {
        AgentRun {
            schema_version: SCHEMA_VERSION,
            run_id: self.run_id.clone(),
            agent_kind: AgentKind::ClaudeCode,
            native_id: "fake-native-session-id-0001".into(),
            cwd: "/fake/work".into(),
            state: State::Waiting,
            urgency: Some(Urgency::Approval),
            last_message: Some("Do you approve this change?".into()),
            // Invariant 5 (confidence honesty): the fake scripts a real
            // PermissionRequest equivalent — scripted, authoritative → High.
            waiting_since: Some(ts.into()),
            confidence: Confidence::High,
            diff_summary: None,
            updated_at: ts.into(),
            extra: Extra::new(),
        }
    }
}

// ---- unit tests (heavy: every structural property of the sequence) --------

#[cfg(test)]
mod tests {
    use super::*;

    const TS: &str = "2026-06-08T12:00:00Z";
    const SESSION_ID: &str = "sess-fake-test";
    const RUN_ID: &str = "run-fake-test";

    fn script() -> TransitionScript {
        TransitionScript::new(SESSION_ID, RUN_ID)
    }

    fn steps() -> Vec<ScriptedStep> {
        script().generate(TS)
    }

    // ── structural / count ──────────────────────────────────────────────────

    #[test]
    fn generates_exactly_six_steps() {
        assert_eq!(steps().len(), 6, "expected 6 scripted steps");
    }

    #[test]
    fn wire_types_in_order() {
        let s = steps();
        let types: Vec<&str> = s.iter().map(|s| s.wire_type()).collect();
        assert_eq!(
            types,
            &[
                "session.upsert",
                "run.upsert",
                "run.upsert",
                "run.upsert",
                "run.upsert",
                "session.remove",
            ],
            "wire type sequence must match spec"
        );
    }

    // ── step 0: session registration ────────────────────────────────────────

    #[test]
    fn step0_session_has_correct_id() {
        let s = steps();
        let sess = s[0].as_session().expect("step 0 must be RegisterSession");
        assert_eq!(sess.session_id, SESSION_ID);
    }

    #[test]
    fn step0_session_has_no_runs() {
        let s = steps();
        let sess = s[0].as_session().unwrap();
        assert!(
            sess.runs.is_empty(),
            "initial session upsert must carry no runs"
        );
    }

    #[test]
    fn step0_session_carries_schema_version() {
        let s = steps();
        let sess = s[0].as_session().unwrap();
        assert_eq!(sess.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn step0_session_has_fake_tag() {
        let s = steps();
        let sess = s[0].as_session().unwrap();
        assert!(sess.tags.contains(&"fake".to_string()));
    }

    #[test]
    fn step0_session_rollup_state_is_idle() {
        // The session shell starts Idle; the rollup updates when a run is added.
        let s = steps();
        let sess = s[0].as_session().unwrap();
        assert_eq!(sess.rollup_state, State::Idle);
    }

    // ── step 1: working ─────────────────────────────────────────────────────

    #[test]
    fn step1_run_state_is_working() {
        let s = steps();
        let (sid, run) = s[1].as_run().expect("step 1 must be UpsertRun");
        assert_eq!(sid, SESSION_ID);
        assert_eq!(run.run_id, RUN_ID);
        assert_eq!(run.state, State::Working);
    }

    #[test]
    fn step1_run_has_no_urgency() {
        let s = steps();
        let (_, run) = s[1].as_run().unwrap();
        assert!(
            run.urgency.is_none(),
            "working run must have no urgency on the wire"
        );
    }

    #[test]
    fn step1_run_carries_schema_version() {
        let s = steps();
        let (_, run) = s[1].as_run().unwrap();
        assert_eq!(run.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn step1_run_agent_kind_is_claude_code() {
        let s = steps();
        let (_, run) = s[1].as_run().unwrap();
        assert!(matches!(run.agent_kind, AgentKind::ClaudeCode));
    }

    #[test]
    fn step1_run_has_last_message() {
        let s = steps();
        let (_, run) = s[1].as_run().unwrap();
        assert!(
            run.last_message.is_some(),
            "working run must have a last_message"
        );
    }

    // ── step 2: waiting(approval) ────────────────────────────────────────────

    #[test]
    fn step2_state_is_waiting() {
        let s = steps();
        let (_, run) = s[2].as_run().expect("step 2 must be UpsertRun");
        assert_eq!(run.state, State::Waiting);
    }

    #[test]
    fn step2_urgency_is_approval() {
        let s = steps();
        let (_, run) = s[2].as_run().unwrap();
        assert_eq!(
            run.urgency,
            Some(Urgency::Approval),
            "waiting step must carry urgency=approval"
        );
    }

    #[test]
    fn step2_confidence_is_high() {
        // Invariant 5 (confidence honesty): the fake scripts this as the
        // authoritative approval signal — confidence must be High, not Inferred.
        let s = steps();
        let (_, run) = s[2].as_run().unwrap();
        assert_eq!(
            run.confidence,
            Confidence::High,
            "scripted approval must carry confidence=high (invariant 5)"
        );
    }

    #[test]
    fn step2_waiting_since_is_set() {
        let s = steps();
        let (_, run) = s[2].as_run().unwrap();
        assert!(
            run.waiting_since.is_some(),
            "waiting step must have waiting_since set"
        );
    }

    #[test]
    fn step2_has_approval_message() {
        let s = steps();
        let (_, run) = s[2].as_run().unwrap();
        let msg = run.last_message.as_deref().unwrap_or("");
        assert!(!msg.is_empty(), "waiting run must have a last_message");
    }

    #[test]
    fn step2_waiting_since_matches_timestamp() {
        let s = steps();
        let (_, run) = s[2].as_run().unwrap();
        assert_eq!(run.waiting_since.as_deref(), Some(TS));
    }

    // ── step 3: working again ────────────────────────────────────────────────

    #[test]
    fn step3_state_is_working() {
        let s = steps();
        let (_, run) = s[3].as_run().expect("step 3 must be UpsertRun");
        assert_eq!(run.state, State::Working);
    }

    #[test]
    fn step3_urgency_is_absent() {
        let s = steps();
        let (_, run) = s[3].as_run().unwrap();
        assert!(
            run.urgency.is_none(),
            "second working step must not retain approval urgency"
        );
    }

    #[test]
    fn step3_waiting_since_is_absent() {
        let s = steps();
        let (_, run) = s[3].as_run().unwrap();
        assert!(
            run.waiting_since.is_none(),
            "second working step must not retain waiting_since"
        );
    }

    // ── step 4: dead ─────────────────────────────────────────────────────────

    #[test]
    fn step4_state_is_dead() {
        let s = steps();
        let (_, run) = s[4].as_run().expect("step 4 must be UpsertRun");
        assert_eq!(run.state, State::Dead);
    }

    #[test]
    fn step4_run_id_is_stable() {
        // The run_id must be the same across all UpsertRun steps (same run
        // transitioning, not a replacement).
        let s = steps();
        for (i, step) in s.iter().enumerate().take(5).skip(1) {
            let (_, run) = step.as_run().unwrap();
            assert_eq!(run.run_id, RUN_ID, "run_id must be stable at step {i}");
        }
    }

    #[test]
    fn step4_session_id_is_stable() {
        let s = steps();
        for (i, step) in s.iter().enumerate().take(5).skip(1) {
            let (sid, _) = step.as_run().unwrap();
            assert_eq!(sid, SESSION_ID, "session_id must be stable at step {i}");
        }
    }

    #[test]
    fn step4_has_last_message() {
        let s = steps();
        let (_, run) = s[4].as_run().unwrap();
        assert!(
            run.last_message.is_some(),
            "dead run must have a last_message"
        );
    }

    // ── step 5: session.remove ───────────────────────────────────────────────

    #[test]
    fn step5_removes_correct_session() {
        let s = steps();
        let sid = s[5].as_remove().expect("step 5 must be RemoveSession");
        assert_eq!(sid, SESSION_ID);
    }

    // ── state sequence property ──────────────────────────────────────────────

    #[test]
    fn run_state_sequence_matches_spec() {
        // Spec: working → waiting(approval) → working → dead
        let s = steps();
        let states: Vec<State> = (1..=4).map(|i| s[i].as_run().unwrap().1.state).collect();
        assert_eq!(
            states,
            vec![State::Working, State::Waiting, State::Working, State::Dead],
            "state sequence must be working→waiting→working→dead"
        );
    }

    // ── pings property ───────────────────────────────────────────────────────

    #[test]
    fn only_waiting_step_pings() {
        // Invariant: only State::Waiting has pings() = true.
        // Exactly one step (step 2) should carry a pinging state.
        let s = steps();
        let ping_steps: Vec<usize> = (1..=4)
            .filter(|&i| s[i].as_run().unwrap().1.state.pings())
            .collect();
        assert_eq!(
            ping_steps,
            vec![2],
            "exactly one step (index 2) should produce a pinging state"
        );
    }

    // ── confidence honesty invariant ─────────────────────────────────────────

    #[test]
    fn confidence_high_only_on_waiting_step() {
        // Invariant 5: High confidence must only appear on the waiting step.
        let s = steps();
        for (i, step) in s.iter().enumerate().take(5).skip(1) {
            let (_, run) = step.as_run().unwrap();
            if run.state == State::Waiting {
                assert_eq!(
                    run.confidence,
                    Confidence::High,
                    "waiting step must have High confidence"
                );
            } else {
                assert_ne!(
                    run.confidence,
                    Confidence::High,
                    "non-waiting step {i} must not have High confidence"
                );
            }
        }
    }

    // ── JSON round-trip for all protocol objects ─────────────────────────────

    #[test]
    fn session_round_trips_through_json() {
        let s = steps();
        let sess = s[0].as_session().unwrap();
        let json = serde_json::to_string(sess).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(
            serde_json::to_value(sess).unwrap(),
            serde_json::to_value(&back).unwrap()
        );
    }

    #[test]
    fn all_runs_round_trip_through_json() {
        let s = steps();
        for (i, step) in s.iter().enumerate().take(5).skip(1) {
            let (_, run) = step.as_run().unwrap();
            let json = serde_json::to_string(run).unwrap();
            let back: AgentRun = serde_json::from_str(&json).unwrap();
            let v1 = serde_json::to_value(run).unwrap();
            let v2 = serde_json::to_value(&back).unwrap();
            assert_eq!(v1, v2, "step {i} run must round-trip through JSON");
        }
    }

    // ── determinism / idempotency ────────────────────────────────────────────

    #[test]
    fn generate_is_deterministic() {
        let sc = script();
        let a = sc.generate(TS);
        let b = sc.generate(TS);
        assert_eq!(a.len(), b.len());
        for (i, (sa, sb)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(sa.label(), sb.label(), "label mismatch at step {i}");
            assert_eq!(
                sa.wire_type(),
                sb.wire_type(),
                "wire_type mismatch at step {i}"
            );
            // Compare JSON-serialized payloads.
            match (sa, sb) {
                (
                    ScriptedStep::RegisterSession { session: sa, .. },
                    ScriptedStep::RegisterSession { session: sb, .. },
                ) => assert_eq!(
                    serde_json::to_value(sa).unwrap(),
                    serde_json::to_value(sb).unwrap(),
                    "step {i} session must match"
                ),
                (
                    ScriptedStep::UpsertRun { run: ra, .. },
                    ScriptedStep::UpsertRun { run: rb, .. },
                ) => assert_eq!(
                    serde_json::to_value(ra).unwrap(),
                    serde_json::to_value(rb).unwrap(),
                    "step {i} run must match"
                ),
                (
                    ScriptedStep::RemoveSession { session_id: a, .. },
                    ScriptedStep::RemoveSession { session_id: b, .. },
                ) => assert_eq!(a, b, "step {i} remove session_id must match"),
                _ => panic!("step {i} variant mismatch"),
            }
        }
    }

    // ── independent sequences ────────────────────────────────────────────────

    #[test]
    fn different_ids_produce_independent_sequences() {
        let s1 = TransitionScript::new("sess-A", "run-A").generate(TS);
        let s2 = TransitionScript::new("sess-B", "run-B").generate(TS);

        assert_eq!(s1[0].as_session().unwrap().session_id, "sess-A");
        assert_eq!(s2[0].as_session().unwrap().session_id, "sess-B");

        let (sid1, run1) = s1[1].as_run().unwrap();
        let (sid2, run2) = s2[1].as_run().unwrap();
        assert_eq!(sid1, "sess-A");
        assert_eq!(sid2, "sess-B");
        assert_eq!(run1.run_id, "run-A");
        assert_eq!(run2.run_id, "run-B");
    }

    // ── helper method contracts ──────────────────────────────────────────────

    #[test]
    fn as_session_returns_none_for_non_session_steps() {
        let s = steps();
        for i in [1, 2, 3, 4, 5] {
            assert!(
                s[i].as_session().is_none(),
                "step {i} must not parse as RegisterSession"
            );
        }
    }

    #[test]
    fn as_run_returns_none_for_non_run_steps() {
        let s = steps();
        for i in [0, 5] {
            assert!(
                s[i].as_run().is_none(),
                "step {i} must not parse as UpsertRun"
            );
        }
    }

    #[test]
    fn as_remove_returns_none_for_non_remove_steps() {
        let s = steps();
        for (i, step) in s.iter().enumerate().take(5) {
            assert!(
                step.as_remove().is_none(),
                "step {i} must not parse as RemoveSession"
            );
        }
    }

    #[test]
    fn all_steps_have_non_empty_labels() {
        for (i, s) in steps().iter().enumerate() {
            assert!(
                !s.label().is_empty(),
                "step {i} must have a non-empty label"
            );
        }
    }

    // ── timestamps propagate ────────────────────────────────────────────────

    #[test]
    fn updated_at_matches_given_timestamp() {
        let s = steps();
        let sess = s[0].as_session().unwrap();
        assert_eq!(sess.updated_at, TS);
        for (i, step) in s.iter().enumerate().take(5).skip(1) {
            let (_, run) = step.as_run().unwrap();
            assert_eq!(
                run.updated_at, TS,
                "run at step {i} must carry the given timestamp"
            );
        }
    }

    // ── native_id is stable ─────────────────────────────────────────────────

    #[test]
    fn native_id_is_stable_across_all_run_steps() {
        let s = steps();
        let native_ids: Vec<&str> = (1..=4)
            .map(|i| s[i].as_run().unwrap().1.native_id.as_str())
            .collect();
        let first = native_ids[0];
        assert!(
            native_ids.iter().all(|id| *id == first),
            "native_id must be stable across all run steps"
        );
    }
}
