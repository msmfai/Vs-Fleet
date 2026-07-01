// Inline unit tests for the Codex detection adapter.
//
// Included from `codex.rs` via `include!`. Covers: hook-event parsing (incl.
// camelCase aliases + schema-drift tolerance), every modelled transition, the
// approval gate (request-only — there is NO inbound decision event), the
// activity-driven auto-resolve, confidence honesty, the durable thread-id anchor,
// the real death path (reporter liveness timeout — Codex has no `SessionEnd`), and
// a property test proving the state machine has NO illegal edge.

use super::*;
use fleet_protocol::{Confidence, State, Urgency};
use proptest::prelude::*;

const THREAD: &str = "0199f3aa-thread-codex-1";

fn ev(kind: CodexHookKind) -> CodexHookEvent {
    CodexHookEvent {
        kind,
        thread_id: THREAD.to_string(),
        turn_id: Some("turn-1".into()),
        cwd: Some("/work".into()),
        tool_name: None,
        stop_hook_active: false,
        last_message: None,
        tool_use_id: None,
    }
}

fn tool_ev(kind: CodexHookKind, tool: &str) -> CodexHookEvent {
    let mut e = ev(kind);
    e.tool_name = Some(tool.to_string());
    e
}

fn machine() -> CodexStateMachine {
    CodexStateMachine::new(THREAD)
}

// ── parsing: each recorded field shape ───────────────────────────────────────

#[test]
fn parses_snake_case_pre_tool_use() {
    let json = r#"{"session_id":"t1","turn_id":"x","cwd":"/p","hook_event_name":"PreToolUse","tool_name":"Bash"}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.kind, CodexHookKind::PreToolUse);
    assert_eq!(e.thread_id, "t1");
    assert_eq!(e.turn_id.as_deref(), Some("x"));
    assert_eq!(e.cwd.as_deref(), Some("/p"));
    assert_eq!(e.tool_name.as_deref(), Some("Bash"));
}

#[test]
fn parses_camel_case_aliases() {
    let json = r#"{"sessionId":"t2","turnId":"y","hookEventName":"PermissionRequest","toolName":"ApplyPatch"}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.kind, CodexHookKind::PermissionRequest);
    assert_eq!(e.thread_id, "t2");
    assert_eq!(e.turn_id.as_deref(), Some("y"));
    assert_eq!(e.tool_name.as_deref(), Some("ApplyPatch"));
}

#[test]
fn parses_thread_id_alias() {
    let json = r#"{"thread_id":"t3","hook_event_name":"SessionStart"}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.thread_id, "t3");
    assert_eq!(e.kind, CodexHookKind::SessionStart);
}

#[test]
fn stop_hook_active_is_parsed() {
    let json = r#"{"session_id":"t","hook_event_name":"Stop","stop_hook_active":true}"#;
    assert!(CodexHookEvent::parse(json).unwrap().stop_hook_active);
    let json2 = r#"{"session_id":"t","hook_event_name":"Stop"}"#;
    assert!(!CodexHookEvent::parse(json2).unwrap().stop_hook_active);
}

// ── parsing: no inbound decision / no SessionEnd (the 2026 Codex contract) ────

#[test]
fn session_end_is_not_a_modelled_event() {
    // REGRESSION (item 2): Codex has NO `SessionEnd` hook. A payload naming it must
    // parse to Other (forward-compatible), NEVER to a death-driving variant.
    let e = CodexHookEvent::parse(r#"{"session_id":"t","hook_event_name":"SessionEnd"}"#).unwrap();
    assert_eq!(e.kind, CodexHookKind::Other("SessionEnd".into()));
}

#[test]
fn inbound_decision_field_is_not_parsed() {
    // REGRESSION (item 3): a `PermissionRequest` `decision` is a hook OUTPUT, not an
    // inbound event. A payload carrying one still parses as a fresh PermissionRequest
    // (the decision is ignored) — there is no `decision`/`is_approval_response` API.
    let json = r#"{"session_id":"t","hook_event_name":"PermissionRequest","decision":"allow","tool_name":"Bash"}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.kind, CodexHookKind::PermissionRequest);
    // And it drives the gate (waiting), never an auto-resolve.
    let mut m = CodexStateMachine::new("t");
    m.apply(&e);
    assert_eq!(m.state(), State::Waiting);
    assert_eq!(m.confidence(), Confidence::High);
}

// ── parsing: error / drift handling (never panics, never overstates) ─────────

#[test]
fn unknown_hook_name_is_other_not_error() {
    let json = r#"{"session_id":"t","hook_event_name":"FutureHook","weird":[1,2]}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.kind, CodexHookKind::Other("FutureHook".into()));
}

#[test]
fn malformed_json_is_error_not_panic() {
    let json = r#"{"session_id":"t","hook_event_name":"PreToolUse""#; // truncated
    assert!(matches!(
        CodexHookEvent::parse(json),
        Err(CodexParseError::InvalidJson(_))
    ));
}

#[test]
fn missing_event_name_is_error() {
    let json = r#"{"session_id":"t"}"#;
    assert_eq!(
        CodexHookEvent::parse(json),
        Err(CodexParseError::MissingEventName)
    );
}

#[test]
fn missing_thread_id_is_error() {
    // Identity honesty: an un-anchored hook cannot become a durable run.
    let json = r#"{"hook_event_name":"PreToolUse"}"#;
    assert_eq!(
        CodexHookEvent::parse(json),
        Err(CodexParseError::MissingThreadId)
    );
}

#[test]
fn empty_thread_id_is_error() {
    let json = r#"{"session_id":"","hook_event_name":"PreToolUse"}"#;
    assert_eq!(
        CodexHookEvent::parse(json),
        Err(CodexParseError::MissingThreadId)
    );
}

#[test]
fn unknown_payload_fields_are_ignored_not_fatal() {
    let json = r#"{"session_id":"t","hook_event_name":"PreToolUse","brand_new_field":{"a":1},"another":42}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.kind, CodexHookKind::PreToolUse);
}

// ── transitions: SessionStart → idle (liveness only, no dead-revive) ─────────

#[test]
fn new_machine_starts_idle_inferred() {
    let m = machine();
    assert_eq!(m.state(), State::Idle);
    assert!(m.urgency().is_none());
    assert_eq!(m.confidence(), Confidence::Inferred);
    assert!(!m.awaiting_approval());
}

#[test]
fn session_start_on_live_thread_is_liveness_noop() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit)); // working
    let t = m.apply(&ev(CodexHookKind::SessionStart));
    assert_eq!(m.state(), State::Working, "SessionStart must not reset a live thread");
    assert!(!t.changed);
    assert!(t.liveness, "SessionStart is a liveness ping");
}

// ── transitions: activity → working ──────────────────────────────────────────

#[test]
fn user_prompt_submit_goes_working() {
    let mut m = machine();
    let t = m.apply(&ev(CodexHookKind::UserPromptSubmit));
    assert_eq!(m.state(), State::Working);
    assert!(m.urgency().is_none());
    assert_eq!(m.confidence(), Confidence::Inferred, "working is never High");
    assert!(t.changed);
    assert!(t.liveness);
}

#[test]
fn pre_tool_use_goes_working() {
    let mut m = machine();
    let t = m.apply(&tool_ev(CodexHookKind::PreToolUse, "Bash"));
    assert_eq!(m.state(), State::Working);
    assert_eq!(m.confidence(), Confidence::Inferred);
    assert!(t.liveness);
}

#[test]
fn repeated_pre_tool_use_stays_working_noop() {
    let mut m = machine();
    m.apply(&tool_ev(CodexHookKind::PreToolUse, "Bash"));
    let t = m.apply(&tool_ev(CodexHookKind::PreToolUse, "Read"));
    assert_eq!(m.state(), State::Working);
    assert!(!t.changed, "working→working is a no-op (idempotent)");
    assert!(t.liveness, "but still a liveness signal");
}

// ── transitions: PostToolUse / compaction → telemetry only ───────────────────

#[test]
fn post_tool_use_is_liveness_only() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit)); // working
    let t = m.apply(&tool_ev(CodexHookKind::PostToolUse, "Bash"));
    assert_eq!(m.state(), State::Working, "PostToolUse never flips state");
    assert!(!t.changed);
    assert!(t.liveness);
}

#[test]
fn compaction_hooks_are_liveness_only() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit));
    for k in [CodexHookKind::PreCompact, CodexHookKind::PostCompact] {
        let t = m.apply(&ev(k));
        assert_eq!(m.state(), State::Working);
        assert!(!t.changed);
        assert!(t.liveness);
    }
}

#[test]
fn unmodelled_hook_is_a_bare_noop() {
    // An `Other(_)` kind is an idempotent no-op — not even a liveness ping.
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit)); // working
    let t = m.apply(&ev(CodexHookKind::Other("Frobnicate".into())));
    assert_eq!(m.state(), State::Working);
    assert!(!t.changed);
    assert!(!t.liveness);
}

// ── transitions: PermissionRequest → waiting+approval (HIGH) ─────────────────

#[test]
fn permission_request_goes_waiting_approval_high() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit)); // working
    let t = m.apply(&tool_ev(CodexHookKind::PermissionRequest, "Bash"));
    assert_eq!(m.state(), State::Waiting);
    assert_eq!(m.urgency(), Some(Urgency::Approval));
    assert_eq!(
        m.confidence(),
        Confidence::High,
        "PermissionRequest is THE authoritative waiting signal (invariant 5)"
    );
    assert!(m.awaiting_approval());
    assert!(t.changed);
}

#[test]
fn waiting_run_snapshot_has_waiting_since_and_high() {
    let mut m = machine();
    m.apply(&tool_ev(CodexHookKind::PermissionRequest, "Bash"));
    let run = m.to_run("codex:run-1", "2026-06-08T00:00:00Z");
    assert_eq!(run.state, State::Waiting);
    assert_eq!(run.urgency, Some(Urgency::Approval));
    assert_eq!(run.confidence, Confidence::High);
    assert_eq!(run.waiting_since.as_deref(), Some("2026-06-08T00:00:00Z"));
    assert_eq!(run.native_id, THREAD, "durable anchor = thread.id");
    assert_eq!(run.agent_kind, AgentKind::Codex);
}

// ── S13: auto-resolve happens via SUBSEQUENT ACTIVITY (no decision event) ─────

#[test]
fn activity_after_permission_auto_resolves() {
    // The user answers in the terminal; Codex resumes and fires PreToolUse. No
    // inbound decision event exists — fresh activity clears the gate.
    let mut m = machine();
    m.apply(&tool_ev(CodexHookKind::PermissionRequest, "Bash")); // waiting
    let t = m.apply(&tool_ev(CodexHookKind::PreToolUse, "Bash"));
    assert_eq!(m.state(), State::Working);
    assert!(!m.awaiting_approval());
    assert_eq!(m.confidence(), Confidence::Inferred);
    assert!(t.resolved_approval, "auto-resolve flagged");
    assert!(t.changed);
}

#[test]
fn stop_during_waiting_clears_approval_to_done() {
    let mut m = machine();
    m.apply(&tool_ev(CodexHookKind::PermissionRequest, "Bash"));
    let t = m.apply(&ev(CodexHookKind::Stop));
    assert_eq!(m.state(), State::Done, "a real Stop ends the turn → done");
    assert!(!m.awaiting_approval());
    assert!(t.resolved_approval);
}

// ── transitions: Stop → done (the turn-complete signal) ──────────────────────

#[test]
fn stop_goes_done() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit)); // working
    let t = m.apply(&ev(CodexHookKind::Stop));
    assert_eq!(m.state(), State::Done, "the Stop event IS the turn-complete signal");
    assert!(m.urgency().is_none());
    assert_eq!(m.confidence(), Confidence::Inferred);
    assert!(t.changed);
}

#[test]
fn stop_from_within_stop_hook_is_liveness_only_not_done() {
    // stop_hook_active=true → a continuation, NOT a real turn boundary → stays
    // working (a liveness ping), never a completion claim.
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit)); // working
    let mut stop = ev(CodexHookKind::Stop);
    stop.stop_hook_active = true;
    let t = m.apply(&stop);
    assert_eq!(m.state(), State::Working);
    assert!(!t.changed);
    assert!(t.liveness);
}

#[test]
fn done_and_idle_are_distinct() {
    // D9: done must never collapse into idle. Idle = fresh thread (nothing
    // produced); Done = a real Stop (turn finished).
    let a = machine();
    let mut b = machine();
    b.apply(&ev(CodexHookKind::UserPromptSubmit));
    b.apply(&ev(CodexHookKind::Stop));
    assert_ne!(a.state(), b.state());
    assert_eq!(a.state(), State::Idle);
    assert_eq!(b.state(), State::Done);
}

// ── the REAL death path: reporter liveness timeout (Codex has no SessionEnd) ──

#[test]
fn codex_run_reaches_dead_via_reporter_liveness_timeout() {
    // REGRESSION (item 2): with no `SessionEnd`, a Codex run's death is driven by
    // the reporter's liveness timeout — NOT by any hook. Drive a real
    // UpsertRun(working) from the Codex adapter into a ReporterCore, go silent past
    // the timeout, and assert `reap_timeouts` marks the run dead.
    use crate::reporter::{ReporterConfig, ReporterCore};
    use std::time::Duration;

    let mut a = CodexAdapter::new();
    let cmds = a.ingest(&ev(CodexHookKind::UserPromptSubmit)).0;
    let run = match &cmds[0] {
        ReporterCommand::UpsertRun(r) => r.clone(),
        other => panic!("expected UpsertRun(working), got {other:?}"),
    };
    assert_eq!(run.state, State::Working);

    let mut config = ReporterConfig::new("sess-codex");
    config.liveness_timeout = Duration::from_secs(30);
    let timeout = config.liveness_timeout;
    let mut core = ReporterCore::new(config);
    core.apply(ReporterCommand::UpsertRun(run.clone()));
    // Within the grace: still alive, nothing reaped.
    core.advance_to(timeout);
    assert!(core.reap_timeouts().is_empty(), "alive within grace");
    // Past the grace with no further liveness → reaped dead (the real path).
    core.advance_to(timeout + Duration::from_secs(1));
    let dead = core.reap_timeouts();
    assert_eq!(dead, vec![run.run_id.clone()], "silent Codex run is reaped dead");
}

// ── thread-id routing guard ──────────────────────────────────────────────────

#[test]
fn foreign_thread_event_is_ignored() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit)); // working
    let mut foreign = ev(CodexHookKind::PermissionRequest);
    foreign.thread_id = "some-other-thread".into();
    let t = m.apply(&foreign);
    assert_eq!(m.state(), State::Working, "foreign thread must not mutate us");
    assert!(!t.changed);
}

// ── cwd propagation ──────────────────────────────────────────────────────────

#[test]
fn cwd_is_captured_from_hooks() {
    let mut m = machine();
    let mut e = ev(CodexHookKind::UserPromptSubmit);
    e.cwd = Some("/Users/dev/repo".into());
    m.apply(&e);
    assert_eq!(m.cwd(), "/Users/dev/repo");
    assert_eq!(m.to_run("r", "ts").cwd, "/Users/dev/repo");
}

// ── full lifecycle sequence (S11→S13 end to end) ─────────────────────────────

#[test]
fn full_lifecycle_working_waiting_working_done() {
    let mut m = machine();
    let seq = [
        (CodexHookKind::SessionStart, State::Idle),
        (CodexHookKind::UserPromptSubmit, State::Working),
        (CodexHookKind::PreToolUse, State::Working),
        (CodexHookKind::PermissionRequest, State::Waiting),
    ];
    for (k, expect) in seq {
        let e = tool_ev(k, "Bash");
        m.apply(&e);
        assert_eq!(m.state(), expect);
    }
    // answer the approval in-terminal → resume via activity
    m.apply(&tool_ev(CodexHookKind::PreToolUse, "Bash"));
    assert_eq!(m.state(), State::Working);
    // turn ends → done
    m.apply(&ev(CodexHookKind::Stop));
    assert_eq!(m.state(), State::Done);
}

// ── adapter: hook stream → ReporterCommands ──────────────────────────────────

#[test]
fn adapter_mints_run_and_emits_upsert_on_first_event() {
    let mut a = CodexAdapter::new();
    let (cmds, t) = a.ingest(&ev(CodexHookKind::UserPromptSubmit));
    assert_eq!(a.thread_count(), 1);
    assert!(t.changed);
    assert_eq!(cmds.len(), 1);
    match &cmds[0] {
        ReporterCommand::UpsertRun(run) => {
            assert_eq!(run.state, State::Working);
            assert_eq!(run.native_id, THREAD);
            assert_eq!(run.agent_kind, AgentKind::Codex);
        }
        other => panic!("expected UpsertRun, got {other:?}"),
    }
}

#[test]
fn adapter_run_id_is_stable_per_thread() {
    let mut a = CodexAdapter::new();
    a.ingest(&ev(CodexHookKind::UserPromptSubmit));
    let id1 = a.run_id_of(THREAD).unwrap().to_string();
    a.ingest(&tool_ev(CodexHookKind::PreToolUse, "Bash"));
    let id2 = a.run_id_of(THREAD).unwrap().to_string();
    assert_eq!(id1, id2, "same thread keeps its Fleet run-id");
}

#[test]
fn adapter_liveness_only_for_telemetry_hook() {
    let mut a = CodexAdapter::new();
    a.ingest(&ev(CodexHookKind::UserPromptSubmit)); // upsert
    let (cmds, _) = a.ingest(&tool_ev(CodexHookKind::PostToolUse, "Bash"));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], ReporterCommand::Liveness { .. }));
}

#[test]
fn adapter_noop_emits_nothing_extra() {
    let mut a = CodexAdapter::new();
    a.ingest(&ev(CodexHookKind::UserPromptSubmit)); // working
    let (cmds, _) = a.ingest(&ev(CodexHookKind::UserPromptSubmit)); // working again
    // working→working is a no-op state-wise but is a liveness signal.
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], ReporterCommand::Liveness { .. }));
}

#[test]
fn adapter_multiplexes_threads() {
    let mut a = CodexAdapter::new();
    let mut e1 = ev(CodexHookKind::PermissionRequest);
    e1.thread_id = "thread-A".into();
    let mut e2 = ev(CodexHookKind::UserPromptSubmit);
    e2.thread_id = "thread-B".into();
    a.ingest(&e1);
    a.ingest(&e2);
    assert_eq!(a.thread_count(), 2);
    assert_eq!(a.state_of("thread-A"), Some(State::Waiting));
    assert_eq!(a.state_of("thread-B"), Some(State::Working));
    assert_ne!(a.run_id_of("thread-A"), a.run_id_of("thread-B"));
}

#[test]
fn adapter_ingest_json_parse_error_yields_no_commands() {
    let mut a = CodexAdapter::new();
    let cmds = a.ingest_json("{ not json");
    assert!(cmds.is_empty());
    assert_eq!(a.thread_count(), 0, "a bad line must not create a ghost run");
}

#[test]
fn adapter_ingest_json_approval_then_activity_resolves() {
    let mut a = CodexAdapter::new();
    let req = r#"{"session_id":"tX","hook_event_name":"PermissionRequest","tool_name":"Bash"}"#;
    let cmds = a.ingest_json(req);
    assert_eq!(cmds.len(), 1);
    assert_eq!(a.state_of("tX"), Some(State::Waiting));
    // Resolve via subsequent activity (there is no inbound decision event).
    let act = r#"{"session_id":"tX","hook_event_name":"PreToolUse","tool_name":"Bash"}"#;
    let cmds = a.ingest_json(act);
    assert_eq!(cmds.len(), 1);
    assert_eq!(a.state_of("tX"), Some(State::Working));
}

// ── CONFIDENCE HONESTY INVARIANT (G2) ────────────────────────────────────────

#[test]
fn high_confidence_only_ever_from_permission_request() {
    // Drive the machine through every event kind and assert High confidence appears
    // ONLY when the run is Waiting (from a PermissionRequest) — the machine never
    // reaches Dead itself (no SessionEnd), so Waiting is the sole High path.
    for kind in all_kinds() {
        let mut m = machine();
        let mut e = ev(kind.clone());
        e.tool_name = Some("Bash".into());
        m.apply(&e);
        if m.confidence() == Confidence::High {
            assert_eq!(
                m.state(),
                State::Waiting,
                "High confidence leaked into state {:?} via {:?}",
                m.state(),
                kind
            );
        }
    }
}

fn all_kinds() -> Vec<CodexHookKind> {
    vec![
        CodexHookKind::SessionStart,
        CodexHookKind::UserPromptSubmit,
        CodexHookKind::PreToolUse,
        CodexHookKind::PostToolUse,
        CodexHookKind::PermissionRequest,
        CodexHookKind::Stop,
        CodexHookKind::PreCompact,
        CodexHookKind::PostCompact,
        CodexHookKind::Other("Weird".into()),
    ]
}

// ── PROPERTY: NO ILLEGAL EDGE (G2) ───────────────────────────────────────────

fn arb_kind() -> impl Strategy<Value = CodexHookKind> {
    prop_oneof![
        Just(CodexHookKind::SessionStart),
        Just(CodexHookKind::UserPromptSubmit),
        Just(CodexHookKind::PreToolUse),
        Just(CodexHookKind::PostToolUse),
        Just(CodexHookKind::PermissionRequest),
        Just(CodexHookKind::Stop),
        Just(CodexHookKind::PreCompact),
        Just(CodexHookKind::PostCompact),
    ]
}

#[derive(Debug, Clone)]
struct ArbEvent {
    kind: CodexHookKind,
    stop_hook_active: bool,
}

fn arb_event() -> impl Strategy<Value = ArbEvent> {
    (arb_kind(), any::<bool>()).prop_map(|(kind, stop_hook_active)| ArbEvent {
        kind,
        stop_hook_active,
    })
}

fn legal_states() -> [State; 6] {
    State::ALL
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 600, ..ProptestConfig::default() })]

    /// For ANY sequence of hook events, the machine:
    ///  - never panics,
    ///  - always rests in one of the six legal States,
    ///  - keeps the confidence-honesty invariant (High ⇒ Waiting),
    ///  - keeps `urgency=approval ⇔ state=waiting` and `awaiting_approval ⇒ waiting`.
    #[test]
    fn no_illegal_edge(events in prop::collection::vec(arb_event(), 0..200)) {
        let mut m = machine();
        for ae in events {
            let mut e = ev(ae.kind.clone());
            e.tool_name = Some("Bash".into());
            e.stop_hook_active = ae.stop_hook_active;
            let t = m.apply(&e);

            // Rests in a legal state.
            prop_assert!(legal_states().contains(&m.state()));
            // Transition's reported state matches the machine.
            prop_assert_eq!(t.state, m.state());

            // Confidence honesty: High only ever accompanies Waiting.
            if m.confidence() == Confidence::High {
                prop_assert_eq!(m.state(), State::Waiting, "High leaked into {:?}", m.state());
            }
            // urgency=approval IFF waiting.
            match m.state() {
                State::Waiting => {
                    prop_assert_eq!(m.urgency(), Some(Urgency::Approval));
                    prop_assert!(m.awaiting_approval());
                    prop_assert_eq!(m.confidence(), Confidence::High);
                }
                _ => {
                    prop_assert!(m.urgency().is_none(), "no urgency outside waiting");
                    prop_assert!(!m.awaiting_approval(), "no pending approval outside waiting");
                }
            }
        }
    }

    /// `to_run` is always internally consistent with the machine's fields, and
    /// `waiting_since` is set exactly when waiting.
    #[test]
    fn to_run_consistent_with_machine(events in prop::collection::vec(arb_event(), 0..100)) {
        let mut m = machine();
        for ae in events {
            let mut e = ev(ae.kind.clone());
            e.stop_hook_active = ae.stop_hook_active;
            m.apply(&e);
        }
        let run = m.to_run("r", "2026-06-08T00:00:00Z");
        prop_assert_eq!(run.state, m.state());
        prop_assert_eq!(run.urgency, m.urgency());
        prop_assert_eq!(run.confidence, m.confidence());
        prop_assert_eq!(run.native_id, m.thread_id());
        prop_assert_eq!(run.waiting_since.is_some(), m.state() == State::Waiting);
    }
}

// ── real Codex payload fidelity (last_assistant_message + tool_use_id) ───────

#[test]
fn codex_stop_surfaces_last_assistant_message_as_done_preview() {
    let json = r#"{"session_id":"t","hook_event_name":"Stop","stop_hook_active":false,
        "last_assistant_message":"Refactored the parser; all green."}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.last_message.as_deref(), Some("Refactored the parser; all green."));
    let mut m = CodexStateMachine::new("t");
    m.apply(&e);
    assert_eq!(m.state(), State::Done);
    assert_eq!(
        m.to_run("r", "2026-06-08T00:00:00Z").last_message.as_deref(),
        Some("Refactored the parser; all green.")
    );
}

#[test]
fn codex_done_preview_falls_back_to_task_complete() {
    // A Stop with no last_assistant_message → the generic "Task complete." preview.
    let mut m = CodexStateMachine::new("t");
    m.apply(&CodexHookEvent::parse(r#"{"session_id":"t","hook_event_name":"Stop"}"#).unwrap());
    assert_eq!(m.state(), State::Done);
    assert_eq!(
        m.to_run("r", "t").last_message.as_deref(),
        Some("Task complete.")
    );
}

#[test]
fn codex_pre_tool_use_parses_tool_use_id() {
    let json = r#"{"session_id":"t","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"tool_789"}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.tool_use_id.as_deref(), Some("tool_789"));
}

#[test]
fn codex_permission_request_is_authoritative_waiting_high() {
    let mut m = CodexStateMachine::new("t");
    m.apply(&CodexHookEvent::parse(r#"{"session_id":"t","hook_event_name":"PermissionRequest","tool_name":"Bash","tool_use_id":"tool_1"}"#).unwrap());
    assert_eq!(m.state(), State::Waiting);
    assert_eq!(m.confidence(), Confidence::High, "PermissionRequest is authoritative");
}

// ── last_message inbox previews for each state ───────────────────────────────

#[test]
fn waiting_preview_names_the_tool_or_is_generic() {
    // Waiting WITH a tool → "Approve <tool>?"; without → "Approval required".
    let mut m = machine();
    m.apply(&tool_ev(CodexHookKind::PermissionRequest, "Bash"));
    assert_eq!(m.to_run("r", "t").last_message.as_deref(), Some("Approve Bash?"));

    let mut m2 = machine();
    m2.apply(&ev(CodexHookKind::PermissionRequest)); // no tool_name
    assert_eq!(m2.to_run("r", "t").last_message.as_deref(), Some("Approval required"));
}

#[test]
fn working_preview_names_the_running_tool() {
    let mut m = machine();
    m.apply(&tool_ev(CodexHookKind::PreToolUse, "Bash"));
    assert_eq!(m.to_run("r", "t").last_message.as_deref(), Some("Running Bash…"));
}

#[test]
fn idle_and_error_states_have_no_preview() {
    // Idle (fresh thread) has no preview, and any un-modelled state (here Error,
    // which the machine never emits via hooks) falls through to None.
    let m = machine();
    assert_eq!(m.state(), State::Idle);
    assert!(m.to_run("r", "t").last_message.is_none());

    let mut e = machine();
    e.state = State::Error;
    let run = e.to_run("r", "2026-06-08T00:00:00Z");
    assert_eq!(run.state, State::Error);
    assert!(run.last_message.is_none(), "an un-modelled state yields no preview");
}

// ── CodexHookKind::name(): every variant round-trips its wire token ───────────

#[test]
fn hook_kind_name_round_trips_every_variant() {
    let cases = [
        (CodexHookKind::SessionStart, "SessionStart"),
        (CodexHookKind::UserPromptSubmit, "UserPromptSubmit"),
        (CodexHookKind::PreToolUse, "PreToolUse"),
        (CodexHookKind::PostToolUse, "PostToolUse"),
        (CodexHookKind::PermissionRequest, "PermissionRequest"),
        (CodexHookKind::Stop, "Stop"),
        (CodexHookKind::PreCompact, "PreCompact"),
        (CodexHookKind::PostCompact, "PostCompact"),
    ];
    for (kind, token) in cases {
        assert_eq!(kind.name(), token, "name() must emit the wire token");
        assert_eq!(CodexHookKind::from_name(token), kind);
    }
    // Other(s) surfaces the raw token unchanged (schema-drift forward-compat), and
    // `SessionEnd` is no longer a modelled variant → it parses to Other.
    let other = CodexHookKind::Other("FutureHook".to_string());
    assert_eq!(other.name(), "FutureHook");
    assert_eq!(
        CodexHookKind::from_name("SessionEnd"),
        CodexHookKind::Other("SessionEnd".to_string())
    );
}

// ── CodexHookEvent::from_value: the serde_json::Value entry point ─────────────

#[test]
fn from_value_parses_a_json_value() {
    let v: serde_json::Value = serde_json::json!({
        "session_id": "tv",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_use_id": "tool_42",
    });
    let e = CodexHookEvent::from_value(v).unwrap();
    assert_eq!(e.kind, CodexHookKind::PreToolUse);
    assert_eq!(e.thread_id, "tv");
    assert_eq!(e.tool_name.as_deref(), Some("Bash"));
    assert_eq!(e.tool_use_id.as_deref(), Some("tool_42"));
}

#[test]
fn from_value_wrong_shape_is_invalid_json_error() {
    let v = serde_json::json!(["not", "an", "object"]);
    assert!(matches!(
        CodexHookEvent::from_value(v),
        Err(CodexParseError::InvalidJson(_))
    ));
}

#[test]
fn from_value_missing_thread_id_errors() {
    let v = serde_json::json!({ "hook_event_name": "PreToolUse" });
    assert_eq!(
        CodexHookEvent::from_value(v),
        Err(CodexParseError::MissingThreadId)
    );
}

// ── adapter accessors: machine_of + forget ───────────────────────────────────

#[test]
fn adapter_machine_of_borrows_tracked_thread() {
    let mut a = CodexAdapter::new();
    a.ingest(&tool_ev(CodexHookKind::PermissionRequest, "Bash"));
    let m = a.machine_of(THREAD).expect("thread is tracked");
    assert_eq!(m.state(), State::Waiting);
    assert_eq!(m.confidence(), Confidence::High);
    assert_eq!(m.thread_id(), THREAD);
    assert!(a.machine_of("no-such-thread").is_none());
}

#[test]
fn adapter_forget_drops_thread_and_reports_presence() {
    let mut a = CodexAdapter::new();
    a.ingest(&ev(CodexHookKind::UserPromptSubmit));
    assert_eq!(a.thread_count(), 1);
    assert!(a.forget(THREAD), "forget returns true for a tracked thread");
    assert_eq!(a.thread_count(), 0);
    assert!(a.state_of(THREAD).is_none());
    assert!(!a.forget(THREAD), "forget returns false for an absent thread");
    a.ingest(&ev(CodexHookKind::UserPromptSubmit));
    assert_eq!(a.thread_count(), 1);
    assert_eq!(a.state_of(THREAD), Some(State::Working));
}
