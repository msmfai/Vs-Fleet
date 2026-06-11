// Inline unit tests for the Claude detection adapter.
//
// Included from `claude.rs` via `include!`. Covers: hook-event parsing (incl.
// camelCase aliases + schema-drift tolerance), every modelled transition,
// done-from-Stop (NEVER from PostToolUse, #31285), the SubagentStop carve-out,
// confidence honesty (structural — no Waiting/High heuristic path), the durable
// session-id anchor, and a property test proving the state machine has NO illegal
// edge and never overstates.

use super::*;
use fleet_protocol::{AgentKind, Confidence, State};
use proptest::prelude::*;

const SESSION: &str = "0199f3aa-claude-session-1";

fn ev(kind: ClaudeHookKind) -> ClaudeHookEvent {
    ClaudeHookEvent {
        kind,
        session_id: SESSION.to_string(),
        cwd: Some("/work".into()),
        tool_name: None,
        stop_hook_active: false,
        turn_complete_done: false,
        last_message: None,
        tool_use_id: None,
    }
}

fn tool_ev(kind: ClaudeHookKind, tool: &str) -> ClaudeHookEvent {
    let mut e = ev(kind);
    e.tool_name = Some(tool.to_string());
    e
}

fn machine() -> ClaudeStateMachine {
    ClaudeStateMachine::new(SESSION)
}

// ── parsing: each recorded field shape ───────────────────────────────────────

#[test]
fn parses_snake_case_pre_tool_use() {
    let json = r#"{"session_id":"s1","cwd":"/p","hook_event_name":"PreToolUse","tool_name":"Bash"}"#;
    let e = ClaudeHookEvent::parse(json).unwrap();
    assert_eq!(e.kind, ClaudeHookKind::PreToolUse);
    assert_eq!(e.session_id, "s1");
    assert_eq!(e.cwd.as_deref(), Some("/p"));
    assert_eq!(e.tool_name.as_deref(), Some("Bash"));
}

#[test]
fn parses_camel_case_aliases() {
    let json = r#"{"sessionId":"s2","hookEventName":"UserPromptSubmit"}"#;
    let e = ClaudeHookEvent::parse(json).unwrap();
    assert_eq!(e.kind, ClaudeHookKind::UserPromptSubmit);
    assert_eq!(e.session_id, "s2");
}

#[test]
fn parses_session_start() {
    let json = r#"{"session_id":"s3","hook_event_name":"SessionStart","source":"resume"}"#;
    let e = ClaudeHookEvent::parse(json).unwrap();
    assert_eq!(e.session_id, "s3");
    assert_eq!(e.kind, ClaudeHookKind::SessionStart);
}

#[test]
fn stop_hook_active_is_parsed() {
    let json = r#"{"session_id":"s","hook_event_name":"Stop","stop_hook_active":true}"#;
    assert!(ClaudeHookEvent::parse(json).unwrap().stop_hook_active);
    let json2 = r#"{"session_id":"s","hook_event_name":"Stop"}"#;
    assert!(!ClaudeHookEvent::parse(json2).unwrap().stop_hook_active);
}

#[test]
fn stop_done_markers_parsed() {
    for json in [
        r#"{"session_id":"s","hook_event_name":"Stop","task_complete":true}"#,
        r#"{"session_id":"s","hook_event_name":"Stop","reason":"completed"}"#,
        r#"{"session_id":"s","hook_event_name":"Stop","subtype":"success"}"#,
    ] {
        assert!(
            ClaudeHookEvent::parse(json).unwrap().turn_complete_done,
            "should mark completion: {json}"
        );
    }
    let bare = r#"{"session_id":"s","hook_event_name":"Stop"}"#;
    assert!(!ClaudeHookEvent::parse(bare).unwrap().turn_complete_done);
}

// ── parsing: error / drift handling (never panics, never overstates) ─────────

#[test]
fn unknown_hook_name_is_other_not_error() {
    let json = r#"{"session_id":"s","hook_event_name":"Notification","message":"x"}"#;
    let e = ClaudeHookEvent::parse(json).unwrap();
    assert_eq!(e.kind, ClaudeHookKind::Other("Notification".into()));
}

#[test]
fn malformed_json_is_error_not_panic() {
    let json = r#"{"session_id":"s","hook_event_name":"PreToolUse""#; // truncated
    assert!(matches!(
        ClaudeHookEvent::parse(json),
        Err(ClaudeParseError::InvalidJson(_))
    ));
}

#[test]
fn missing_event_name_is_error() {
    let json = r#"{"session_id":"s"}"#;
    assert_eq!(
        ClaudeHookEvent::parse(json),
        Err(ClaudeParseError::MissingEventName)
    );
}

#[test]
fn missing_session_id_is_error() {
    // Identity honesty: an un-anchored hook cannot become a durable run.
    let json = r#"{"hook_event_name":"PreToolUse"}"#;
    assert_eq!(
        ClaudeHookEvent::parse(json),
        Err(ClaudeParseError::MissingSessionId)
    );
}

#[test]
fn empty_session_id_is_error() {
    let json = r#"{"session_id":"","hook_event_name":"PreToolUse"}"#;
    assert_eq!(
        ClaudeHookEvent::parse(json),
        Err(ClaudeParseError::MissingSessionId)
    );
}

#[test]
fn unknown_payload_fields_are_ignored_not_fatal() {
    let json = r#"{"session_id":"s","hook_event_name":"PreToolUse","transcript_path":"/x.jsonl","permission_mode":"default","new_field":{"a":1}}"#;
    let e = ClaudeHookEvent::parse(json).unwrap();
    assert_eq!(e.kind, ClaudeHookKind::PreToolUse);
}

// ── transitions: SessionStart → idle ─────────────────────────────────────────

#[test]
fn new_machine_starts_idle_inferred() {
    let m = machine();
    assert_eq!(m.state(), State::Idle);
    assert_eq!(m.confidence(), Confidence::Inferred);
}

#[test]
fn session_start_on_live_session_is_noop() {
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit)); // working
    let t = m.apply(&ev(ClaudeHookKind::SessionStart));
    assert_eq!(
        m.state(),
        State::Working,
        "SessionStart must not reset a live session"
    );
    assert!(!t.changed);
}

#[test]
fn session_start_revives_dead_session() {
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::SessionEnd)); // dead
    assert_eq!(m.state(), State::Dead);
    let t = m.apply(&ev(ClaudeHookKind::SessionStart));
    assert_eq!(
        m.state(),
        State::Idle,
        "resume/continue revives a dead session to idle"
    );
    assert!(t.changed);
}

// ── transitions: activity → working ──────────────────────────────────────────

#[test]
fn user_prompt_submit_goes_working() {
    let mut m = machine();
    let t = m.apply(&ev(ClaudeHookKind::UserPromptSubmit));
    assert_eq!(m.state(), State::Working);
    assert_eq!(m.confidence(), Confidence::Inferred, "working is never High");
    assert!(t.changed);
    assert!(t.liveness);
}

#[test]
fn pre_tool_use_goes_working() {
    let mut m = machine();
    let t = m.apply(&tool_ev(ClaudeHookKind::PreToolUse, "Bash"));
    assert_eq!(m.state(), State::Working);
    assert_eq!(m.confidence(), Confidence::Inferred);
    assert!(t.liveness);
}

#[test]
fn repeated_pre_tool_use_stays_working_noop() {
    let mut m = machine();
    m.apply(&tool_ev(ClaudeHookKind::PreToolUse, "Bash"));
    let t = m.apply(&tool_ev(ClaudeHookKind::PreToolUse, "Read"));
    assert_eq!(m.state(), State::Working);
    assert!(!t.changed, "working→working is a no-op (idempotent)");
    assert!(t.liveness, "but still a liveness signal");
}

// ── PostToolUse is liveness-only and NEVER drives done (#31285) ──────────────

#[test]
fn post_tool_use_is_liveness_only_never_done() {
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit)); // working
    let t = m.apply(&tool_ev(ClaudeHookKind::PostToolUse, "Bash"));
    assert_eq!(
        m.state(),
        State::Working,
        "PostToolUse never flips state (does not fire in native UI, #31285)"
    );
    assert!(!t.changed);
    assert!(t.liveness);
}

#[test]
fn post_tool_use_does_not_make_a_run_done() {
    // The whole point of S15: `done` is derived from Stop, never PostToolUse.
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit));
    let mut e = tool_ev(ClaudeHookKind::PostToolUse, "Bash");
    e.turn_complete_done = true; // even if a PostToolUse claimed completion…
    m.apply(&e);
    assert_ne!(m.state(), State::Done, "PostToolUse must never reach Done");
    assert_eq!(m.state(), State::Working);
}

#[test]
fn subagent_stop_is_liveness_only() {
    // A subagent finishing must not end the main run.
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit)); // working
    let t = m.apply(&ev(ClaudeHookKind::SubagentStop));
    assert_eq!(m.state(), State::Working, "SubagentStop does not end the run");
    assert!(!t.changed);
    assert!(t.liveness);
}

#[test]
fn compaction_is_liveness_only() {
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit));
    let t = m.apply(&ev(ClaudeHookKind::PreCompact));
    assert_eq!(m.state(), State::Working);
    assert!(!t.changed);
    assert!(t.liveness);
}

// ── transitions: Stop → idle / done (the completion signal) ──────────────────

#[test]
fn stop_goes_idle() {
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit)); // working
    let t = m.apply(&ev(ClaudeHookKind::Stop));
    assert_eq!(m.state(), State::Idle);
    assert_eq!(m.confidence(), Confidence::Inferred);
    assert!(t.changed);
}

#[test]
fn stop_with_completion_goes_done() {
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit));
    let mut stop = ev(ClaudeHookKind::Stop);
    stop.turn_complete_done = true;
    let t = m.apply(&stop);
    assert_eq!(
        m.state(),
        State::Done,
        "completion marker → done (D9 distinct)"
    );
    assert!(t.changed);
}

#[test]
fn stop_from_within_stop_hook_stays_idle_not_done() {
    // stop_hook_active=true means we are inside a Stop hook's own continuation —
    // NOT a real task end, so we must not claim `done`.
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit));
    let mut stop = ev(ClaudeHookKind::Stop);
    stop.turn_complete_done = true;
    stop.stop_hook_active = true;
    m.apply(&stop);
    assert_eq!(
        m.state(),
        State::Idle,
        "stop_hook_active suppresses the done claim (conservative)"
    );
}

#[test]
fn done_and_idle_are_distinct() {
    // D9: done must never collapse into idle.
    let mut a = machine();
    a.apply(&ev(ClaudeHookKind::UserPromptSubmit));
    a.apply(&ev(ClaudeHookKind::Stop));
    let mut b = machine();
    b.apply(&ev(ClaudeHookKind::UserPromptSubmit));
    let mut stop = ev(ClaudeHookKind::Stop);
    stop.turn_complete_done = true;
    b.apply(&stop);
    assert_eq!(a.state(), State::Idle);
    assert_eq!(b.state(), State::Done);
    assert_ne!(a.state(), b.state());
}

// ── transitions: SessionEnd → dead ───────────────────────────────────────────

#[test]
fn session_end_goes_dead_high() {
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit));
    let t = m.apply(&ev(ClaudeHookKind::SessionEnd));
    assert_eq!(m.state(), State::Dead);
    assert_eq!(
        m.confidence(),
        Confidence::High,
        "confirmed exit is authoritative"
    );
    assert!(t.changed);
}

// ── thread/session-id routing guard ──────────────────────────────────────────

#[test]
fn foreign_session_event_is_ignored() {
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit)); // working
    let mut foreign = ev(ClaudeHookKind::Stop);
    foreign.session_id = "some-other-session".into();
    let t = m.apply(&foreign);
    assert_eq!(
        m.state(),
        State::Working,
        "foreign session must not mutate us"
    );
    assert!(!t.changed);
}

// ── cwd propagation ──────────────────────────────────────────────────────────

#[test]
fn cwd_is_captured_from_hooks() {
    let mut m = machine();
    let mut e = ev(ClaudeHookKind::UserPromptSubmit);
    e.cwd = Some("/Users/dev/repo".into());
    m.apply(&e);
    assert_eq!(m.cwd(), "/Users/dev/repo");
    assert_eq!(m.to_run("r", "ts").cwd, "/Users/dev/repo");
}

// ── run snapshot shape ───────────────────────────────────────────────────────

#[test]
fn run_snapshot_is_claude_anchored_and_never_waiting() {
    let mut m = machine();
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit));
    let run = m.to_run("claude:run-1", "2026-06-08T00:00:00Z");
    assert_eq!(run.agent_kind, AgentKind::ClaudeCode);
    assert_eq!(run.native_id, SESSION, "durable anchor = session_id");
    assert_eq!(run.state, State::Working);
    assert!(run.urgency.is_none(), "S15 never produces urgency");
    assert!(run.waiting_since.is_none(), "S15 never produces waiting");
}

// ── full lifecycle sequence (S15 end to end) ─────────────────────────────────

#[test]
fn full_lifecycle_idle_working_idle_done_dead() {
    let mut m = machine();
    let seq = [
        (ClaudeHookKind::SessionStart, State::Idle),
        (ClaudeHookKind::UserPromptSubmit, State::Working),
        (ClaudeHookKind::PreToolUse, State::Working),
        (ClaudeHookKind::PostToolUse, State::Working), // liveness only
        (ClaudeHookKind::Stop, State::Idle),
    ];
    for (k, expect) in seq {
        let e = tool_ev(k, "Bash");
        m.apply(&e);
        assert_eq!(m.state(), expect);
    }
    // a fresh prompt then a completion-marked Stop → done
    m.apply(&ev(ClaudeHookKind::UserPromptSubmit));
    let mut stop = ev(ClaudeHookKind::Stop);
    stop.turn_complete_done = true;
    m.apply(&stop);
    assert_eq!(m.state(), State::Done);
    m.apply(&ev(ClaudeHookKind::SessionEnd));
    assert_eq!(m.state(), State::Dead);
}

// ── adapter: hook stream → ReporterCommands ──────────────────────────────────

#[test]
fn adapter_mints_run_and_emits_upsert_on_first_event() {
    let mut a = ClaudeAdapter::new();
    let (cmds, t) = a.ingest(&ev(ClaudeHookKind::UserPromptSubmit));
    assert_eq!(a.session_count(), 1);
    assert!(t.changed);
    assert_eq!(cmds.len(), 1);
    match &cmds[0] {
        ReporterCommand::UpsertRun(run) => {
            assert_eq!(run.state, State::Working);
            assert_eq!(run.native_id, SESSION);
            assert_eq!(run.agent_kind, AgentKind::ClaudeCode);
        }
        other => panic!("expected UpsertRun, got {other:?}"),
    }
}

#[test]
fn adapter_run_id_is_stable_per_session() {
    let mut a = ClaudeAdapter::new();
    a.ingest(&ev(ClaudeHookKind::UserPromptSubmit));
    let id1 = a.run_id_of(SESSION).unwrap().to_string();
    a.ingest(&tool_ev(ClaudeHookKind::PreToolUse, "Bash"));
    let id2 = a.run_id_of(SESSION).unwrap().to_string();
    assert_eq!(id1, id2, "same session keeps its Fleet run-id");
}

#[test]
fn adapter_liveness_only_for_telemetry_hook() {
    let mut a = ClaudeAdapter::new();
    a.ingest(&ev(ClaudeHookKind::UserPromptSubmit)); // upsert
    let (cmds, _) = a.ingest(&tool_ev(ClaudeHookKind::PostToolUse, "Bash"));
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], ReporterCommand::Liveness { .. }));
}

#[test]
fn adapter_noop_emits_only_liveness() {
    let mut a = ClaudeAdapter::new();
    a.ingest(&ev(ClaudeHookKind::UserPromptSubmit)); // working
    let (cmds, _) = a.ingest(&ev(ClaudeHookKind::UserPromptSubmit)); // working again
    assert_eq!(cmds.len(), 1);
    assert!(matches!(cmds[0], ReporterCommand::Liveness { .. }));
}

#[test]
fn adapter_multiplexes_sessions() {
    let mut a = ClaudeAdapter::new();
    let mut e1 = ev(ClaudeHookKind::Stop);
    e1.session_id = "session-A".into();
    let mut e2 = ev(ClaudeHookKind::UserPromptSubmit);
    e2.session_id = "session-B".into();
    a.ingest(&e1);
    a.ingest(&e2);
    assert_eq!(a.session_count(), 2);
    assert_eq!(a.state_of("session-A"), Some(State::Idle));
    assert_eq!(a.state_of("session-B"), Some(State::Working));
    assert_ne!(a.run_id_of("session-A"), a.run_id_of("session-B"));
}

#[test]
fn adapter_ingest_json_parse_error_yields_no_commands_no_ghost() {
    let mut a = ClaudeAdapter::new();
    let cmds = a.ingest_json("{ not json");
    assert!(cmds.is_empty());
    assert_eq!(a.session_count(), 0, "a bad line must not create a ghost run");
}

#[test]
fn adapter_ingest_json_working_idle_done_cycle() {
    let mut a = ClaudeAdapter::new();
    a.ingest_json(r#"{"session_id":"sX","hook_event_name":"UserPromptSubmit"}"#);
    assert_eq!(a.state_of("sX"), Some(State::Working));
    a.ingest_json(r#"{"session_id":"sX","hook_event_name":"Stop"}"#);
    assert_eq!(a.state_of("sX"), Some(State::Idle));
    a.ingest_json(r#"{"session_id":"sX","hook_event_name":"UserPromptSubmit"}"#);
    a.ingest_json(r#"{"session_id":"sX","hook_event_name":"Stop","reason":"completed"}"#);
    assert_eq!(a.state_of("sX"), Some(State::Done));
}

#[test]
fn adapter_session_end_emits_dead_high() {
    let mut a = ClaudeAdapter::new();
    a.ingest_json(r#"{"session_id":"sX","hook_event_name":"UserPromptSubmit"}"#);
    let cmds = a.ingest_json(r#"{"session_id":"sX","hook_event_name":"SessionEnd"}"#);
    assert_eq!(a.state_of("sX"), Some(State::Dead));
    match &cmds[0] {
        ReporterCommand::UpsertRun(run) => {
            assert_eq!(run.state, State::Dead);
            assert_eq!(run.confidence, Confidence::High);
        }
        other => panic!("expected dead upsert, got {other:?}"),
    }
}

#[test]
fn adapter_forget_lets_a_fresh_run_start() {
    let mut a = ClaudeAdapter::new();
    a.ingest(&ev(ClaudeHookKind::UserPromptSubmit));
    let id1 = a.run_id_of(SESSION).unwrap().to_string();
    assert!(a.forget(SESSION));
    a.ingest(&ev(ClaudeHookKind::UserPromptSubmit));
    let id2 = a.run_id_of(SESSION).unwrap().to_string();
    assert_ne!(id1, id2, "a forgotten session mints a fresh run-id");
}

// ── CONFIDENCE HONESTY INVARIANT (G2) — STRUCTURAL for S15 ───────────────────

#[test]
fn s15_never_emits_waiting_or_heuristic_high() {
    // S15 has no waiting/approval path at all (that's S16/S17). Drive every event
    // kind from a fresh machine and assert: never Waiting, and High only on Dead.
    for kind in all_kinds() {
        let mut m = machine();
        let mut e = ev(kind.clone());
        e.tool_name = Some("Bash".into());
        e.turn_complete_done = true; // try to provoke an over-claim
        m.apply(&e);
        assert_ne!(m.state(), State::Waiting, "S15 must never enter Waiting ({kind:?})");
        if m.confidence() == Confidence::High {
            assert_eq!(
                m.state(),
                State::Dead,
                "High confidence only from a confirmed SessionEnd exit ({kind:?})"
            );
        }
    }
}

fn all_kinds() -> Vec<ClaudeHookKind> {
    vec![
        ClaudeHookKind::SessionStart,
        ClaudeHookKind::UserPromptSubmit,
        ClaudeHookKind::PreToolUse,
        ClaudeHookKind::PostToolUse,
        ClaudeHookKind::Stop,
        ClaudeHookKind::SubagentStop,
        ClaudeHookKind::SessionEnd,
        ClaudeHookKind::PreCompact,
        ClaudeHookKind::Other("Notification".into()),
    ]
}

// ── PROPERTY: NO ILLEGAL EDGE / NO OVERSTATEMENT (G2) ────────────────────────

fn arb_kind() -> impl Strategy<Value = ClaudeHookKind> {
    prop_oneof![
        Just(ClaudeHookKind::SessionStart),
        Just(ClaudeHookKind::UserPromptSubmit),
        Just(ClaudeHookKind::PreToolUse),
        Just(ClaudeHookKind::PostToolUse),
        Just(ClaudeHookKind::Stop),
        Just(ClaudeHookKind::SubagentStop),
        Just(ClaudeHookKind::SessionEnd),
        Just(ClaudeHookKind::PreCompact),
    ]
}

#[derive(Debug, Clone)]
struct ArbEvent {
    kind: ClaudeHookKind,
    done: bool,
    stop_hook_active: bool,
}

fn arb_event() -> impl Strategy<Value = ArbEvent> {
    (arb_kind(), any::<bool>(), any::<bool>()).prop_map(|(kind, done, stop_hook_active)| ArbEvent {
        kind,
        done,
        stop_hook_active,
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 600, ..ProptestConfig::default() })]

    /// For ANY sequence of hook events, the S15 machine:
    ///  - never panics,
    ///  - always rests in one of the six legal States,
    ///  - NEVER enters Waiting (no S15 approval path) and has no urgency,
    ///  - keeps confidence honesty: High ⇒ Dead (confirmed exit) ONLY,
    ///  - never reaches Done via a PostToolUse (only Stop can),
    ///  - keeps `to_run` internally consistent.
    #[test]
    fn no_illegal_edge_no_overstatement(events in prop::collection::vec(arb_event(), 0..200)) {
        let mut m = machine();
        for ae in events {
            let mut e = ev(ae.kind.clone());
            e.tool_name = Some("Bash".into());
            e.turn_complete_done = ae.done;
            e.stop_hook_active = ae.stop_hook_active;
            let was_state = m.state();
            let t = m.apply(&e);

            // Rests in a legal state; transition agrees with the machine.
            prop_assert!(State::ALL.contains(&m.state()));
            prop_assert_eq!(t.state, m.state());

            // S15 NEVER produces Waiting.
            prop_assert_ne!(m.state(), State::Waiting);

            // Confidence honesty: High only on Dead.
            if m.confidence() == Confidence::High {
                prop_assert_eq!(m.state(), State::Dead, "High leaked into {:?}", m.state());
            }

            // PostToolUse / SubagentStop / PreCompact never change state.
            if matches!(ae.kind, ClaudeHookKind::PostToolUse
                | ClaudeHookKind::SubagentStop
                | ClaudeHookKind::PreCompact)
            {
                prop_assert_eq!(m.state(), was_state, "{:?} must not flip state", ae.kind);
                prop_assert!(!t.changed);
            }

            // Done is *entered* ONLY via a Stop (with a completion marker and not
            // a stop-hook continuation). A run already Done that sees a no-op event
            // stays Done — what we forbid is *transitioning into* Done by anything
            // other than Stop.
            if m.state() == State::Done && was_state != State::Done {
                prop_assert_eq!(&ae.kind, &ClaudeHookKind::Stop, "Done only entered from Stop");
            }
        }

        // `to_run` is always internally consistent and never claims waiting.
        let run = m.to_run("r", "2026-06-08T00:00:00Z");
        prop_assert_eq!(run.state, m.state());
        prop_assert_eq!(run.confidence, m.confidence());
        prop_assert_eq!(run.native_id, m.session_id());
        prop_assert!(run.urgency.is_none());
        prop_assert!(run.waiting_since.is_none());
        prop_assert_eq!(run.agent_kind, AgentKind::ClaudeCode);
    }
}

// ── last_assistant_message → inbox preview (real Stop carries it) ─────────────

#[test]
fn stop_surfaces_last_assistant_message_as_idle_preview() {
    // A real Stop payload carries `last_assistant_message`; the run's preview
    // should be that text (not None / not a generic line).
    let json = r#"{"session_id":"s","hook_event_name":"Stop","stop_hook_active":false,
        "last_assistant_message":"The current model is Claude Opus 4.8 (1M context)."}"#;
    let e = ClaudeHookEvent::parse(json).unwrap();
    assert_eq!(e.last_message.as_deref(), Some("The current model is Claude Opus 4.8 (1M context)."));

    let mut m = ClaudeStateMachine::new("s");
    m.apply(&e);
    assert_eq!(m.state(), State::Idle);
    let run = m.to_run("r", "2026-06-08T00:00:00Z");
    assert_eq!(
        run.last_message.as_deref(),
        Some("The current model is Claude Opus 4.8 (1M context).")
    );
}

#[test]
fn idle_preview_is_none_without_a_message() {
    // A bare Stop (no last_assistant_message) → idle with no preview (unchanged).
    let mut m = ClaudeStateMachine::new("s");
    m.apply(&ClaudeHookEvent::parse(r#"{"session_id":"s","hook_event_name":"Stop"}"#).unwrap());
    assert_eq!(m.state(), State::Idle);
    assert!(m.to_run("r", "t").last_message.is_none());
}

#[test]
fn preview_truncates_a_long_multiline_message_to_one_line() {
    let long = "line one\nline two ".to_string() + &"x".repeat(200);
    let json = format!(
        r#"{{"session_id":"s","hook_event_name":"Stop","stop_hook_active":false,"last_assistant_message":{}}}"#,
        serde_json::to_string(&long).unwrap()
    );
    let mut m = ClaudeStateMachine::new("s");
    m.apply(&ClaudeHookEvent::parse(&json).unwrap());
    let msg = m.to_run("r", "t").last_message.unwrap();
    assert!(!msg.contains('\n'), "preview is single-line");
    assert!(msg.ends_with('…'), "long preview is truncated");
    assert!(msg.chars().count() <= 101, "preview bounded (~100 + ellipsis)");
}

#[test]
fn pre_tool_use_parses_tool_use_id() {
    // Real PreToolUse carries `tool_use_id` (a toolu_… correlation anchor).
    let json = r#"{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Read",
        "tool_use_id":"toolu_016FQ3SN7uLEwwQEkQxU2nMA"}"#;
    let e = ClaudeHookEvent::parse(json).unwrap();
    assert_eq!(e.tool_use_id.as_deref(), Some("toolu_016FQ3SN7uLEwwQEkQxU2nMA"));
}
