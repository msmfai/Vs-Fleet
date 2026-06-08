//! Integration tests: the Codex detection adapter vs **recorded hook-event JSON
//! fixtures** on disk (PLAN S11–S13 / node CODEX; gate G2 "each adapter vs
//! recorded fixtures").
//!
//! These do NOT require a real codex/VS Code install — the fixtures under
//! `tests/fixtures/codex/` are recorded Codex hook payloads (shapes validated
//! against the cmux `main` Codex-hook regression suite). We replay them through
//! the public adapter API and assert the resulting state machine + reporter
//! commands.

use std::path::PathBuf;

use fleet_protocol::{Confidence, State, Urgency};
use fleet_reporter::codex::{
    CodexAdapter, CodexHookEvent, CodexHookKind, CodexParseError, CodexStateMachine,
};
use fleet_reporter::reporter::ReporterCommand;

fn fixture(name: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/codex");
    p.push(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"))
}

fn parse(name: &str) -> CodexHookEvent {
    CodexHookEvent::parse(&fixture(name)).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

// ── each fixture parses to the right kind / fields ───────────────────────────

#[test]
fn fixtures_parse_to_expected_kinds() {
    assert_eq!(
        parse("session_start.json").kind,
        CodexHookKind::SessionStart
    );
    assert_eq!(
        parse("user_prompt_submit.json").kind,
        CodexHookKind::UserPromptSubmit
    );
    assert_eq!(parse("pre_tool_use.json").kind, CodexHookKind::PreToolUse);
    assert_eq!(parse("post_tool_use.json").kind, CodexHookKind::PostToolUse);
    assert_eq!(
        parse("permission_request.json").kind,
        CodexHookKind::PermissionRequest
    );
    assert_eq!(parse("stop_idle.json").kind, CodexHookKind::Stop);
    assert_eq!(parse("stop_done.json").kind, CodexHookKind::Stop);
    assert_eq!(parse("session_end.json").kind, CodexHookKind::SessionEnd);
}

#[test]
fn permission_request_fixture_carries_thread_and_tool() {
    let e = parse("permission_request.json");
    assert_eq!(e.thread_id, "0199f3aa-thread-codex-1");
    assert_eq!(e.tool_name.as_deref(), Some("Bash"));
    assert!(e.decision.is_none(), "a fresh request has no decision");
    assert!(!e.is_approval_response());
}

#[test]
fn permission_response_fixtures_carry_decisions() {
    assert!(parse("permission_response_allow.json").is_approval_response());
    assert!(parse("permission_response_deny.json").is_approval_response());
}

#[test]
fn camelcase_fixture_parses() {
    let e = parse("camelcase_pre_tool_use.json");
    assert_eq!(e.kind, CodexHookKind::PreToolUse);
    assert_eq!(e.thread_id, "0199f3aa-thread-codex-1");
    assert_eq!(e.tool_name.as_deref(), Some("ApplyPatch"));
}

#[test]
fn stop_done_fixture_marks_completion() {
    assert!(parse("stop_done.json").turn_complete_done);
    assert!(!parse("stop_idle.json").turn_complete_done);
}

// ── drift / error fixtures degrade gracefully ────────────────────────────────

#[test]
fn unknown_hook_fixture_is_other_not_error() {
    let e = parse("unknown_hook.json");
    assert!(matches!(e.kind, CodexHookKind::Other(_)));
}

#[test]
fn malformed_fixture_errors_cleanly() {
    let err = CodexHookEvent::parse(&fixture("malformed.json")).unwrap_err();
    assert!(matches!(err, CodexParseError::InvalidJson(_)));
}

#[test]
fn missing_session_id_fixture_errors() {
    let err = CodexHookEvent::parse(&fixture("missing_session_id.json")).unwrap_err();
    assert_eq!(err, CodexParseError::MissingThreadId);
}

// ── end-to-end replay through the adapter (S11 working/idle/done) ────────────

#[test]
fn replay_working_idle_done_from_fixtures() {
    let mut a = CodexAdapter::new();

    a.ingest_json(&fixture("session_start.json"));
    assert_eq!(a.state_of("0199f3aa-thread-codex-1"), Some(State::Idle));

    a.ingest_json(&fixture("user_prompt_submit.json"));
    assert_eq!(a.state_of("0199f3aa-thread-codex-1"), Some(State::Working));

    a.ingest_json(&fixture("pre_tool_use.json"));
    assert_eq!(a.state_of("0199f3aa-thread-codex-1"), Some(State::Working));

    a.ingest_json(&fixture("stop_idle.json"));
    assert_eq!(a.state_of("0199f3aa-thread-codex-1"), Some(State::Idle));

    // A fresh prompt then a completion-marked Stop → done (distinct from idle).
    a.ingest_json(&fixture("user_prompt_submit.json"));
    a.ingest_json(&fixture("stop_done.json"));
    assert_eq!(a.state_of("0199f3aa-thread-codex-1"), Some(State::Done));
}

// ── S12: approval shows waiting+approval, high ──────────────────────────────

#[test]
fn replay_approval_is_waiting_high() {
    let mut a = CodexAdapter::new();
    a.ingest_json(&fixture("user_prompt_submit.json"));
    let cmds = a.ingest_json(&fixture("permission_request.json"));
    assert_eq!(cmds.len(), 1);
    match &cmds[0] {
        ReporterCommand::UpsertRun(run) => {
            assert_eq!(run.state, State::Waiting);
            assert_eq!(run.urgency, Some(Urgency::Approval));
            assert_eq!(run.confidence, Confidence::High);
            assert!(run.waiting_since.is_some());
        }
        other => panic!("expected UpsertRun(waiting), got {other:?}"),
    }
}

// ── S13: auto-resolve — answer in terminal clears waiting ───────────────────

#[test]
fn replay_auto_resolve_via_decision_fixture() {
    let mut a = CodexAdapter::new();
    a.ingest_json(&fixture("permission_request.json"));
    assert_eq!(a.state_of("0199f3aa-thread-codex-1"), Some(State::Waiting));
    a.ingest_json(&fixture("permission_response_allow.json"));
    assert_eq!(a.state_of("0199f3aa-thread-codex-1"), Some(State::Working));
}

#[test]
fn replay_auto_resolve_via_activity_fixture() {
    // No explicit decision event: the user answers, Codex resumes and emits
    // PreToolUse, which clears the gate (S13 "no Fleet interaction").
    let mut a = CodexAdapter::new();
    a.ingest_json(&fixture("permission_request.json"));
    assert_eq!(a.state_of("0199f3aa-thread-codex-1"), Some(State::Waiting));
    a.ingest_json(&fixture("pre_tool_use.json"));
    assert_eq!(a.state_of("0199f3aa-thread-codex-1"), Some(State::Working));
}

#[test]
fn replay_deny_also_resolves() {
    let mut a = CodexAdapter::new();
    a.ingest_json(&fixture("permission_request.json"));
    a.ingest_json(&fixture("permission_response_deny.json"));
    assert_eq!(a.state_of("0199f3aa-thread-codex-1"), Some(State::Working));
}

// ── SessionEnd → dead ────────────────────────────────────────────────────────

#[test]
fn replay_session_end_is_dead() {
    let mut a = CodexAdapter::new();
    a.ingest_json(&fixture("user_prompt_submit.json"));
    let cmds = a.ingest_json(&fixture("session_end.json"));
    assert_eq!(a.state_of("0199f3aa-thread-codex-1"), Some(State::Dead));
    match &cmds[0] {
        ReporterCommand::UpsertRun(run) => {
            assert_eq!(run.state, State::Dead);
            assert_eq!(run.confidence, Confidence::High);
        }
        other => panic!("expected dead upsert, got {other:?}"),
    }
}

// ── a bad fixture line never creates a ghost run ─────────────────────────────

#[test]
fn malformed_line_creates_no_ghost() {
    let mut a = CodexAdapter::new();
    let cmds = a.ingest_json(&fixture("malformed.json"));
    assert!(cmds.is_empty());
    assert_eq!(a.thread_count(), 0);
}

// ── full ordered transcript: each transition observed exactly once ───────────

#[test]
fn full_transcript_transition_sequence() {
    let mut a = CodexAdapter::new();
    let tid = "0199f3aa-thread-codex-1";
    let script = [
        ("session_start.json", State::Idle),
        ("user_prompt_submit.json", State::Working),
        ("pre_tool_use.json", State::Working),
        ("permission_request.json", State::Waiting),
        ("permission_response_allow.json", State::Working),
        ("pre_tool_use.json", State::Working),
        ("stop_idle.json", State::Idle),
        ("session_end.json", State::Dead),
    ];
    for (file, expect) in script {
        a.ingest_json(&fixture(file));
        assert_eq!(a.state_of(tid), Some(expect), "after {file}");
    }
}

// ── real-fidelity fields (captured shape per OpenAI Codex hooks docs) ────────

#[test]
fn stop_idle_fixture_carries_last_assistant_message_preview() {
    let e = parse("stop_idle.json");
    assert_eq!(e.kind, CodexHookKind::Stop);
    assert_eq!(
        e.last_message.as_deref(),
        Some("Refactored the parser; all tests pass.")
    );
    let mut m = CodexStateMachine::new(&e.thread_id);
    m.apply(&e);
    assert_eq!(
        m.to_run("r", "2026-06-08T00:00:00Z")
            .last_message
            .as_deref(),
        Some("Refactored the parser; all tests pass.")
    );
}

#[test]
fn pre_tool_use_fixture_carries_tool_use_id() {
    assert_eq!(
        parse("pre_tool_use.json").tool_use_id.as_deref(),
        Some("tool_789")
    );
}
