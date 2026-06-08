// Inline unit tests for the S16 inferred-waiting Claude adapter (node CLINFER),
// `include!`d from `claude_infer.rs`. Cover the locked S16 acceptance --
// PreToolUse-without-Stop debounce -> inferred waiting+approval, confidence:
// inferred, with exact debounce timing -- plus the transcript-JSONL drift-guard
// corroboration, auto-resolve, and the confidence-honesty structural invariant
// (this machine can NEVER produce `high` for a waiting state).

use super::*;
use crate::reporter::ReporterCommand;
use fleet_protocol::{Confidence, State, Urgency};

const SID: &str = "0199f3aa-claude-session-1";
const WINDOW: u64 = 1_500;

fn prompt_json() -> String {
    format!(r#"{{"session_id":"{SID}","hook_event_name":"UserPromptSubmit","prompt":"go"}}"#)
}
fn pre_tool_json() -> String {
    format!(
        r#"{{"session_id":"{SID}","cwd":"/Users/dev/project","hook_event_name":"PreToolUse","tool_name":"Bash"}}"#
    )
}
fn post_tool_json() -> String {
    format!(r#"{{"session_id":"{SID}","hook_event_name":"PostToolUse","tool_name":"Bash"}}"#)
}
fn stop_idle_json() -> String {
    format!(r#"{{"session_id":"{SID}","hook_event_name":"Stop","stop_hook_active":false}}"#)
}
fn session_end_json() -> String {
    format!(r#"{{"session_id":"{SID}","hook_event_name":"SessionEnd","reason":"exit"}}"#)
}

fn ev(json: &str) -> ClaudeHookEvent {
    ClaudeHookEvent::parse(json).unwrap()
}

fn machine() -> ClaudeInferMachine {
    ClaudeInferMachine::with_debounce_ms(SID, WINDOW)
}

// ── THE S16 acceptance: PreToolUse-without-Stop debounce -> inferred waiting ──

#[test]
fn pre_tool_use_without_stop_infers_waiting_after_window() {
    let mut m = machine();
    let t = m.apply(&ev(&pre_tool_json()), 0);
    assert_eq!(t.state, State::Working);
    assert!(m.is_debouncing());
    assert!(!m.is_inferred_waiting());

    let t = m.tick(WINDOW - 1);
    assert!(!t.changed, "tick at window-1 must not infer waiting");
    assert_eq!(m.state(), State::Working);
    assert!(m.is_debouncing());

    let t = m.tick(WINDOW);
    assert!(t.changed, "tick at the window must infer waiting");
    assert_eq!(t.state, State::Waiting);
    assert_eq!(t.urgency, Some(Urgency::Approval));
    assert_eq!(t.confidence, Confidence::Inferred, "inferred waiting must never be high");
    assert!(m.is_inferred_waiting());
}

#[test]
fn tick_is_idempotent_once_fired() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    assert!(m.tick(WINDOW).changed);
    assert!(!m.tick(WINDOW * 10).changed);
    assert_eq!(m.state(), State::Waiting);
}

#[test]
fn tick_with_no_arm_is_noop() {
    let mut m = machine();
    for now in [0, WINDOW, WINDOW * 5] {
        let t = m.tick(now);
        assert!(!t.changed);
        assert_ne!(m.state(), State::Waiting);
    }
}

#[test]
fn fast_tool_followed_by_stop_never_infers() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    m.apply(&ev(&stop_idle_json()), WINDOW / 3);
    assert_eq!(m.state(), State::Idle);
    assert!(!m.is_debouncing());
    assert!(!m.tick(WINDOW * 5).changed, "a cancelled debounce never fires");
}

#[test]
fn second_pre_tool_use_re_arms_window() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    m.apply(&ev(&pre_tool_json()), WINDOW / 2);
    assert!(m.is_debouncing());
    assert!(!m.tick(WINDOW).changed, "window restarts on re-arm");
    assert!(m.tick(WINDOW / 2 + WINDOW).changed);
    assert_eq!(m.state(), State::Waiting);
}

#[test]
fn post_tool_use_cancels_pending_debounce() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    assert!(m.is_debouncing());
    m.apply(&ev(&post_tool_json()), 100);
    assert!(!m.is_debouncing());
    assert!(!m.tick(WINDOW * 5).changed);
    assert_eq!(m.state(), State::Working);
}

#[test]
fn activity_auto_resolves_inferred_waiting() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    m.tick(WINDOW);
    assert_eq!(m.state(), State::Waiting);
    let t = m.apply(&ev(&pre_tool_json()), WINDOW + 500);
    assert_eq!(m.state(), State::Working);
    assert!(t.resolved_inference, "fresh activity auto-resolves the inference");
    assert!(!m.is_inferred_waiting());
}

#[test]
fn stop_resolves_inferred_waiting_to_idle() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    m.tick(WINDOW);
    assert_eq!(m.state(), State::Waiting);
    let t = m.apply(&ev(&stop_idle_json()), WINDOW + 1);
    assert_eq!(m.state(), State::Idle);
    assert!(t.resolved_inference);
    assert_eq!(m.confidence(), Confidence::Inferred);
}

#[test]
fn user_prompt_resolves_inferred_waiting_to_working() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    m.tick(WINDOW);
    assert_eq!(m.state(), State::Waiting);
    let t = m.apply(&ev(&prompt_json()), WINDOW + 1);
    assert_eq!(m.state(), State::Working);
    assert!(t.resolved_inference);
}

// ── transcript JSONL drift-guard verdicts ─────────────────────────────────────

#[test]
fn corroborate_jsonl_detects_stuck_tool() {
    let body = r#"{"message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash"}]}}"#;
    assert_eq!(corroborate_jsonl(body), Corroboration::Stuck);
}

#[test]
fn corroborate_jsonl_detects_resolved_tool() {
    let body = concat!(
        r#"{"message":{"content":[{"type":"tool_use","id":"toolu_1"}]}}"#,
        "\n",
        r#"{"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1"}]}}"#,
    );
    assert_eq!(corroborate_jsonl(body), Corroboration::Resolved);
}

#[test]
fn corroborate_jsonl_unknown_on_empty_or_drift() {
    assert_eq!(corroborate_jsonl(""), Corroboration::Unknown);
    assert_eq!(corroborate_jsonl("   \n  "), Corroboration::Unknown);
    let drifted = concat!(
        "{ this is not json\n",
        r#"{"message":{"content":[{"type":"text","text":"hi"}]}}"#,
        "\n",
        r#"{"some":"other","schema":42}"#,
    );
    assert_eq!(corroborate_jsonl(drifted), Corroboration::Unknown);
}

#[test]
fn corroborate_jsonl_bare_content_envelope() {
    let body = r#"{"content":[{"type":"tool_use","id":"t1"}]}"#;
    assert_eq!(corroborate_jsonl(body), Corroboration::Stuck);
}

#[test]
fn corroborate_transcript_is_corroborate_jsonl() {
    let body = r#"{"content":[{"type":"tool_use","id":"t1"}]}"#;
    assert_eq!(corroborate_transcript(body), corroborate_jsonl(body));
}

// ── corroboration folds into the machine without changing confidence ─────────

#[test]
fn resolved_corroboration_cancels_pending_debounce() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    assert!(m.is_debouncing());
    let t = m.corroborate(Corroboration::Resolved);
    assert!(!t.changed);
    assert!(!m.is_debouncing());
    assert!(!m.tick(WINDOW * 5).changed);
    assert_eq!(m.state(), State::Working);
    assert_ne!(m.state(), State::Waiting);
}

#[test]
fn resolved_corroboration_auto_resolves_raised_waiting() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    m.tick(WINDOW);
    assert_eq!(m.state(), State::Waiting);
    let t = m.corroborate(Corroboration::Resolved);
    assert!(t.resolved_inference);
    assert_eq!(m.state(), State::Working);
    assert_eq!(m.confidence(), Confidence::Inferred);
}

#[test]
fn stuck_and_unknown_corroboration_leave_debounce_to_timing() {
    for verdict in [Corroboration::Stuck, Corroboration::Unknown] {
        let mut m = machine();
        m.apply(&ev(&pre_tool_json()), 0);
        let t = m.corroborate(verdict);
        assert!(!t.changed, "{verdict:?} does not itself change state");
        assert!(m.is_debouncing(), "{verdict:?} leaves the debounce armed");
        assert!(m.tick(WINDOW).changed);
        assert_eq!(m.state(), State::Waiting);
        assert_eq!(m.confidence(), Confidence::Inferred);
    }
}

// ── SessionEnd is the only authoritative (high) edge ─────────────────────────

#[test]
fn session_end_is_dead_high_even_while_inferred_waiting() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    m.tick(WINDOW);
    assert_eq!(m.state(), State::Waiting);
    let t = m.apply(&ev(&session_end_json()), WINDOW + 1);
    assert_eq!(t.state, State::Dead);
    assert_eq!(t.confidence, Confidence::High, "confirmed exit is authoritative");
}

// ── to_run snapshot is correct for inferred waiting ──────────────────────────

#[test]
fn to_run_inferred_waiting_snapshot() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    m.tick(WINDOW);
    let run = m.to_run("run-1", "2026-06-08T00:00:00Z");
    assert_eq!(run.state, State::Waiting);
    assert_eq!(run.urgency, Some(Urgency::Approval));
    assert_eq!(run.confidence, Confidence::Inferred);
    assert!(run.waiting_since.is_some());
    assert_eq!(run.native_id, SID);
    assert_eq!(run.agent_kind, fleet_protocol::AgentKind::ClaudeCode);
}

// ── routing: a foreign session never mutates this machine ────────────────────

#[test]
fn foreign_session_event_is_noop() {
    let mut m = machine();
    let foreign =
        r#"{"session_id":"someone-else","hook_event_name":"PreToolUse","tool_name":"Bash"}"#;
    let t = m.apply(&ev(foreign), 0);
    assert!(!t.changed);
    assert!(!m.is_debouncing());
}

// ── adapter layer: ingest + tick ─────────────────────────────────────────────

#[test]
fn adapter_infers_waiting_via_tick() {
    let mut a = ClaudeInferAdapter::with_debounce_ms(WINDOW);
    a.ingest_json(&pre_tool_json(), 0);
    assert_eq!(a.state_of(SID), Some(State::Working));

    let cmds = a.tick(WINDOW - 1);
    assert!(
        !cmds.iter().any(|c| matches!(c, ReporterCommand::UpsertRun(r) if r.state == State::Waiting)),
        "no waiting before the window"
    );
    assert_eq!(a.state_of(SID), Some(State::Working));

    let cmds = a.tick(WINDOW);
    let run = cmds
        .iter()
        .find_map(|c| match c {
            ReporterCommand::UpsertRun(r) => Some(r),
            _ => None,
        })
        .expect("a waiting upsert");
    assert_eq!(run.state, State::Waiting);
    assert_eq!(run.urgency, Some(Urgency::Approval));
    assert_eq!(run.confidence, Confidence::Inferred);
    assert_eq!(a.confidence_of(SID), Some(Confidence::Inferred));
}

#[test]
fn adapter_corroborate_resolved_vetoes() {
    let mut a = ClaudeInferAdapter::with_debounce_ms(WINDOW);
    a.ingest_json(&pre_tool_json(), 0);
    a.corroborate(SID, Corroboration::Resolved);
    let cmds = a.tick(WINDOW);
    assert!(
        !cmds.iter().any(|c| matches!(c, ReporterCommand::UpsertRun(r) if r.state == State::Waiting)),
        "Resolved must suppress the false waiting"
    );
    assert_eq!(a.state_of(SID), Some(State::Working));
}

#[test]
fn adapter_malformed_line_creates_no_ghost() {
    let mut a = ClaudeInferAdapter::with_debounce_ms(WINDOW);
    let cmds = a.ingest_json("{ not json", 0);
    assert!(cmds.is_empty());
    assert_eq!(a.session_count(), 0);
}

#[test]
fn adapter_full_native_ui_transcript() {
    let mut a = ClaudeInferAdapter::with_debounce_ms(WINDOW);
    a.ingest_json(&prompt_json(), 0);
    assert_eq!(a.state_of(SID), Some(State::Working));
    a.ingest_json(&pre_tool_json(), 100);
    a.tick(100 + WINDOW);
    assert_eq!(a.state_of(SID), Some(State::Waiting));
    assert_eq!(a.confidence_of(SID), Some(Confidence::Inferred));
    a.ingest_json(&pre_tool_json(), 100 + WINDOW + 200);
    assert_eq!(a.state_of(SID), Some(State::Working));
    a.ingest_json(&stop_idle_json(), 100 + WINDOW + 400);
    assert_eq!(a.state_of(SID), Some(State::Idle));
}

// ── property: confidence honesty is structural (never high for waiting) ──────

mod props {
    use super::super::*;
    use super::{SID, WINDOW};
    use crate::claude::ClaudeHookEvent;
    use fleet_protocol::{Confidence, State};
    use proptest::prelude::*;

    fn any_hook_json(sid: &str) -> impl Strategy<Value = String> {
        let sid = sid.to_string();
        prop_oneof![
            Just("UserPromptSubmit"),
            Just("PreToolUse"),
            Just("PostToolUse"),
            Just("Stop"),
            Just("SessionStart"),
            Just("SessionEnd"),
            Just("SubagentStop"),
            Just("Frobnicate"),
        ]
        .prop_map(move |name| {
            format!(r#"{{"session_id":"{sid}","hook_event_name":"{name}","tool_name":"Bash"}}"#)
        })
    }

    fn any_corroboration() -> impl Strategy<Value = Corroboration> {
        prop_oneof![
            Just(Corroboration::Stuck),
            Just(Corroboration::Resolved),
            Just(Corroboration::Unknown),
        ]
    }

    proptest! {
        #[test]
        fn inferred_waiting_is_never_high(
            steps in proptest::collection::vec(
                (any_hook_json(SID), 0u64..10_000, any_corroboration(), any::<bool>()), 0..40)
        ) {
            let mut m = ClaudeInferMachine::with_debounce_ms(SID, WINDOW);
            let mut now: u64 = 0;
            for (json, dt, corr, do_corr) in &steps {
                now = now.saturating_add(*dt);
                if let Ok(e) = ClaudeHookEvent::parse(json) {
                    m.apply(&e, now);
                }
                m.tick(now);
                if *do_corr {
                    m.corroborate(*corr);
                }
                if m.state() == State::Waiting {
                    prop_assert_eq!(m.confidence(), Confidence::Inferred);
                }
            }
        }

        #[test]
        fn high_only_on_dead(
            steps in proptest::collection::vec((any_hook_json(SID), 0u64..10_000), 0..40)
        ) {
            let mut m = ClaudeInferMachine::with_debounce_ms(SID, WINDOW);
            let mut now: u64 = 0;
            for (json, dt) in &steps {
                now = now.saturating_add(*dt);
                if let Ok(e) = ClaudeHookEvent::parse(json) {
                    m.apply(&e, now);
                }
                m.tick(now);
                if m.confidence() == Confidence::High {
                    prop_assert_eq!(m.state(), State::Dead);
                }
            }
        }

        #[test]
        fn no_pre_tool_use_never_waits(times in proptest::collection::vec(0u64..10_000, 0..40)) {
            let mut m = ClaudeInferMachine::with_debounce_ms(SID, WINDOW);
            let mut now: u64 = 0;
            for dt in &times {
                now = now.saturating_add(*dt);
                let t = m.tick(now);
                prop_assert!(!t.changed);
                prop_assert_ne!(m.state(), State::Waiting);
            }
        }

        #[test]
        fn corroborate_jsonl_never_panics(body in ".*") {
            let _ = corroborate_jsonl(&body);
        }
    }
}

// ── precise tool_use_id correlation (CLINFER hardening) ──────────────────────

#[test]
fn corroborate_jsonl_for_keys_on_the_specific_tool() {
    // Two tools dispatched; only tool A resolved. The transcript's *last*
    // dispatched is B (still stuck), but if we armed on A we should see Resolved.
    let blob = [
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_A"}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_B"}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_A"}]}}"#,
    ].join("\n");

    assert_eq!(corroborate_jsonl_for(&blob, "toolu_A"), Corroboration::Resolved);
    assert_eq!(corroborate_jsonl_for(&blob, "toolu_B"), Corroboration::Stuck);
    // An id never seen → Unknown (never suppress a genuine approval on timing).
    assert_eq!(corroborate_jsonl_for(&blob, "toolu_ZZ"), Corroboration::Unknown);
    // The last-dispatched heuristic, by contrast, sees B outstanding → Stuck.
    assert_eq!(corroborate_jsonl(&blob), Corroboration::Stuck);
}

#[test]
fn machine_pins_the_armed_tool_use_id_from_pre_tool_use() {
    let mut m = ClaudeInferMachine::new(SID);
    m.apply(
        &ClaudeHookEvent::parse(&format!(
            r#"{{"session_id":"{SID}","hook_event_name":"PreToolUse","tool_name":"Read","tool_use_id":"toolu_XYZ"}}"#
        ))
        .unwrap(),
        0,
    );
    assert_eq!(m.armed_tool_use_id(), Some("toolu_XYZ"));
    // Clearing the inference (e.g. a Stop) drops the anchor.
    m.apply(
        &ClaudeHookEvent::parse(&format!(r#"{{"session_id":"{SID}","hook_event_name":"Stop"}}"#))
            .unwrap(),
        10,
    );
    assert_eq!(m.armed_tool_use_id(), None);
}

#[test]
fn adapter_corroborate_blob_auto_resolves_via_precise_anchor() {
    let mut a = ClaudeInferAdapter::new();
    // Arm on tool A, then fire the inferred waiting (past the debounce window).
    a.ingest_json(
        &format!(
            r#"{{"session_id":"{SID}","hook_event_name":"PreToolUse","tool_name":"Bash","tool_use_id":"toolu_A"}}"#
        ),
        0,
    );
    a.tick(WINDOW + 1);
    assert_eq!(a.state_of(SID), Some(State::Waiting));

    // A transcript where a LATER tool B is still outstanding but OUR armed tool A
    // resolved. Precise correlation must auto-resolve the waiting.
    let blob = [
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_A"}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_B"}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_A"}]}}"#,
    ].join("\n");
    let cmds = a.corroborate_blob(SID, &blob);
    assert_eq!(a.state_of(SID), Some(State::Working), "armed tool resolved → no longer waiting");
    assert!(
        cmds.iter().any(|c| matches!(c, ReporterCommand::UpsertRun(_))),
        "auto-resolve emits a run upsert"
    );
}
