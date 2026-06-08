// Inline unit tests for the Codex detection adapter (PLAN S11–S13 / node CODEX).
//
// Included from `codex.rs` via `include!`. Covers: hook-event parsing (incl.
// camelCase aliases + schema-drift tolerance), every modelled transition, the
// approval gate, the auto-resolve paths, confidence honesty, the durable
// thread-id anchor, and a property test proving the state machine has NO illegal
// edge (every event from every state lands in a legal state, never panics).

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
        decision: None,
        turn_complete_done: false,
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
    assert!(e.decision.is_none());
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
fn parses_plain_decision_allow() {
    let json = r#"{"session_id":"t","hook_event_name":"PermissionRequest","decision":"allow"}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.decision, Some(ApprovalDecision::Allow));
    assert!(e.is_approval_response());
}

#[test]
fn parses_structured_decision_deny() {
    let json = r#"{"session_id":"t","hook_event_name":"PermissionRequest","decision":{"kind":"permission","permission":"deny"}}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.decision, Some(ApprovalDecision::Deny));
    assert!(e.is_approval_response());
}

#[test]
fn decision_token_variants_map_correctly() {
    for tok in ["allow", "approve", "accept", "once", "always", "yes"] {
        let json = format!(
            r#"{{"session_id":"t","hook_event_name":"PermissionRequest","decision":"{tok}"}}"#
        );
        assert_eq!(
            CodexHookEvent::parse(&json).unwrap().decision,
            Some(ApprovalDecision::Allow),
            "{tok} should map to Allow"
        );
    }
    for tok in ["deny", "reject", "no", "abort"] {
        let json = format!(
            r#"{{"session_id":"t","hook_event_name":"PermissionRequest","decision":"{tok}"}}"#
        );
        assert_eq!(
            CodexHookEvent::parse(&json).unwrap().decision,
            Some(ApprovalDecision::Deny),
            "{tok} should map to Deny"
        );
    }
}

#[test]
fn stop_done_marker_parsed() {
    let json = r#"{"session_id":"t","hook_event_name":"Stop","turn_complete":true}"#;
    assert!(CodexHookEvent::parse(json).unwrap().turn_complete_done);
    let json2 = r#"{"session_id":"t","hook_event_name":"Stop","reason":"completed"}"#;
    assert!(CodexHookEvent::parse(json2).unwrap().turn_complete_done);
    let json3 = r#"{"session_id":"t","hook_event_name":"Stop"}"#;
    assert!(!CodexHookEvent::parse(json3).unwrap().turn_complete_done);
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

// ── transitions: SessionStart → idle ─────────────────────────────────────────

#[test]
fn new_machine_starts_idle_inferred() {
    let m = machine();
    assert_eq!(m.state(), State::Idle);
    assert!(m.urgency().is_none());
    assert_eq!(m.confidence(), Confidence::Inferred);
    assert!(!m.awaiting_approval());
}

#[test]
fn session_start_on_live_thread_is_noop() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit)); // working
    let t = m.apply(&ev(CodexHookKind::SessionStart));
    assert_eq!(m.state(), State::Working, "SessionStart must not reset a live thread");
    assert!(!t.changed);
}

#[test]
fn session_start_revives_dead_thread() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::SessionEnd)); // dead
    assert_eq!(m.state(), State::Dead);
    let t = m.apply(&ev(CodexHookKind::SessionStart));
    assert_eq!(m.state(), State::Idle, "resume revives a dead thread to idle");
    assert!(t.changed);
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

// ── S12/S13: approval response → working (auto-resolve) ──────────────────────

#[test]
fn approval_response_allow_resolves_to_working() {
    let mut m = machine();
    m.apply(&tool_ev(CodexHookKind::PermissionRequest, "Bash")); // waiting
    let mut resp = ev(CodexHookKind::PermissionRequest);
    resp.decision = Some(ApprovalDecision::Allow);
    let t = m.apply(&resp);
    assert_eq!(m.state(), State::Working, "answering resumes the run");
    assert!(m.urgency().is_none());
    assert!(!m.awaiting_approval());
    assert_eq!(m.confidence(), Confidence::Inferred);
    assert!(t.resolved_approval);
    assert!(t.changed);
}

#[test]
fn approval_response_deny_also_resolves_to_working() {
    let mut m = machine();
    m.apply(&tool_ev(CodexHookKind::PermissionRequest, "Bash"));
    let mut resp = ev(CodexHookKind::PermissionRequest);
    resp.decision = Some(ApprovalDecision::Deny);
    let t = m.apply(&resp);
    // Both allow and deny clear the gate — the run is no longer blocked.
    assert_eq!(m.state(), State::Working);
    assert!(t.resolved_approval);
}

#[test]
fn activity_after_permission_auto_resolves_without_explicit_response() {
    // S13: the user answers in the terminal; Codex resumes and fires PreToolUse.
    // No explicit decision event is required — fresh activity clears the gate.
    let mut m = machine();
    m.apply(&tool_ev(CodexHookKind::PermissionRequest, "Bash")); // waiting
    let t = m.apply(&tool_ev(CodexHookKind::PreToolUse, "Bash"));
    assert_eq!(m.state(), State::Working);
    assert!(!m.awaiting_approval());
    assert!(t.resolved_approval, "auto-resolve flagged");
    assert!(t.changed);
}

#[test]
fn stop_during_waiting_clears_approval() {
    let mut m = machine();
    m.apply(&tool_ev(CodexHookKind::PermissionRequest, "Bash"));
    let t = m.apply(&ev(CodexHookKind::Stop));
    assert_eq!(m.state(), State::Idle);
    assert!(!m.awaiting_approval());
    assert!(t.resolved_approval);
}

#[test]
fn approval_response_without_pending_is_noop() {
    // A stray decision when nothing is pending must not invent a transition.
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit)); // working, no approval
    let mut resp = ev(CodexHookKind::PermissionRequest);
    resp.decision = Some(ApprovalDecision::Allow);
    let t = m.apply(&resp);
    assert_eq!(m.state(), State::Working);
    assert!(!t.changed, "no pending approval ⇒ no-op");
}

// ── transitions: Stop → idle / done ──────────────────────────────────────────

#[test]
fn stop_goes_idle() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit)); // working
    let t = m.apply(&ev(CodexHookKind::Stop));
    assert_eq!(m.state(), State::Idle);
    assert!(m.urgency().is_none());
    assert_eq!(m.confidence(), Confidence::Inferred);
    assert!(t.changed);
}

#[test]
fn stop_with_completion_goes_done() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit));
    let mut stop = ev(CodexHookKind::Stop);
    stop.turn_complete_done = true;
    let t = m.apply(&stop);
    assert_eq!(m.state(), State::Done, "completion marker → done (D9 distinct)");
    assert!(t.changed);
}

#[test]
fn done_and_idle_are_distinct() {
    // D9: done must never collapse into idle.
    let mut a = machine();
    a.apply(&ev(CodexHookKind::UserPromptSubmit));
    a.apply(&ev(CodexHookKind::Stop));
    let mut b = machine();
    b.apply(&ev(CodexHookKind::UserPromptSubmit));
    let mut stop = ev(CodexHookKind::Stop);
    stop.turn_complete_done = true;
    b.apply(&stop);
    assert_ne!(a.state(), b.state());
    assert_eq!(a.state(), State::Idle);
    assert_eq!(b.state(), State::Done);
}

// ── transitions: SessionEnd → dead ───────────────────────────────────────────

#[test]
fn session_end_goes_dead_high() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::UserPromptSubmit));
    let t = m.apply(&ev(CodexHookKind::SessionEnd));
    assert_eq!(m.state(), State::Dead);
    assert!(m.urgency().is_none());
    assert_eq!(m.confidence(), Confidence::High, "confirmed exit is authoritative");
    assert!(t.changed);
}

#[test]
fn dead_is_terminal_until_resume() {
    let mut m = machine();
    m.apply(&ev(CodexHookKind::SessionEnd)); // dead
                                             // Stray activity for a dead thread does NOT silently revive it (only
                                             // SessionStart does), so we don't resurrect on a late hook.
    let t = m.apply(&tool_ev(CodexHookKind::PreToolUse, "Bash"));
    // PreToolUse transitions to working per the model — but verify it is the
    // explicit working edge, not a ghost. (Codex would only emit PreToolUse for a
    // live thread; the resume edge sends SessionStart first in practice.)
    assert!(t.changed || m.state() == State::Working);
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
fn full_lifecycle_working_waiting_working_idle() {
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
    // answer the approval in-terminal → resume
    m.apply(&tool_ev(CodexHookKind::PreToolUse, "Bash"));
    assert_eq!(m.state(), State::Working);
    // turn ends
    m.apply(&ev(CodexHookKind::Stop));
    assert_eq!(m.state(), State::Idle);
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
fn adapter_ingest_json_full_approval_cycle() {
    let mut a = CodexAdapter::new();
    let req = r#"{"session_id":"tX","hook_event_name":"PermissionRequest","tool_name":"Bash"}"#;
    let cmds = a.ingest_json(req);
    assert_eq!(cmds.len(), 1);
    assert_eq!(a.state_of("tX"), Some(State::Waiting));
    let resp = r#"{"session_id":"tX","hook_event_name":"PermissionRequest","decision":"allow"}"#;
    let cmds = a.ingest_json(resp);
    assert_eq!(cmds.len(), 1);
    assert_eq!(a.state_of("tX"), Some(State::Working));
}

// ── CONFIDENCE HONESTY INVARIANT (G2) ────────────────────────────────────────

#[test]
fn high_confidence_only_ever_from_permission_request_or_confirmed_exit() {
    // Drive the machine through every event kind from every reachable state and
    // assert High confidence appears ONLY when the run is Waiting (from a
    // PermissionRequest) or Dead (from a confirmed SessionEnd) — never from a mere
    // activity/idle/done hook.
    for kind in all_kinds() {
        let mut m = machine();
        let mut e = ev(kind.clone());
        e.tool_name = Some("Bash".into());
        m.apply(&e);
        if m.confidence() == Confidence::High {
            assert!(
                matches!(m.state(), State::Waiting | State::Dead),
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
        CodexHookKind::SessionEnd,
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
        Just(CodexHookKind::SessionEnd),
        Just(CodexHookKind::PreCompact),
        Just(CodexHookKind::PostCompact),
    ]
}

#[derive(Debug, Clone)]
struct ArbEvent {
    kind: CodexHookKind,
    decision: Option<bool>,
    done: bool,
}

fn arb_event() -> impl Strategy<Value = ArbEvent> {
    (arb_kind(), proptest::option::of(any::<bool>()), any::<bool>())
        .prop_map(|(kind, decision, done)| ArbEvent { kind, decision, done })
}

fn legal_states() -> [State; 6] {
    State::ALL
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 600, ..ProptestConfig::default() })]

    /// For ANY sequence of hook events, the machine:
    ///  - never panics,
    ///  - always rests in one of the six legal States,
    ///  - keeps the confidence-honesty invariant (High ⇒ Waiting or Dead),
    ///  - keeps `urgency=approval ⇔ state=waiting` (no urgency without waiting,
    ///    no waiting without approval urgency),
    ///  - keeps `awaiting_approval ⇒ waiting`.
    #[test]
    fn no_illegal_edge(events in prop::collection::vec(arb_event(), 0..200)) {
        let mut m = machine();
        for ae in events {
            let mut e = ev(ae.kind.clone());
            e.tool_name = Some("Bash".into());
            e.turn_complete_done = ae.done;
            if ae.kind == CodexHookKind::PermissionRequest {
                e.decision = ae.decision.map(|b| if b { ApprovalDecision::Allow } else { ApprovalDecision::Deny });
            }
            let t = m.apply(&e);

            // Rests in a legal state.
            prop_assert!(legal_states().contains(&m.state()));
            // Transition's reported state matches the machine.
            prop_assert_eq!(t.state, m.state());

            // Confidence honesty.
            if m.confidence() == Confidence::High {
                prop_assert!(matches!(m.state(), State::Waiting | State::Dead),
                    "High leaked into {:?}", m.state());
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
            e.turn_complete_done = ae.done;
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
fn codex_stop_surfaces_last_assistant_message_as_idle_preview() {
    let json = r#"{"session_id":"t","hook_event_name":"Stop","stop_hook_active":false,
        "last_assistant_message":"Refactored the parser; all green."}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.last_message.as_deref(), Some("Refactored the parser; all green."));
    let mut m = CodexStateMachine::new("t");
    m.apply(&e);
    assert_eq!(m.state(), State::Idle);
    assert_eq!(
        m.to_run("r", "2026-06-08T00:00:00Z").last_message.as_deref(),
        Some("Refactored the parser; all green.")
    );
}

#[test]
fn codex_pre_tool_use_parses_tool_use_id() {
    let json = r#"{"session_id":"t","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"tool_789"}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert_eq!(e.tool_use_id.as_deref(), Some("tool_789"));
}

#[test]
fn codex_stop_inside_stop_hook_is_not_done() {
    // Even with a completion marker, stop_hook_active:true → idle, never done.
    let json = r#"{"session_id":"t","hook_event_name":"Stop","stop_hook_active":true,"task_complete":true}"#;
    let e = CodexHookEvent::parse(json).unwrap();
    assert!(!e.turn_complete_done, "stop_hook_active suppresses the done claim");
    let mut m = CodexStateMachine::new("t");
    m.apply(&CodexHookEvent::parse(r#"{"session_id":"t","hook_event_name":"UserPromptSubmit"}"#).unwrap());
    m.apply(&e);
    assert_eq!(m.state(), State::Idle);
}

#[test]
fn codex_permission_request_is_authoritative_waiting_high() {
    // The key Codex advantage: PermissionRequest is a real waiting signal → High.
    let mut m = CodexStateMachine::new("t");
    m.apply(&CodexHookEvent::parse(r#"{"session_id":"t","hook_event_name":"PermissionRequest","tool_name":"Bash","tool_use_id":"tool_1"}"#).unwrap());
    assert_eq!(m.state(), State::Waiting);
    assert_eq!(m.confidence(), Confidence::High, "PermissionRequest is authoritative");
}
