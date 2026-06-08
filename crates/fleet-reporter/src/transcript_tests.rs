// Inline unit tests for the Claude transcript-JSONL corroboration reader
// (PLAN S16 / node CLINFER). Included from `transcript.rs` via `include!`.
//
// This is the **S16 JSONL channel** drift-guard — the test surface the G2 gate
// criterion names explicitly ("schema-drift fuzz: malformed JSONL → degrades,
// never panics or overstates"). It is distinct from the hook-JSON channel drift
// tests (`claude_fixtures::malformed_line_creates_no_ghost`,
// `codex_tests::malformed_json_is_error_not_panic`), which guard a different wire
// surface.
//
// Covers: the happy `tool_use`-without-`tool_result` corroboration; resolution
// (in-order and out-of-order); the nested vs flattened content shapes; camelCase
// drift; per-line isolation; and a proptest schema-drift fuzz over arbitrary bytes
// asserting the two locked invariants — NEVER panics, NEVER overstates.

use super::*;
use proptest::prelude::*;

// ── happy path: an unmatched tool_use corroborates waiting ───────────────────

#[test]
fn unmatched_tool_use_is_outstanding() {
    let text = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu_1","name":"Bash"}]}}"#;
    let c = corroborate(text);
    assert!(c.suggests_waiting());
    assert!(c.outstanding.contains("tu_1"));
    assert_eq!(c.confidence(), Confidence::Inferred);
    assert!(!c.is_uninformative());
}

#[test]
fn matched_tool_use_is_not_outstanding() {
    let text = concat!(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu_1"}]}}"#,
        "\n",
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu_1"}]}}"#,
    );
    let c = corroborate(text);
    assert!(!c.suggests_waiting(), "a resolved tool_use must not be outstanding");
    assert!(c.outstanding.is_empty());
}

#[test]
fn out_of_order_result_still_cancels() {
    // The tool_result appears BEFORE its tool_use in the window — resolution must
    // still win so we never strand a phantom approval.
    let text = concat!(
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu_9"}]}}"#,
        "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"tu_9"}]}}"#,
    );
    let c = corroborate(text);
    assert!(!c.suggests_waiting(), "out-of-order result must still cancel");
}

#[test]
fn multiple_uses_one_resolved_one_pending() {
    let text = concat!(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"a"},{"type":"tool_use","id":"b"}]}}"#,
        "\n",
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a"}]}}"#,
    );
    let c = corroborate(text);
    assert_eq!(c.outstanding.iter().cloned().collect::<Vec<_>>(), vec!["b".to_string()]);
}

// ── tolerated shape variants (drift defensiveness) ───────────────────────────

#[test]
fn flattened_top_level_content_is_understood() {
    // Some recorded transcripts flatten `content` to the top level (no `message`).
    let text = r#"{"type":"assistant","content":[{"type":"tool_use","id":"flat_1"}]}"#;
    let c = corroborate(text);
    assert!(c.outstanding.contains("flat_1"));
}

#[test]
fn camelcase_tool_use_id_alias_resolves() {
    let text = concat!(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"x"}]}}"#,
        "\n",
        r#"{"type":"user","message":{"content":[{"type":"tool_result","toolUseId":"x"}]}}"#,
    );
    let c = corroborate(text);
    assert!(!c.suggests_waiting(), "camelCase toolUseId must resolve the call");
}

#[test]
fn plain_text_lines_are_understood_but_inert() {
    let text = concat!(
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#,
        "\n",
        r#"{"type":"system","subtype":"init"}"#,
    );
    let c = corroborate(text);
    assert!(!c.suggests_waiting());
    assert_eq!(c.understood_lines, 2, "both lines parse, neither corroborates");
    assert_eq!(c.skipped_lines, 0);
}

// ── DRIFT: a tool_use whose id field drifted away is NOT counted ──────────────

#[test]
fn drifted_tool_use_without_id_is_not_overstated() {
    // Confidence honesty: a tool_use we cannot anchor must NOT become a phantom
    // outstanding approval — we never overstate what we can't prove.
    let text = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#;
    let c = corroborate(text);
    assert!(
        !c.suggests_waiting(),
        "an un-anchored tool_use must not be reported as outstanding"
    );
}

// ── per-line isolation: a malformed line never poisons its neighbours ─────────

#[test]
fn malformed_line_is_skipped_not_fatal() {
    let text = concat!(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"keep"}]}}"#,
        "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"trunc""#, // truncated/malformed
        "\n",
        r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"other"}]}}"#,
    );
    let c = corroborate(text);
    // The good lines on either side of the bad one are still processed.
    assert!(c.outstanding.contains("keep"));
    assert_eq!(c.skipped_lines, 1, "exactly the one malformed line is skipped");
    assert!(c.understood_lines >= 2);
}

#[test]
fn blank_lines_are_not_drift() {
    let text = concat!(
        "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"z"}]}}"#,
        "\n\n",
    );
    let c = corroborate(text);
    assert!(c.outstanding.contains("z"));
    assert_eq!(c.skipped_lines, 0, "blank lines are not counted as drift");
}

#[test]
fn non_object_json_line_is_skipped() {
    // A bare array / number / string / null is drift, not a transcript object.
    for junk in ["[1,2,3]", "42", "\"hello\"", "null", "true"] {
        let c = corroborate(junk);
        assert_eq!(c.skipped_lines, 1, "{junk} should be skipped");
        assert!(c.outstanding.is_empty());
        assert!(c.is_uninformative(), "{junk} yields no understood line");
    }
}

#[test]
fn empty_input_is_uninformative_not_waiting() {
    let c = corroborate("");
    assert!(c.is_uninformative());
    assert!(!c.suggests_waiting());
    assert_eq!(c.skipped_lines, 0);
}

#[test]
fn wholly_garbage_transcript_degrades_to_uninformative() {
    let text = "}{not json\n<<<\n\u{0}\u{1}garbage\nalso not json";
    let c = corroborate(text);
    assert!(c.is_uninformative(), "no line understood");
    assert!(!c.suggests_waiting(), "garbage must never claim waiting");
    assert!(c.skipped_lines >= 1);
}

// ── confidence honesty is structural ─────────────────────────────────────────

#[test]
fn confidence_is_always_inferred() {
    // No transcript content can ever raise transcript confidence above Inferred —
    // it is the inferred channel by construction (invariant 5).
    let with_use = corroborate(
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"q"}]}}"#,
    );
    let empty = corroborate("");
    assert_eq!(with_use.confidence(), Confidence::Inferred);
    assert_eq!(empty.confidence(), Confidence::Inferred);
}

// ── SCHEMA-DRIFT FUZZ (the G2-named test) ────────────────────────────────────
//
// "malformed JSONL → degrades, never panics or overstates." We fuzz arbitrary
// byte-ish line content (including JSON fragments, control chars, and partial
// objects) and assert the two locked invariants on EVERY input:
//   (1) corroborate() NEVER panics; and
//   (2) it NEVER overstates — any id it reports outstanding must be an id that
//       genuinely appears as an *anchored, unresolved* tool_use in the input.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2048))]

    // (1) Arbitrary text never panics and never invents outstanding ids that are
    // not literally present somewhere in the input.
    #[test]
    fn fuzz_arbitrary_text_never_panics_or_invents(text in ".{0,400}") {
        let c = corroborate(&text);
        for id in &c.outstanding {
            prop_assert!(
                text.contains(id.as_str()),
                "reported outstanding id {id:?} that is absent from the input"
            );
        }
        // understood + skipped never exceeds the number of non-empty lines.
        let nonempty = text.lines().filter(|l| !l.trim().is_empty()).count();
        prop_assert!(c.understood_lines + c.skipped_lines <= nonempty);
    }

    // (2) Drift fuzz over *structured-but-corrupted* JSONL: build lines that look
    // like transcript objects with randomly present/absent id fields and random
    // truncation, and assert the no-overstate invariant — a tool_use is reported
    // outstanding ONLY when its id is present AND no tool_result for it appears.
    #[test]
    fn fuzz_corrupted_jsonl_never_overstates(
        lines in proptest::collection::vec(corrupt_line_strategy(), 0..12)
    ) {
        let text = lines.join("\n");
        let c = corroborate(&text);

        // Recompute the ground truth independently from the same lines, parsing
        // each defensively, to assert corroborate() agrees (never broader).
        let mut expected_uses: std::collections::BTreeSet<String> = Default::default();
        let mut expected_results: std::collections::BTreeSet<String> = Default::default();
        for line in text.lines() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) {
                collect_truth(&v, &mut expected_uses, &mut expected_results);
            }
        }
        let expected_outstanding: std::collections::BTreeSet<String> =
            expected_uses.difference(&expected_results).cloned().collect();

        prop_assert_eq!(
            &c.outstanding, &expected_outstanding,
            "corroborate() must equal seen-and-anchored-minus-resolved (no over/understatement)"
        );
    }
}

// A strategy producing lines that resemble transcript JSONL but may be corrupted:
// well-formed tool_use / tool_result objects, the same with the id field dropped,
// truncated objects, blanks, and pure junk.
fn corrupt_line_strategy() -> impl Strategy<Value = String> {
    let id = "tu_[a-c]"; // tiny id space so collisions/resolutions actually occur
    prop_oneof![
        // anchored tool_use
        id.prop_map(|i| format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{i}"}}]}}}}"#
        )),
        // tool_result resolving an id
        id.prop_map(|i| format!(
            r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{i}"}}]}}}}"#
        )),
        // tool_use with the id field DROPPED (drift) — must not be counted
        Just(r#"{"type":"assistant","message":{"content":[{"type":"tool_use"}]}}"#.to_string()),
        // truncated / malformed object
        id.prop_map(|i| format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{i}""#
        )),
        // inert recognised line
        Just(r#"{"type":"system","subtype":"init"}"#.to_string()),
        // non-object junk
        Just("[1,2,3]".to_string()),
        Just(String::new()),
        Just("not json at all".to_string()),
    ]
}

// Mirror of the reader's anchoring rules, used by the fuzz to compute ground truth
// independently (so the test isn't tautological with the implementation).
fn collect_truth(
    v: &serde_json::Value,
    uses: &mut std::collections::BTreeSet<String>,
    results: &mut std::collections::BTreeSet<String>,
) {
    let blocks = v
        .get("message")
        .and_then(|m| m.get("content"))
        .or_else(|| v.get("content"))
        .and_then(|c| c.as_array());
    let Some(blocks) = blocks else { return };
    for b in blocks {
        match b.get("type").and_then(|t| t.as_str()) {
            Some("tool_use") => {
                if let Some(id) = b.get("id").and_then(|i| i.as_str()).filter(|s| !s.is_empty()) {
                    uses.insert(id.to_string());
                }
            }
            Some("tool_result") => {
                let id = b
                    .get("tool_use_id")
                    .or_else(|| b.get("toolUseId"))
                    .and_then(|i| i.as_str())
                    .filter(|s| !s.is_empty());
                if let Some(id) = id {
                    results.insert(id.to_string());
                }
            }
            _ => {}
        }
    }
}
