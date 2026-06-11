//! Integration tests for the inferred-waiting Claude adapter against recorded
//! hook-event JSON fixtures and recorded transcript-JSONL fixtures on disk.
//!
//! These do NOT require a real claude/VS Code install. We drive the lifecycle hook
//! fixtures under `tests/fixtures/claude/` through the S16 debounce machine and the
//! transcript fixtures under `tests/fixtures/transcript/` through the
//! `corroborate_jsonl` drift-guard, asserting the native-UI surface behavior: a
//! `PreToolUse`-without-`Stop` debounce yields
//! `waiting`+`approval` at **`confidence: inferred`** (never `high`), with the JSONL
//! corroborating or vetoing the inference.

use std::path::PathBuf;

use fleet_protocol::{Confidence, State, Urgency};
use fleet_reporter::claude::ClaudeHookEvent;
use fleet_reporter::claude_infer::{
    corroborate_jsonl, ClaudeInferAdapter, ClaudeInferMachine, Corroboration,
};
use fleet_reporter::reporter::ReporterCommand;

const SID: &str = "0199f3aa-claude-session-1";
const WINDOW: u64 = 1_500;

fn claude_fixture(name: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/claude");
    p.push(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"))
}

fn transcript_fixture(name: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/transcript");
    p.push(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"))
}

// ── THE S16 acceptance from recorded fixtures: debounce -> inferred waiting ────

#[test]
fn pre_tool_use_fixture_without_stop_infers_inferred_waiting() {
    let mut m = ClaudeInferMachine::with_debounce_ms(SID, WINDOW);

    m.apply(
        &ClaudeHookEvent::parse(&claude_fixture("pre_tool_use.json")).unwrap(),
        0,
    );
    assert_eq!(m.state(), State::Working);
    assert!(m.is_debouncing());

    assert!(!m.tick(WINDOW - 1).changed);

    let t = m.tick(WINDOW);
    assert!(t.changed);
    assert_eq!(t.state, State::Waiting);
    assert_eq!(t.urgency, Some(Urgency::Approval));
    assert_eq!(t.confidence, Confidence::Inferred);
}

// ── transcript fixtures drive the drift-guard verdicts ───────────────────────

#[test]
fn pending_approval_transcript_corroborates() {
    assert_eq!(
        corroborate_jsonl(&transcript_fixture("pending_approval.jsonl")),
        Corroboration::Stuck,
        "an issued tool_use with no tool_result corroborates a stuck PreToolUse"
    );
}

#[test]
fn resolved_transcript_vetoes() {
    assert_eq!(
        corroborate_jsonl(&transcript_fixture("resolved.jsonl")),
        Corroboration::Resolved,
        "a tool_use with its matching tool_result vetoes the inference"
    );
}

#[test]
fn drifted_transcript_degrades_without_overstating() {
    // The drift fixture has a last anchored tool_use (`toolu_keep`) with no
    // matching result, plus drifted/truncated lines that must be skipped, never
    // panicking. The anchored pending tool stands.
    assert_eq!(
        corroborate_jsonl(&transcript_fixture("drifted.jsonl")),
        Corroboration::Stuck,
        "drifted lines are skipped; the one anchored pending tool_use still counts"
    );
}

// ── end-to-end: the transcript veto suppresses a would-be false waiting ───────

#[test]
fn resolved_transcript_suppresses_debounce_end_to_end() {
    let mut m = ClaudeInferMachine::with_debounce_ms(SID, WINDOW);
    m.apply(
        &ClaudeHookEvent::parse(&claude_fixture("pre_tool_use.json")).unwrap(),
        0,
    );
    let verdict = corroborate_jsonl(&transcript_fixture("resolved.jsonl"));
    let t = m.corroborate(verdict);
    assert!(
        !t.changed,
        "the resolved transcript vetoes the false waiting"
    );
    assert!(
        !m.is_debouncing(),
        "the resolved transcript cancels the arm"
    );
    assert!(!m.tick(WINDOW * 5).changed);
    assert_eq!(m.state(), State::Working);
}

#[test]
fn pending_transcript_confirms_debounce_end_to_end() {
    let mut m = ClaudeInferMachine::with_debounce_ms(SID, WINDOW);
    m.apply(
        &ClaudeHookEvent::parse(&claude_fixture("pre_tool_use.json")).unwrap(),
        0,
    );
    let verdict = corroborate_jsonl(&transcript_fixture("pending_approval.jsonl"));
    assert_eq!(verdict, Corroboration::Stuck);
    assert!(
        !m.corroborate(verdict).changed,
        "Stuck does not itself raise waiting"
    );
    assert!(m.is_debouncing());
    let t = m.tick(WINDOW);
    assert!(t.changed);
    assert_eq!(m.state(), State::Waiting);
    assert_eq!(m.confidence(), Confidence::Inferred);
}

// ── adapter-level replay through the gated reporter-command surface ───────────

#[test]
fn adapter_full_native_ui_replay_from_fixtures() {
    let mut a = ClaudeInferAdapter::with_debounce_ms(WINDOW);

    a.ingest_json(&claude_fixture("user_prompt_submit.json"), 0);
    assert_eq!(a.state_of(SID), Some(State::Working));

    a.ingest_json(&claude_fixture("pre_tool_use.json"), 100);
    let cmds = a.tick(100 + WINDOW);
    let run = cmds
        .iter()
        .find_map(|c| match c {
            ReporterCommand::UpsertRun(r) => Some(r),
            _ => None,
        })
        .expect("an inferred waiting upsert");
    assert_eq!(run.state, State::Waiting);
    assert_eq!(run.urgency, Some(Urgency::Approval));
    assert_eq!(run.confidence, Confidence::Inferred);

    a.ingest_json(&claude_fixture("stop_idle.json"), 100 + WINDOW + 50);
    assert_eq!(a.state_of(SID), Some(State::Idle));
}

// ── a malformed hook line never creates a ghost run (drift-guard) ────────────

#[test]
fn malformed_hook_fixture_creates_no_ghost() {
    let mut a = ClaudeInferAdapter::with_debounce_ms(WINDOW);
    let cmds = a.ingest_json(&claude_fixture("malformed.json"), 0);
    assert!(cmds.is_empty());
    assert_eq!(a.session_count(), 0);
}
