// Inline unit tests for the S17 shim-aware Claude adapter ,
// `include!`d from `claude_shim.rs`. They assert the one locked S17 acceptance —
// the SAME PermissionRequest approval yields `inferred` in the native-UI surface
// and `high` under the shim terminal — plus the full lifecycle, auto-resolve, the
// confidence-honesty structural invariant, and schema-drift tolerance.

use super::*;
use crate::reporter::ReporterCommand;
use fleet_protocol::{Confidence, State, Urgency};

const SID: &str = "0199f3aa-claude-session-1";

// The SAME approval payload, parsed once, applied under both contexts. This is the
// canonical S17 fixture-in-code: one approval, two confidences.
fn permission_request_json() -> String {
    format!(
        r#"{{"session_id":"{SID}","cwd":"/Users/dev/project","hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{{"command":"rm -rf build"}}}}"#
    )
}

fn permission_response_json(decision: &str) -> String {
    format!(
        r#"{{"session_id":"{SID}","hook_event_name":"PermissionRequest","tool_name":"Bash","permission":"{decision}"}}"#
    )
}

fn prompt_json() -> String {
    format!(r#"{{"session_id":"{SID}","hook_event_name":"UserPromptSubmit","prompt":"go"}}"#)
}

fn stop_idle_json() -> String {
    format!(r#"{{"session_id":"{SID}","hook_event_name":"Stop","stop_hook_active":false}}"#)
}

fn session_end_json() -> String {
    format!(r#"{{"session_id":"{SID}","hook_event_name":"SessionEnd","reason":"exit"}}"#)
}

// ── THE S17 acceptance: same approval, two confidences ───────────────────────

#[test]
fn same_approval_yields_inferred_native_vs_high_shim() {
    let json = permission_request_json();
    let req = ApprovalRequest::parse(&json).unwrap().unwrap();
    assert!(!req.is_response(), "a bare request is not a response");

    // Native UI: PermissionRequest does not fire authoritatively → inferred.
    let mut native = ClaudeShimStateMachine::new(SID, LaunchContext::NativeUi);
    let t_native = native.apply_approval(&req);
    assert_eq!(native.state(), State::Waiting);
    assert_eq!(native.urgency(), Some(Urgency::Approval));
    assert_eq!(
        t_native.confidence,
        Confidence::Inferred,
        "native-UI approval must be inferred (PermissionRequest does not fire there)"
    );

    // Shim terminal: authoritative PermissionRequest → high.
    let mut shim = ClaudeShimStateMachine::new(SID, LaunchContext::ShimTerminal);
    let t_shim = shim.apply_approval(&req);
    assert_eq!(shim.state(), State::Waiting);
    assert_eq!(shim.urgency(), Some(Urgency::Approval));
    assert_eq!(
        t_shim.confidence,
        Confidence::High,
        "shimmed integrated-terminal approval must be high (Use-Terminal mode)"
    );

    // The states/urgencies are identical; ONLY confidence differs.
    assert_eq!(t_native.state, t_shim.state);
    assert_eq!(t_native.urgency, t_shim.urgency);
    assert_ne!(t_native.confidence, t_shim.confidence);
}

#[test]
fn same_approval_two_confidences_through_the_adapter() {
    let json = permission_request_json();

    let mut native = ClaudeShimAdapter::new(LaunchContext::NativeUi);
    let cmds_n = native.ingest_json(&json);
    assert_eq!(native.state_of(SID), Some(State::Waiting));
    assert_eq!(native.confidence_of(SID), Some(Confidence::Inferred));
    assert_waiting_upsert(&cmds_n, Confidence::Inferred);

    let mut shim = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    let cmds_s = shim.ingest_json(&json);
    assert_eq!(shim.state_of(SID), Some(State::Waiting));
    assert_eq!(shim.confidence_of(SID), Some(Confidence::High));
    assert_waiting_upsert(&cmds_s, Confidence::High);
}

fn assert_waiting_upsert(cmds: &[ReporterCommand], expect: Confidence) {
    let run = cmds
        .iter()
        .find_map(|c| match c {
            ReporterCommand::UpsertRun(r) => Some(r),
            _ => None,
        })
        .expect("a waiting upsert");
    assert_eq!(run.state, State::Waiting);
    assert_eq!(run.urgency, Some(Urgency::Approval));
    assert_eq!(run.confidence, expect);
    assert!(run.waiting_since.is_some(), "waiting carries waiting_since");
}

// ── launch context predicate + env detection ────────────────────────────────

#[test]
fn launch_context_gates_high_confidence() {
    assert!(LaunchContext::ShimTerminal.permission_request_is_authoritative());
    assert!(!LaunchContext::NativeUi.permission_request_is_authoritative());
    assert_eq!(LaunchContext::ShimTerminal.approval_confidence(), Confidence::High);
    assert_eq!(LaunchContext::NativeUi.approval_confidence(), Confidence::Inferred);
}

#[test]
fn launch_context_from_env() {
    let shim = LaunchContext::from_env(|k| (k == "FLEET_SHIM").then_some("claude"));
    assert_eq!(shim, LaunchContext::ShimTerminal);

    let shim1 = LaunchContext::from_env(|k| (k == "FLEET_SHIM").then_some("1"));
    assert_eq!(shim1, LaunchContext::ShimTerminal);

    // No shim env at all → native UI (the conservative, honest default).
    let native = LaunchContext::from_env(|_| None);
    assert_eq!(native, LaunchContext::NativeUi);

    // An unrelated FLEET_SHIM value does not falsely claim authority.
    let other = LaunchContext::from_env(|k| (k == "FLEET_SHIM").then_some("codex"));
    assert_eq!(other, LaunchContext::NativeUi);
}

// ── full lifecycle in the shim terminal ──────────────────────────────────────

#[test]
fn full_shim_lifecycle_working_waiting_high_resolve_done() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);

    a.ingest_json(&prompt_json());
    assert_eq!(a.state_of(SID), Some(State::Working));
    assert_eq!(a.confidence_of(SID), Some(Confidence::Inferred));

    // Authoritative approval → waiting + high.
    a.ingest_json(&permission_request_json());
    assert_eq!(a.state_of(SID), Some(State::Waiting));
    assert_eq!(a.confidence_of(SID), Some(Confidence::High));
    assert!(a.machine_of(SID).unwrap().awaiting_approval());

    // Answer in the terminal → auto-resolve back to working, confidence drops to
    // inferred (working is never authoritative).
    a.ingest_json(&permission_response_json("allow"));
    assert_eq!(a.state_of(SID), Some(State::Working));
    assert_eq!(a.confidence_of(SID), Some(Confidence::Inferred));
    assert!(!a.machine_of(SID).unwrap().awaiting_approval());

    // Stop → idle.
    a.ingest_json(&stop_idle_json());
    assert_eq!(a.state_of(SID), Some(State::Idle));
}

#[test]
fn activity_auto_resolves_a_pending_approval() {
    // Even without an explicit decision event, fresh activity clears waiting.
    let mut m = ClaudeShimStateMachine::new(SID, LaunchContext::ShimTerminal);
    let req = ApprovalRequest::parse(&permission_request_json()).unwrap().unwrap();
    m.apply_approval(&req);
    assert_eq!(m.state(), State::Waiting);

    let ev = ClaudeHookEvent::parse(&prompt_json()).unwrap();
    let t = m.apply(&ev);
    assert_eq!(m.state(), State::Working);
    assert!(t.resolved_approval, "fresh activity auto-resolves the approval");
    assert!(!m.awaiting_approval());
}

#[test]
fn deny_response_also_resolves() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    a.ingest_json(&permission_request_json());
    assert_eq!(a.state_of(SID), Some(State::Waiting));
    a.ingest_json(&permission_response_json("deny"));
    assert_eq!(a.state_of(SID), Some(State::Working));
}

#[test]
fn stop_while_waiting_clears_to_idle() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    a.ingest_json(&permission_request_json());
    assert_eq!(a.state_of(SID), Some(State::Waiting));
    a.ingest_json(&stop_idle_json());
    assert_eq!(a.state_of(SID), Some(State::Idle));
    assert_eq!(a.confidence_of(SID), Some(Confidence::Inferred));
}

#[test]
fn session_end_is_dead_high_even_while_waiting() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    a.ingest_json(&permission_request_json());
    let cmds = a.ingest_json(&session_end_json());
    assert_eq!(a.state_of(SID), Some(State::Dead));
    let run = cmds
        .iter()
        .find_map(|c| match c {
            ReporterCommand::UpsertRun(r) => Some(r),
            _ => None,
        })
        .expect("dead upsert");
    assert_eq!(run.state, State::Dead);
    assert_eq!(run.confidence, Confidence::High);
    assert!(run.urgency.is_none());
}

// ── identity is the same anchor S15 uses (durable across contexts) ───────────

#[test]
fn native_id_is_session_id_regardless_of_context() {
    for ctx in [LaunchContext::ShimTerminal, LaunchContext::NativeUi] {
        let m = ClaudeShimStateMachine::new(SID, ctx);
        let run = m.to_run("run-1", "2026-06-08T00:00:00Z");
        assert_eq!(run.native_id, SID);
        assert_eq!(run.agent_kind, fleet_protocol::AgentKind::ClaudeCode);
    }
}

// ── routing: foreign session never mutates this machine ──────────────────────

#[test]
fn foreign_session_event_is_noop() {
    let mut m = ClaudeShimStateMachine::new(SID, LaunchContext::ShimTerminal);
    let foreign =
        r#"{"session_id":"someone-else","hook_event_name":"PermissionRequest","tool_name":"Bash"}"#;
    let req = ApprovalRequest::parse(foreign).unwrap().unwrap();
    let t = m.apply_approval(&req);
    assert!(!t.changed);
    assert_eq!(m.state(), State::Idle);
}

// ── schema-drift / malformed input degrades, never panics or overstates ──────

#[test]
fn malformed_line_creates_no_ghost_and_no_state() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    let cmds = a.ingest_json("{ this is not json");
    assert!(cmds.is_empty());
    assert_eq!(a.session_count(), 0);
}

#[test]
fn non_permission_event_parses_to_no_approval() {
    // A lifecycle event is not an ApprovalRequest.
    let r = ApprovalRequest::parse(&prompt_json()).unwrap();
    assert!(r.is_none());
}

#[test]
fn permission_request_with_no_decision_envelope_is_a_request() {
    let r = ApprovalRequest::parse(&permission_request_json())
        .unwrap()
        .unwrap();
    assert!(!r.is_response());
    assert_eq!(r.tool_name.as_deref(), Some("Bash"));
}

#[test]
fn structured_decision_envelope_parses() {
    let json = format!(
        r#"{{"session_id":"{SID}","hook_event_name":"PermissionRequest","decision":{{"behavior":"allow"}}}}"#
    );
    let r = ApprovalRequest::parse(&json).unwrap().unwrap();
    assert!(r.is_response());
}

#[test]
fn unknown_decision_token_is_treated_as_fresh_request() {
    // A decision token we don't recognise must NOT silently resolve the approval.
    let json = permission_response_json("frobnicate");
    let r = ApprovalRequest::parse(&json).unwrap().unwrap();
    assert!(!r.is_response(), "unknown decision token ⇒ not a resolution");

    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    a.ingest_json(&json);
    // Stays waiting (the approval is still outstanding), high confidence.
    assert_eq!(a.state_of(SID), Some(State::Waiting));
    assert_eq!(a.confidence_of(SID), Some(Confidence::High));
}

// ── liveness-only events do not flip state ───────────────────────────────────

#[test]
fn post_tool_use_is_liveness_only() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    a.ingest_json(&prompt_json());
    let json = format!(
        r#"{{"session_id":"{SID}","hook_event_name":"PostToolUse","tool_name":"Bash"}}"#
    );
    let cmds = a.ingest_json(&json);
    assert_eq!(a.state_of(SID), Some(State::Working));
    assert!(cmds.iter().all(|c| matches!(c, ReporterCommand::Liveness { .. })));
}

// ── property: confidence honesty is structural ───────────────────────────────
//
// In the native UI, NO event sequence can ever produce `high` for a `waiting`
// state. The only `high` allowed in native UI is the confirmed-exit `dead`.

mod props {
    use super::super::*;
    use super::SID;
    use fleet_protocol::{Confidence, State};
    use proptest::prelude::*;

    fn any_hook_json(sid: &str) -> impl Strategy<Value = String> {
        let sid = sid.to_string();
        prop_oneof![
            Just("UserPromptSubmit"),
            Just("PreToolUse"),
            Just("PostToolUse"),
            Just("PermissionRequest"),
            Just("Stop"),
            Just("SessionStart"),
            Just("SubagentStop"),
            Just("Frobnicate"),
        ]
        .prop_map(move |name| {
            format!(
                r#"{{"session_id":"{sid}","hook_event_name":"{name}","tool_name":"Bash"}}"#
            )
        })
    }

    proptest! {
        // In native UI, a `waiting` state is NEVER `high` — the same machine that
        // is `high` under the shim must be `inferred` here. This is the central
        // confidence-honesty guarantee, fuzzed over arbitrary event streams.
        #[test]
        fn native_ui_waiting_is_never_high(events in proptest::collection::vec(any_hook_json(SID), 0..40)) {
            let mut a = ClaudeShimAdapter::new(LaunchContext::NativeUi);
            for e in &events {
                a.ingest_json(e);
                if a.state_of(SID) == Some(State::Waiting) {
                    prop_assert_eq!(a.confidence_of(SID), Some(Confidence::Inferred));
                }
            }
        }

        // Under the shim, a `waiting` state is ALWAYS `high` (authoritative).
        #[test]
        fn shim_waiting_is_always_high(events in proptest::collection::vec(any_hook_json(SID), 0..40)) {
            let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
            for e in &events {
                a.ingest_json(e);
                if a.state_of(SID) == Some(State::Waiting) {
                    prop_assert_eq!(a.confidence_of(SID), Some(Confidence::High));
                }
            }
        }

        // No event sequence ever panics, and `high` only ever accompanies
        // `waiting` (shim) or `dead` (confirmed exit) — never `working`/`idle`/`done`.
        #[test]
        fn high_only_on_waiting_or_dead(events in proptest::collection::vec(any_hook_json(SID), 0..40)) {
            for ctx in [LaunchContext::ShimTerminal, LaunchContext::NativeUi] {
                let mut a = ClaudeShimAdapter::new(ctx);
                for e in &events {
                    a.ingest_json(e);
                    if a.confidence_of(SID) == Some(Confidence::High) {
                        let st = a.state_of(SID).unwrap();
                        prop_assert!(
                            st == State::Waiting || st == State::Dead,
                            "high confidence leaked to {:?}", st
                        );
                    }
                }
            }
        }
    }
}
