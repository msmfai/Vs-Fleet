//! Integration tests: the Claude transcript-JSONL corroboration reader vs
//! **recorded transcript-JSONL fixtures** on disk (the engineering spec ; gate
//! G2 "each adapter vs recorded fixtures … transcript JSONL").
//!
//! These do NOT require a real claude/VS Code install — the fixtures under
//! `tests/fixtures/transcript/` are recorded Claude transcript lines (one JSON
//! object per line: `type`, nested `message.content` blocks of `tool_use` /
//! `tool_result`). We replay whole transcripts through the public `corroborate`
//! API and assert the corroboration verdict.
//!
//! This is the **S16 JSONL channel** — distinct from the hook-JSON channel covered
//! by `claude_fixtures.rs`. The G2 JSONL drift-guard
//! ("malformed JSONL → degrades, never panics or overstates") is the
//! `drifted.jsonl` case below plus the proptest fuzz inside
//! `transcript::tests::fuzz_*`.

use std::path::PathBuf;

use fleet_protocol::Confidence;
use fleet_reporter::transcript::corroborate;

fn fixture(name: &str) -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/transcript");
    p.push(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {p:?}: {e}"))
}

#[test]
fn pending_approval_transcript_corroborates_waiting() {
    let c = corroborate(&fixture("pending_approval.jsonl"));
    assert!(
        c.suggests_waiting(),
        "an issued tool_use with no tool_result corroborates waiting"
    );
    assert!(c.outstanding.contains("toolu_01abc"));
    assert_eq!(
        c.confidence(),
        Confidence::Inferred,
        "S16 is always inferred"
    );
    assert_eq!(c.skipped_lines, 0, "the recorded transcript is well-formed");
}

#[test]
fn resolved_transcript_does_not_corroborate_waiting() {
    let c = corroborate(&fixture("resolved.jsonl"));
    assert!(
        !c.suggests_waiting(),
        "a tool_use with a matching tool_result must not corroborate waiting"
    );
    assert!(c.outstanding.is_empty());
    assert_eq!(c.skipped_lines, 0);
}

#[test]
fn drifted_transcript_degrades_without_panic_or_overstatement() {
    // The G2 JSONL drift-guard against a recorded fixture: one truncated line, one
    // tool_use whose id field drifted to `tool_call_id`, and an unrelated result.
    let c = corroborate(&fixture("drifted.jsonl"));

    // The well-anchored tool_use is still corroborated...
    assert!(c.outstanding.contains("toolu_keep"));
    // ...the drifted-id tool_use is NOT overstated as outstanding...
    assert!(
        !c.outstanding.contains("toolu_drifted"),
        "a tool_use whose id field drifted must not be claimed as outstanding"
    );
    // ...the unrelated tool_result resolves nothing it shouldn't...
    assert!(!c.outstanding.contains("toolu_unrelated"));
    // ...and the malformed/truncated line is skipped, not fatal.
    assert_eq!(c.skipped_lines, 1, "exactly the truncated line is skipped");
    assert_eq!(
        c.confidence(),
        Confidence::Inferred,
        "drift never upgrades confidence"
    );
}
