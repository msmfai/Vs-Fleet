//! Integration tests: the Claude detection adapter vs **recorded hook-event JSON
//! fixtures** on disk (PLAN S15 / node CLHOOK; gate G2 "each adapter vs recorded
//! fixtures").
//!
//! These do NOT require a real claude/VS Code install — the fixtures under
//! `tests/fixtures/claude/` are recorded Claude Code hook payloads (shapes mirror
//! the documented Claude hook stdin: `hook_event_name`, `session_id`,
//! `transcript_path`, `cwd`, `tool_name`, `stop_hook_active`). We replay them
//! through the public adapter API and assert the resulting state machine +
//! reporter commands.
//!
//! S15 reliability boundary asserted here:
//! - `working`/`idle`/`done`/`dead` are reliable in ALL surfaces (incl. native UI).
//! - `done` is derived from `Stop`, **never** from `PostToolUse` (#31285).
//! - S15 never produces `waiting`/approval (that's S16/S17), so confidence honesty
//!   is structural.

use std::path::PathBuf;

use fleet_protocol::{Confidence, State};
use fleet_reporter::claude::{ClaudeAdapter, ClaudeHookEvent, ClaudeHookKind, ClaudeParseError};
use fleet_reporter::reporter::ReporterCommand;

const SESSION: &str = "0199f3aa-claude-session-1";

fn fixture(name: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/claude");
    p.push(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"))
}

fn parse(name: &str) -> ClaudeHookEvent {
    ClaudeHookEvent::parse(&fixture(name)).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

// ── each fixture parses to the right kind / fields ───────────────────────────

#[test]
fn fixtures_parse_to_expected_kinds() {
    assert_eq!(
        parse("session_start.json").kind,
        ClaudeHookKind::SessionStart
    );
    assert_eq!(
        parse("user_prompt_submit.json").kind,
        ClaudeHookKind::UserPromptSubmit
    );
    assert_eq!(parse("pre_tool_use.json").kind, ClaudeHookKind::PreToolUse);
    assert_eq!(
        parse("post_tool_use.json").kind,
        ClaudeHookKind::PostToolUse
    );
    assert_eq!(parse("stop_idle.json").kind, ClaudeHookKind::Stop);
    assert_eq!(parse("stop_done.json").kind, ClaudeHookKind::Stop);
    assert_eq!(
        parse("subagent_stop.json").kind,
        ClaudeHookKind::SubagentStop
    );
    assert_eq!(parse("session_end.json").kind, ClaudeHookKind::SessionEnd);
}

#[test]
fn pre_tool_use_fixture_carries_session_and_tool() {
    let e = parse("pre_tool_use.json");
    assert_eq!(e.session_id, SESSION);
    assert_eq!(e.tool_name.as_deref(), Some("Bash"));
    assert_eq!(e.cwd.as_deref(), Some("/Users/dev/project"));
}

#[test]
fn camelcase_fixture_parses() {
    let e = parse("camelcase_user_prompt_submit.json");
    assert_eq!(e.kind, ClaudeHookKind::UserPromptSubmit);
    assert_eq!(e.session_id, SESSION);
}

#[test]
fn stop_done_fixture_marks_completion_idle_does_not() {
    assert!(parse("stop_done.json").turn_complete_done);
    assert!(!parse("stop_idle.json").turn_complete_done);
}

#[test]
fn stop_hook_active_fixture_is_flagged() {
    let e = parse("stop_hook_active.json");
    assert!(e.stop_hook_active);
    assert!(
        e.turn_complete_done,
        "carries the marker but inside a stop hook"
    );
}

// ── drift / error fixtures degrade gracefully ────────────────────────────────

#[test]
fn unknown_hook_fixture_is_other_not_error() {
    // Claude `Notification` does NOT fire in the native UI and is not an S15
    // signal — it must parse to Other (forward-compatible), never crash.
    let e = parse("notification_unknown.json");
    assert!(matches!(e.kind, ClaudeHookKind::Other(_)));
}

#[test]
fn malformed_fixture_errors_cleanly() {
    let err = ClaudeHookEvent::parse(&fixture("malformed.json")).unwrap_err();
    assert!(matches!(err, ClaudeParseError::InvalidJson(_)));
}

#[test]
fn missing_session_id_fixture_errors() {
    let err = ClaudeHookEvent::parse(&fixture("missing_session_id.json")).unwrap_err();
    assert_eq!(err, ClaudeParseError::MissingSessionId);
}

// ── end-to-end replay through the adapter (S15 working/idle/done) ────────────

#[test]
fn replay_working_idle_done_from_fixtures() {
    let mut a = ClaudeAdapter::new();

    a.ingest_json(&fixture("session_start.json"));
    assert_eq!(a.state_of(SESSION), Some(State::Idle));

    a.ingest_json(&fixture("user_prompt_submit.json"));
    assert_eq!(a.state_of(SESSION), Some(State::Working));

    a.ingest_json(&fixture("pre_tool_use.json"));
    assert_eq!(a.state_of(SESSION), Some(State::Working));

    // PostToolUse is liveness-only — must NOT flip to idle/done (#31285).
    a.ingest_json(&fixture("post_tool_use.json"));
    assert_eq!(a.state_of(SESSION), Some(State::Working));

    a.ingest_json(&fixture("stop_idle.json"));
    assert_eq!(a.state_of(SESSION), Some(State::Idle));

    // A fresh prompt then a completion-marked Stop → done (distinct from idle).
    a.ingest_json(&fixture("user_prompt_submit.json"));
    a.ingest_json(&fixture("stop_done.json"));
    assert_eq!(a.state_of(SESSION), Some(State::Done));
}

// ── #31285: done is from Stop, NEVER from PostToolUse ────────────────────────

#[test]
fn post_tool_use_never_completes_the_run() {
    let mut a = ClaudeAdapter::new();
    a.ingest_json(&fixture("user_prompt_submit.json"));
    // Even a long run of PostToolUse fixtures keeps the run Working, not Done.
    for _ in 0..5 {
        a.ingest_json(&fixture("post_tool_use.json"));
    }
    assert_eq!(
        a.state_of(SESSION),
        Some(State::Working),
        "done must be derived from Stop, never PostToolUse (#31285)"
    );
}

// ── stop_hook_active suppresses an over-eager done claim ─────────────────────

#[test]
fn stop_inside_stop_hook_stays_idle() {
    let mut a = ClaudeAdapter::new();
    a.ingest_json(&fixture("user_prompt_submit.json"));
    a.ingest_json(&fixture("stop_hook_active.json"));
    assert_eq!(
        a.state_of(SESSION),
        Some(State::Idle),
        "a Stop from within a stop hook is not a real task end"
    );
}

// ── SubagentStop does not end the main run ───────────────────────────────────

#[test]
fn subagent_stop_does_not_end_run() {
    let mut a = ClaudeAdapter::new();
    a.ingest_json(&fixture("user_prompt_submit.json"));
    let cmds = a.ingest_json(&fixture("subagent_stop.json"));
    assert_eq!(a.state_of(SESSION), Some(State::Working));
    // No state change → at most a liveness command, never an UpsertRun(idle).
    assert!(
        cmds.iter()
            .all(|c| matches!(c, ReporterCommand::Liveness { .. })),
        "SubagentStop must not upsert a state change"
    );
}

// ── SessionEnd → dead (confirmed exit, high) ─────────────────────────────────

#[test]
fn replay_session_end_is_dead_high() {
    let mut a = ClaudeAdapter::new();
    a.ingest_json(&fixture("user_prompt_submit.json"));
    let cmds = a.ingest_json(&fixture("session_end.json"));
    assert_eq!(a.state_of(SESSION), Some(State::Dead));
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
    let mut a = ClaudeAdapter::new();
    let cmds = a.ingest_json(&fixture("malformed.json"));
    assert!(cmds.is_empty());
    assert_eq!(a.session_count(), 0);
}

// ── S15 NEVER emits a waiting/approval upsert from any fixture ───────────────

#[test]
fn no_fixture_ever_produces_waiting() {
    let files = [
        "session_start.json",
        "user_prompt_submit.json",
        "pre_tool_use.json",
        "post_tool_use.json",
        "stop_idle.json",
        "stop_done.json",
        "stop_hook_active.json",
        "subagent_stop.json",
        "notification_unknown.json",
        "session_end.json",
    ];
    let mut a = ClaudeAdapter::new();
    for f in files {
        let cmds = a.ingest_json(&fixture(f));
        for c in &cmds {
            if let ReporterCommand::UpsertRun(run) = c {
                assert_ne!(
                    run.state,
                    State::Waiting,
                    "{f} produced waiting (S15 must not)"
                );
                assert!(run.urgency.is_none(), "{f} produced urgency (S15 must not)");
            }
        }
    }
}

// ── full ordered transcript: each transition observed exactly once ───────────

#[test]
fn full_transcript_transition_sequence() {
    let mut a = ClaudeAdapter::new();
    let script = [
        ("session_start.json", State::Idle),
        ("user_prompt_submit.json", State::Working),
        ("pre_tool_use.json", State::Working),
        ("post_tool_use.json", State::Working),
        ("stop_idle.json", State::Idle),
        ("user_prompt_submit.json", State::Working),
        ("stop_done.json", State::Done),
        ("session_end.json", State::Dead),
    ];
    for (file, expect) in script {
        a.ingest_json(&fixture(file));
        assert_eq!(a.state_of(SESSION), Some(expect), "after {file}");
    }
}
