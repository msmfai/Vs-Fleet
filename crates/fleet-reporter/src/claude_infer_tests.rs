// Inline unit tests for the S16 inferred-waiting Claude adapter ,
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
    assert_eq!(m.state(), State::Done, "a real Stop ends the turn → done");
    assert!(!m.is_debouncing());
    assert!(!m.tick(WINDOW * 5).changed, "a cancelled debounce never fires");
}

#[test]
fn continuation_stop_stays_working_and_cancels_debounce() {
    // A Stop with stop_hook_active=true is NOT a real turn boundary — it is a
    // continuation → stays working (a liveness ping) and cancels any pending arm.
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    assert!(m.is_debouncing());
    let cont = format!(
        r#"{{"session_id":"{SID}","hook_event_name":"Stop","stop_hook_active":true}}"#
    );
    let t = m.apply(&ev(&cont), 100);
    assert_eq!(m.state(), State::Working);
    assert!(t.liveness);
    assert!(!m.is_debouncing(), "the continuation cancels the pending debounce");
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
fn stop_resolves_inferred_waiting_to_done() {
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    m.tick(WINDOW);
    assert_eq!(m.state(), State::Waiting);
    let t = m.apply(&ev(&stop_idle_json()), WINDOW + 1);
    assert_eq!(m.state(), State::Done, "a real Stop ends the turn → done");
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
    assert_eq!(a.state_of(SID), Some(State::Done), "a real Stop → done");
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

// ── coverage gap-closing: drift-guard skip arms, accessors, Display/Default ──

#[test]
fn corroborate_jsonl_skips_blocks_without_type() {
    // A block with no `type` key must be skipped (the `continue` at the block
    // level), but a later well-formed tool_use still yields a real verdict.
    let body = concat!(
        r#"{"message":{"content":[{"id":"no_type_here","name":"Bash"}]}}"#,
        "\n",
        r#"{"message":{"content":[{"type":"tool_use","id":"toolu_after"}]}}"#,
    );
    assert_eq!(corroborate_jsonl(body), Corroboration::Stuck);
}

#[test]
fn corroborate_jsonl_unknown_when_only_tool_result_seen() {
    // A bare `tool_result` with no preceding `tool_use`: we *saw a tool* (so we
    // don't bail early) but `dispatched` is empty → the `None` arm → Unknown.
    let body = r#"{"message":{"content":[{"type":"tool_result","tool_use_id":"orphan_result"}]}}"#;
    assert_eq!(corroborate_jsonl(body), Corroboration::Unknown);
}

#[test]
fn corroborate_jsonl_for_drift_guard_skips_all_bad_lines() {
    // Exercise every skip arm of corroborate_jsonl_for: blank line, unparseable
    // JSON, a non-array `content`, and a block with no `type`. Then a real
    // tool_use anchored on our id so the verdict is positively Stuck.
    let body = concat!(
        "   \n",                                              // blank → skip
        "{ not valid json at all\n",                          // parse err → skip
        r#"{"message":{"content":"not-an-array"}}"#,          // non-array → skip
        "\n",
        r#"{"message":{"content":[{"id":"x","name":"Bash"}]}}"#, // no type → skip
        "\n",
        r#"{"message":{"content":[{"type":"tool_use","id":"toolu_keyed"}]}}"#,
    );
    assert_eq!(corroborate_jsonl_for(body, "toolu_keyed"), Corroboration::Stuck);
    // And the keyed lookup degrades to Unknown when the id is never dispatched.
    assert_eq!(corroborate_jsonl_for(body, "toolu_missing"), Corroboration::Unknown);
}

#[test]
fn machine_accessors_reflect_state() {
    // Cover session_id(), urgency(), cwd(), debounce_ms() accessors with real
    // values driven through the machine.
    let mut m = ClaudeInferMachine::with_debounce_ms("sess-accessor", 777);
    assert_eq!(m.session_id(), "sess-accessor");
    assert_eq!(m.debounce_ms(), 777);
    assert_eq!(m.cwd(), "/", "default cwd before any cwd-bearing event");
    assert_eq!(m.urgency(), None);

    // A PreToolUse carrying cwd updates cwd; firing the debounce sets urgency.
    m.apply(
        &ev(
            r#"{"session_id":"sess-accessor","cwd":"/work/here","hook_event_name":"PreToolUse","tool_name":"Bash"}"#,
        ),
        0,
    );
    assert_eq!(m.cwd(), "/work/here");
    m.tick(777);
    assert_eq!(m.urgency(), Some(Urgency::Approval));
    assert_eq!(m.state(), State::Waiting);
}

#[test]
fn real_stop_goes_done() {
    // A real Stop (stop_hook_active=false) → State::Done and the Done last_message.
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    let t = m.apply(&ev(&stop_idle_json()), 100);
    assert_eq!(t.state, State::Done);
    assert!(t.changed);
    let run = m.to_run("run-done", "2026-06-24T00:00:00Z");
    assert_eq!(run.state, State::Done);
    assert_eq!(run.last_message.as_deref(), Some("Task complete."));
}

#[test]
fn waiting_last_message_without_tool_is_generic() {
    // Waiting state with no last_tool → the `None` arm of last_message (line 516).
    // Arm without a tool_name so last_tool stays None, then fire the debounce.
    let mut m = machine();
    let pre_no_tool = format!(r#"{{"session_id":"{SID}","hook_event_name":"PreToolUse"}}"#);
    m.apply(&ev(&pre_no_tool), 0);
    m.tick(WINDOW);
    assert_eq!(m.state(), State::Waiting);
    let run = m.to_run("run-w", "2026-06-24T00:00:00Z");
    assert_eq!(run.last_message.as_deref(), Some("Possibly waiting (inferred)"));
}

#[test]
fn waiting_last_message_with_tool_names_it() {
    // Waiting WITH a last_tool → the Some arm of the Waiting last_message branch.
    let mut m = machine();
    m.apply(&ev(&pre_tool_json()), 0);
    m.tick(WINDOW);
    let run = m.to_run("run-wt", "2026-06-24T00:00:00Z");
    assert_eq!(
        run.last_message.as_deref(),
        Some("Possibly waiting on Bash (inferred)")
    );
}

#[test]
fn dead_last_message_on_session_end() {
    // SessionEnd → Dead → the Dead arm of last_message (line 520).
    let mut m = machine();
    m.apply(&ev(&session_end_json()), 0);
    assert_eq!(m.state(), State::Dead);
    let run = m.to_run("run-dead", "2026-06-24T00:00:00Z");
    assert_eq!(run.last_message.as_deref(), Some("Session closed."));
    assert_eq!(run.confidence, Confidence::High);
}

#[test]
fn adapter_default_matches_new() {
    // Default impl (lines 544-546) must equal ::new() behavior.
    let a = ClaudeInferAdapter::default();
    assert_eq!(a.session_count(), 0);
    // And it actually tracks sessions like a fresh adapter.
    let mut a = ClaudeInferAdapter::default();
    a.ingest_json(&pre_tool_json(), 0);
    assert_eq!(a.session_count(), 1);
    assert_eq!(a.state_of(SID), Some(State::Working));
}

#[test]
fn adapter_run_id_and_machine_accessors() {
    // run_id_of (588-590) and machine_of (593-595) for tracked + untracked ids.
    let mut a = ClaudeInferAdapter::with_debounce_ms(WINDOW);
    assert_eq!(a.run_id_of(SID), None, "untracked → None");
    assert!(a.machine_of(SID).is_none(), "untracked → None");

    a.ingest_json(&pre_tool_json(), 0);
    let run_id = a.run_id_of(SID).expect("tracked run id");
    assert!(run_id.starts_with("claude:"), "minted run id: {run_id}");
    assert!(run_id.contains(SID));
    let machine = a.machine_of(SID).expect("tracked machine");
    assert_eq!(machine.state(), State::Working);
    assert_eq!(machine.session_id(), SID);
}

#[test]
fn adapter_corroborate_unknown_session_is_empty() {
    // corroborate on an unknown session → the None arm returns no commands (648).
    let mut a = ClaudeInferAdapter::with_debounce_ms(WINDOW);
    let cmds = a.corroborate("nope-not-here", Corroboration::Resolved);
    assert!(cmds.is_empty());
}

#[test]
fn adapter_corroborate_blob_unknown_session_is_empty() {
    // corroborate_blob on an unknown session → the None arm (664).
    let mut a = ClaudeInferAdapter::with_debounce_ms(WINDOW);
    let blob = r#"{"message":{"content":[{"type":"tool_use","id":"t1"}]}}"#;
    let cmds = a.corroborate_blob("ghost-session", blob);
    assert!(cmds.is_empty());
}

#[test]
fn adapter_corroborate_blob_falls_back_to_last_dispatched() {
    // When the armed PreToolUse carried NO tool_use_id, corroborate_blob falls
    // back to corroborate_jsonl (line 662, the None arm of armed_tool_use_id).
    // A Resolved transcript then auto-resolves the raised waiting.
    let mut a = ClaudeInferAdapter::with_debounce_ms(WINDOW);
    a.ingest_json(&pre_tool_json(), 0); // pre_tool_json has NO tool_use_id
    a.tick(WINDOW);
    assert_eq!(a.state_of(SID), Some(State::Waiting));
    assert!(
        a.machine_of(SID).unwrap().armed_tool_use_id().is_none(),
        "no anchor pinned, so the last-dispatched heuristic is used"
    );
    let blob = concat!(
        r#"{"message":{"content":[{"type":"tool_use","id":"t1"}]}}"#,
        "\n",
        r#"{"message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]}}"#,
    );
    let cmds = a.corroborate_blob(SID, blob);
    assert_eq!(a.state_of(SID), Some(State::Working), "last-dispatched resolved → veto");
    assert!(cmds.iter().any(|c| matches!(c, ReporterCommand::UpsertRun(_))));
}

#[test]
fn adapter_forget_removes_session() {
    // forget (670-672): removes a tracked session, returns false for unknown.
    let mut a = ClaudeInferAdapter::with_debounce_ms(WINDOW);
    a.ingest_json(&pre_tool_json(), 0);
    assert_eq!(a.session_count(), 1);
    assert!(a.forget(SID), "forgetting a tracked session returns true");
    assert_eq!(a.session_count(), 0);
    assert!(!a.forget(SID), "forgetting an unknown session returns false");
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
