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

    // Answer in the terminal → Claude resumes and fires the NEXT activity hook
    // (there is no inbound decision event), auto-resolving back to working with
    // confidence dropping to inferred (working is never authoritative).
    a.ingest_json(&prompt_json());
    assert_eq!(a.state_of(SID), Some(State::Working));
    assert_eq!(a.confidence_of(SID), Some(Confidence::Inferred));
    assert!(!a.machine_of(SID).unwrap().awaiting_approval());

    // Stop → done (the turn-complete signal).
    a.ingest_json(&stop_idle_json());
    assert_eq!(a.state_of(SID), Some(State::Done));
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
fn stop_while_waiting_clears_to_done() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    a.ingest_json(&permission_request_json());
    assert_eq!(a.state_of(SID), Some(State::Waiting));
    a.ingest_json(&stop_idle_json());
    assert_eq!(a.state_of(SID), Some(State::Done), "a real Stop ends the turn → done");
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
fn permission_request_is_a_request_with_the_tool() {
    let r = ApprovalRequest::parse(&permission_request_json())
        .unwrap()
        .unwrap();
    assert_eq!(r.tool_name.as_deref(), Some("Bash"));
    assert_eq!(r.session_id, SID);
}

#[test]
fn permission_request_carrying_a_decision_field_still_raises_waiting() {
    // REGRESSION (item 3): a `decision` is a hook OUTPUT, not an inbound event.
    // A PermissionRequest that happens to carry one is STILL a fresh request — the
    // decision is ignored and the gate is raised (never a silent auto-resolve).
    let json = format!(
        r#"{{"session_id":"{SID}","hook_event_name":"PermissionRequest","tool_name":"Bash","decision":{{"behavior":"allow"}}}}"#
    );
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    a.ingest_json(&json);
    assert_eq!(a.state_of(SID), Some(State::Waiting));
    assert_eq!(a.confidence_of(SID), Some(Confidence::High));
}

// ── continuation Stop (stop_hook_active) is liveness, not a completion ────────

#[test]
fn continuation_stop_stays_working() {
    let mut m = ClaudeShimStateMachine::new(SID, LaunchContext::ShimTerminal);
    m.apply(&ClaudeHookEvent::parse(&prompt_json()).unwrap()); // working
    let cont = format!(
        r#"{{"session_id":"{SID}","hook_event_name":"Stop","stop_hook_active":true}}"#
    );
    let t = m.apply(&ClaudeHookEvent::parse(&cont).unwrap());
    assert_eq!(m.state(), State::Working, "a continuation Stop is not a turn end");
    assert!(!t.changed);
    assert!(t.liveness);
}

// ── accessors expose the real machine identity / context / cwd ───────────────

#[test]
fn machine_accessors_reflect_real_state() {
    let mut m = ClaudeShimStateMachine::new(SID, LaunchContext::ShimTerminal);
    // Identity + fixed context are exposed verbatim.
    assert_eq!(m.session_id(), SID);
    assert_eq!(m.context(), LaunchContext::ShimTerminal);
    // cwd defaults to "/" then tracks the event's cwd.
    assert_eq!(m.cwd(), "/");

    let ev = ClaudeHookEvent::parse(&permission_request_json()).unwrap();
    // permission_request_json carries cwd "/Users/dev/project"; it is routed as a
    // PermissionRequest through apply() and updates cwd.
    m.apply(&ev);
    assert_eq!(m.cwd(), "/Users/dev/project");
    assert_eq!(m.state(), State::Waiting);
}

// ── apply() lifecycle path: foreign session is an idempotent no-op ───────────

#[test]
fn apply_lifecycle_foreign_session_is_noop() {
    let mut m = ClaudeShimStateMachine::new(SID, LaunchContext::ShimTerminal);
    let foreign =
        r#"{"session_id":"not-ours","hook_event_name":"UserPromptSubmit","prompt":"go"}"#;
    let ev = ClaudeHookEvent::parse(foreign).unwrap();
    let t = m.apply(&ev);
    assert!(!t.changed, "a foreign-session lifecycle event must not change state");
    assert_eq!(m.state(), State::Idle, "still idle, untouched");
}

// ── apply() routes a PermissionRequest through raise_approval ─────────────────

#[test]
fn apply_routes_permission_request_to_raise_approval() {
    // A PermissionRequest reaching the lifecycle apply() path (not apply_approval)
    // is raised as a fresh approval, stamped with the launch-context confidence.
    let mut m = ClaudeShimStateMachine::new(SID, LaunchContext::ShimTerminal);
    let ev = ClaudeHookEvent::parse(&permission_request_json()).unwrap();
    let t = m.apply(&ev);
    assert_eq!(m.state(), State::Waiting);
    assert_eq!(m.urgency(), Some(Urgency::Approval));
    assert_eq!(t.confidence, Confidence::High);
    assert!(m.awaiting_approval());
}

// ── Stop carrying a completion marker ⇒ Done (not Idle) ──────────────────────

#[test]
fn stop_with_completion_marker_is_done() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    a.ingest_json(&prompt_json());
    assert_eq!(a.state_of(SID), Some(State::Working));
    // A real Stop (stop_hook_active:false) ⇒ Done; the phantom `task_complete` is
    // ignored (the Stop event itself is the turn-complete signal).
    let stop_done = format!(
        r#"{{"session_id":"{SID}","hook_event_name":"Stop","stop_hook_active":false,"task_complete":true}}"#
    );
    a.ingest_json(&stop_done);
    assert_eq!(a.state_of(SID), Some(State::Done));
    assert_eq!(a.confidence_of(SID), Some(Confidence::Inferred));
}

// ── SessionStart after a confirmed exit revives the run to idle ──────────────

#[test]
fn session_start_after_session_end_revives_to_idle() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    a.ingest_json(&prompt_json());
    a.ingest_json(&session_end_json());
    assert_eq!(a.state_of(SID), Some(State::Dead));
    assert_eq!(a.confidence_of(SID), Some(Confidence::High));

    // A fresh SessionStart on the same session id revives it: dead → idle, and the
    // transition is marked changed (into_changed) so a fresh upsert is emitted.
    let start = format!(r#"{{"session_id":"{SID}","hook_event_name":"SessionStart"}}"#);
    let cmds = a.ingest_json(&start);
    assert_eq!(a.state_of(SID), Some(State::Idle));
    assert_eq!(a.confidence_of(SID), Some(Confidence::Inferred));
    let run = cmds
        .iter()
        .find_map(|c| match c {
            ReporterCommand::UpsertRun(r) => Some(r),
            _ => None,
        })
        .expect("revival emits an upsert because into_changed marks it changed");
    assert_eq!(run.state, State::Idle);
}

#[test]
fn session_start_on_live_run_is_a_noop() {
    // SessionStart while not dead is an idempotent no-op (the else arm).
    let mut m = ClaudeShimStateMachine::new(SID, LaunchContext::ShimTerminal);
    let prompt = ClaudeHookEvent::parse(&prompt_json()).unwrap();
    m.apply(&prompt);
    assert_eq!(m.state(), State::Working);
    let start = format!(r#"{{"session_id":"{SID}","hook_event_name":"SessionStart"}}"#);
    let ev = ClaudeHookEvent::parse(&start).unwrap();
    let t = m.apply(&ev);
    assert!(!t.changed, "SessionStart on a live run does not change state");
    assert_eq!(m.state(), State::Working);
}

// ── last_message: every state's human-readable summary in to_run ─────────────

#[test]
fn waiting_with_no_tool_says_approval_required() {
    // A PermissionRequest with no tool_name ⇒ last_message "Approval required".
    let mut m = ClaudeShimStateMachine::new(SID, LaunchContext::ShimTerminal);
    let json = format!(
        r#"{{"session_id":"{SID}","hook_event_name":"PermissionRequest"}}"#
    );
    let req = ApprovalRequest::parse(&json).unwrap().unwrap();
    m.apply_approval(&req);
    assert_eq!(m.state(), State::Waiting);
    let run = m.to_run("run-1", "2026-06-08T00:00:00Z");
    assert_eq!(run.last_message.as_deref(), Some("Approval required"));
}

#[test]
fn done_state_reports_task_complete_message() {
    let mut m = ClaudeShimStateMachine::new(SID, LaunchContext::ShimTerminal);
    let stop_done = format!(
        r#"{{"session_id":"{SID}","hook_event_name":"Stop","stop_hook_active":false,"task_complete":true}}"#
    );
    let ev = ClaudeHookEvent::parse(&stop_done).unwrap();
    m.apply(&ev);
    assert_eq!(m.state(), State::Done);
    let run = m.to_run("run-1", "2026-06-08T00:00:00Z");
    assert_eq!(run.last_message.as_deref(), Some("Task complete."));
}

// ── adapter accessors: context / run_id_of / ingest_json error path / forget ──

#[test]
fn adapter_context_is_exposed() {
    assert_eq!(
        ClaudeShimAdapter::new(LaunchContext::ShimTerminal).context(),
        LaunchContext::ShimTerminal
    );
    assert_eq!(
        ClaudeShimAdapter::new(LaunchContext::NativeUi).context(),
        LaunchContext::NativeUi
    );
}

#[test]
fn run_id_of_returns_minted_id_for_tracked_session() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    assert_eq!(a.run_id_of(SID), None, "untracked session has no run id");
    a.ingest_json(&prompt_json());
    let run_id = a.run_id_of(SID).expect("a tracked session has a minted run id");
    assert_eq!(run_id, format!("claude:{SID}:run-1"));
}

#[test]
fn ingest_json_valid_json_but_unparseable_event_is_swallowed() {
    // Valid JSON that the ApprovalRequest path skips (not a PermissionRequest) but
    // that the lifecycle ClaudeHookEvent::parse rejects (no session_id) must be
    // swallowed: no commands, no ghost session.
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    let cmds = a.ingest_json(r#"{"hook_event_name":"UserPromptSubmit","prompt":"go"}"#);
    assert!(cmds.is_empty(), "an event missing session_id yields no commands");
    assert_eq!(a.session_count(), 0, "and creates no ghost session");
}

#[test]
fn forget_removes_a_tracked_session_only_once() {
    let mut a = ClaudeShimAdapter::new(LaunchContext::ShimTerminal);
    a.ingest_json(&prompt_json());
    assert_eq!(a.session_count(), 1);
    assert!(a.forget(SID), "forgetting a tracked session returns true");
    assert_eq!(a.session_count(), 0);
    assert!(!a.forget(SID), "forgetting an unknown session returns false");
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
