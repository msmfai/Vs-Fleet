//! Integration tests: the S17 shim-aware Claude adapter  vs
//! **recorded hook-event JSON fixtures** on disk (//! recorded fixtures").
//!
//! These do NOT require a real claude/VS Code install — the fixtures under
//! `tests/fixtures/claude_shim/` are recorded Claude `PermissionRequest` hook
//! payloads (request + allow/deny/structured responses) plus the lifecycle hooks
//! around them.
//!
//! The acceptance asserted here (the engineering spec / §21.3): the **same** recorded
//! `PermissionRequest` approval fixture yields **`confidence: inferred`** when the
//! run is in the native-UI surface and **`confidence: high`** when it is launched
//! via the integrated-terminal shim (Use-Terminal mode). State + urgency are
//! identical; only confidence differs — confidence honesty (§3 invariant 5).

use std::path::PathBuf;

use fleet_protocol::{Confidence, State, Urgency};
use fleet_reporter::claude_shim::{
    ApprovalRequest, ClaudeShimAdapter, ClaudeShimStateMachine, LaunchContext,
};
use fleet_reporter::reporter::ReporterCommand;

const SID: &str = "0199f3aa-claude-session-1";

fn fixture(name: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/claude_shim");
    p.push(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"))
}

fn upsert(cmds: &[ReporterCommand]) -> &fleet_protocol::AgentRun {
    cmds.iter()
        .find_map(|c| match c {
            ReporterCommand::UpsertRun(r) => Some(r),
            _ => None,
        })
        .expect("an UpsertRun command")
}

// ── THE S17 acceptance against the recorded fixture ──────────────────────────

#[test]
fn same_fixture_inferred_in_native_high_in_shim() {
    let json = fixture("permission_request.json");

    // Native UI surface.
    let mut native = ClaudeShimAdapter::new(LaunchContext::NativeUi);
    let n = native.ingest_json(&json);
    let run_n = upsert(&n);
    assert_eq!(run_n.state, State::Waiting);
    assert_eq!(run_n.urgency, Some(Urgency::Approval));
    assert_eq!(run_n.confidence, Confidence::Inferred);

    // Shim terminal surface (same bytes).
    let mut shim = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    let s = shim.ingest_json(&json);
    let run_s = upsert(&s);
    assert_eq!(run_s.state, State::Waiting);
    assert_eq!(run_s.urgency, Some(Urgency::Approval));
    assert_eq!(run_s.confidence, Confidence::High);

    // Identical except confidence — the whole point of S17.
    assert_eq!(run_n.state, run_s.state);
    assert_eq!(run_n.urgency, run_s.urgency);
    assert_eq!(run_n.native_id, run_s.native_id);
    assert_ne!(run_n.confidence, run_s.confidence);
}

// ── the parsed fixture is recognised as an approval request ──────────────────

#[test]
fn permission_request_fixture_parses_as_request() {
    let req = ApprovalRequest::parse(&fixture("permission_request.json"))
        .unwrap()
        .unwrap();
    assert_eq!(req.session_id, SID);
    assert_eq!(req.tool_name.as_deref(), Some("Bash"));
    assert!(!req.is_response());
}

#[test]
fn allow_response_fixture_is_a_response() {
    let req = ApprovalRequest::parse(&fixture("permission_response_allow.json"))
        .unwrap()
        .unwrap();
    assert!(req.is_response());
}

#[test]
fn structured_response_fixture_is_a_response() {
    let req = ApprovalRequest::parse(&fixture("permission_response_structured.json"))
        .unwrap()
        .unwrap();
    assert!(req.is_response());
}

// ── full shim transcript from fixtures: working→waiting(high)→resolve→idle ────

#[test]
fn full_shim_transcript_from_fixtures() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);

    a.ingest_json(&fixture("user_prompt_submit.json"));
    assert_eq!(a.state_of(SID), Some(State::Working));

    a.ingest_json(&fixture("permission_request.json"));
    assert_eq!(a.state_of(SID), Some(State::Waiting));
    assert_eq!(a.confidence_of(SID), Some(Confidence::High));

    a.ingest_json(&fixture("permission_response_allow.json"));
    assert_eq!(a.state_of(SID), Some(State::Working));

    a.ingest_json(&fixture("stop_idle.json"));
    assert_eq!(a.state_of(SID), Some(State::Idle));
}

// ── a lifecycle fixture is not an approval ──────────────────────────────────

#[test]
fn lifecycle_fixture_is_not_an_approval() {
    let r = ApprovalRequest::parse(&fixture("user_prompt_submit.json")).unwrap();
    assert!(r.is_none());
}

// ── the state-machine layer agrees with the adapter layer ────────────────────

#[test]
fn machine_layer_matches_adapter_for_the_fixture() {
    let req = ApprovalRequest::parse(&fixture("permission_request.json"))
        .unwrap()
        .unwrap();
    let mut shim = ClaudeShimStateMachine::new(SID, LaunchContext::ShimTerminal);
    let t = shim.apply_approval(&req);
    assert_eq!(t.confidence, Confidence::High);

    let mut native = ClaudeShimStateMachine::new(SID, LaunchContext::NativeUi);
    let t = native.apply_approval(&req);
    assert_eq!(t.confidence, Confidence::Inferred);
}
