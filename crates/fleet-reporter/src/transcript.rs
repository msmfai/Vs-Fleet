//! Claude **transcript-JSONL** corroboration reader.
//!
//! This is the S16 *inferred-waiting* corroboration channel. It is **not** the
//! hook-JSON channel (that is [`crate::claude`] / [`crate::claude_shim`]). The two
//! are distinct wire surfaces and must be guarded distinctly:
//!
//! - **Hook-JSON channel** (S15/S17): one JSON object per hook invocation,
//!   delivered on the hook stdin/socket. Drift-guarded by
//!   [`ClaudeHookEvent::parse`](crate::claude::ClaudeHookEvent::parse) and proven
//!   by `claude_fixtures::malformed_line_creates_no_ghost` /
//!   `codex_tests::malformed_json_is_error_not_panic`.
//! - **Transcript-JSONL channel** (S16, *this module*): the append-only
//!   `transcript_path` file Claude writes, **one JSON object per line**, that the
//!   native-UI inference path reads to *corroborate* a `PreToolUse`-without-`Stop`
//!   debounce — specifically, a `tool_use` content block with **no matching
//!   `tool_result`** means the agent is blocked awaiting the tool's
//!   completion/approval. This is the only place that JSONL is parsed,
//!   so the G2 drift-guard ("malformed JSONL → degrades, never panics or
//!   overstates") lives and is exercised **here**.
//!
//! ## Why a separate, *best-effort, schema-drift-guarded* reader
//!
//! The transcript line schema is **community-documented and version-sensitive**
//! The engineering spec notes: the field that anchors a tool call (`tool_use.id`) and the
//! field that resolves it (`tool_result.tool_use_id`) are *not* a stable public
//! contract. So this reader **parses best-effort behind a schema-drift guard that
//! degrades gracefully rather than mis-stating**. Concretely:
//!
//! 1. **Per-line isolation.** A malformed/blank/non-object line is *skipped*, never
//!    fatal — a drifted line in the middle of a transcript never discards the lines
//!    around it and never panics.
//! 2. **Never overstate.** Corroboration is *monotone toward caution*: the reader
//!    only ever reports `tool_use` ids it can both **see** *and* **anchor**. A
//!    `tool_use` whose id field drifted away is **not** counted as outstanding
//!    (could neither be matched nor mis-claimed) — confidence honesty: we never
//!    upgrade to a stronger signal than we can prove. The result is always
//!    [`Confidence::Inferred`] (S16 is the inferred path by construction).
//! 3. **Resolution wins ties.** A `tool_result` for an id always cancels that id's
//!    outstanding `tool_use`, regardless of line order within the scanned window,
//!    so an out-of-order or duplicated result never strands a phantom approval.
//!
//! The module is pure and sync (no I/O beyond the caller handing us the text), so
//! every behaviour is exhaustively unit-testable against **recorded transcript
//! JSONL fixtures** and a **schema-drift fuzz** that asserts the no-panic /
//! no-overstate invariants on arbitrary byte input.

use std::collections::BTreeSet;

use serde::Deserialize;

use fleet_protocol::Confidence;

/// The corroboration verdict the transcript reader produces for a window of
/// transcript-JSONL text. It deliberately carries **no** state-machine authority —
/// it is a *corroboration signal* the S16 inference path consults alongside the
/// `PreToolUse`-without-`Stop` debounce, never a standalone state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corroboration {
    /// The set of `tool_use` ids that were seen **and anchored** but have **no**
    /// matching `tool_result` — i.e. tool calls the agent is currently blocked on.
    /// Empty ⇒ no outstanding call ⇒ no waiting corroboration.
    pub outstanding: BTreeSet<String>,
    /// How many JSONL lines were skipped because they were blank, non-JSON,
    /// non-object, or otherwise un-anchorable. Surfaced for telemetry/drift
    /// monitoring; a non-zero count is *not* an error.
    pub skipped_lines: usize,
    /// How many lines were understood (parsed to a known transcript shape). Used by
    /// callers to decide whether the transcript is trustworthy enough to act on.
    pub understood_lines: usize,
}

impl Corroboration {
    /// `true` when at least one anchored `tool_use` is outstanding — the
    /// corroborating condition for S16 `waiting`+`approval`.
    pub fn suggests_waiting(&self) -> bool {
        !self.outstanding.is_empty()
    }

    /// The confidence S16 corroboration can ever carry: **always**
    /// [`Confidence::Inferred`]. This is structural — the transcript channel has no
    /// authoritative-signal path, so it can never produce `High` (invariant 5,
    /// confidence honesty). Exposed as a method so callers can't accidentally pair
    /// transcript corroboration with a stronger confidence.
    pub fn confidence(&self) -> Confidence {
        Confidence::Inferred
    }

    /// `true` when no line in the window parsed to a recognised transcript shape.
    /// In that case the reader has *no* basis to corroborate anything and the
    /// caller must fall back to the debounce alone — it must **not** treat an
    /// unreadable transcript as evidence of either waiting or not-waiting.
    pub fn is_uninformative(&self) -> bool {
        self.understood_lines == 0
    }
}

/// One transcript line's recognised shape. Only the two content-block kinds S16
/// needs (`tool_use`, `tool_result`) are modelled; every other recognised line is
/// [`Line::Other`] (counted as understood but carrying no corroboration), and an
/// unparseable line is dropped before it reaches this enum.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    /// An assistant turn that issued one or more anchored tool calls.
    ToolUses(Vec<String>),
    /// A user/tool turn that resolved one or more tool calls.
    ToolResults(Vec<String>),
    /// A recognised line with no tool-call relevance (plain text, system, etc.).
    Other,
}

/// Raw transcript line shape. Claude transcript lines are objects with a `type`
/// (`"assistant"`, `"user"`, `"system"`, …) and (for assistant/user turns) a
/// `message` whose `content` is an array of typed blocks. We tolerate **both** the
/// nested `message.content` shape and a flattened top-level `content` shape, and
/// both `snake_case` and `camelCase` id spellings — defensively, because the line
/// schema is not a stable contract.
#[derive(Debug, Deserialize)]
struct RawLine {
    #[serde(default, alias = "role")]
    r#type: Option<String>,
    #[serde(default)]
    message: Option<RawMessage>,
    #[serde(default)]
    content: Option<Vec<RawBlock>>,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    #[serde(default)]
    content: Option<Vec<RawBlock>>,
}

/// A single content block. `type` selects the variant; the id fields differ per
/// variant and may have drifted away entirely (then the block is simply un-anchored
/// and contributes nothing — never panics, never over-claims).
#[derive(Debug, Deserialize)]
struct RawBlock {
    #[serde(default)]
    r#type: Option<String>,
    /// `tool_use.id` — the tool call's anchor.
    #[serde(default)]
    id: Option<String>,
    /// `tool_result.tool_use_id` — the id this result resolves.
    #[serde(default, alias = "toolUseId")]
    tool_use_id: Option<String>,
}

impl RawLine {
    fn classify(self) -> Option<Line> {
        // A line with neither a nested nor a flattened content array is a
        // recognised-but-irrelevant line (e.g. a summary/system marker) IFF it at
        // least had a `type`; a totally shapeless object is treated as Other too so
        // it counts as "understood, no corroboration" rather than a parse failure —
        // serde already proved it was a JSON object.
        let blocks = self
            .message
            .and_then(|m| m.content)
            .or(self.content)
            .unwrap_or_default();

        let _ = self.r#type; // type is informational; tool blocks self-identify.

        let mut uses = Vec::new();
        let mut results = Vec::new();
        for b in blocks {
            match b.r#type.as_deref() {
                Some("tool_use") => {
                    // Only anchor a tool_use we can name. A drifted/absent id is
                    // dropped: it can be neither matched nor mis-claimed.
                    if let Some(id) = b.id.filter(|s| !s.is_empty()) {
                        uses.push(id);
                    }
                }
                Some("tool_result") => {
                    if let Some(id) = b.tool_use_id.filter(|s| !s.is_empty()) {
                        results.push(id);
                    }
                }
                _ => {}
            }
        }

        if !uses.is_empty() {
            Some(Line::ToolUses(uses))
        } else if !results.is_empty() {
            Some(Line::ToolResults(results))
        } else {
            Some(Line::Other)
        }
    }
}

/// Parse one transcript-JSONL **line** best-effort. Returns `None` for a line that
/// is blank or not a JSON object (drift / partial write) — never panics, never
/// errors out the whole scan. A recognised line returns its [`Line`] shape.
fn parse_line(line: &str) -> Option<Line> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // Must be a JSON *object*. A bare array/number/string/`null` is drift, skipped.
    let raw: RawLine = serde_json::from_str(line).ok()?;
    raw.classify()
}

/// Scan a window of transcript-JSONL text and produce the [`Corroboration`].
///
/// **Drift-guarded by construction**: each line is parsed in isolation;
/// a malformed line increments `skipped_lines` and is otherwise ignored, so the
/// scan **degrades gracefully and never panics**. The outstanding set only contains
/// `tool_use` ids that were both seen and anchored *and* never resolved by a
/// `tool_result` anywhere in the window — so the reader **never overstates** a
/// pending approval.
///
/// `text` may be a whole transcript, a tail window, or a single appended line; the
/// result is order-insensitive for resolution (a `tool_result` cancels its id
/// wherever it appears in the window).
pub fn corroborate(text: &str) -> Corroboration {
    let mut outstanding: BTreeSet<String> = BTreeSet::new();
    let mut resolved: BTreeSet<String> = BTreeSet::new();
    let mut skipped = 0usize;
    let mut understood = 0usize;

    for line in text.lines() {
        match parse_line(line) {
            None => {
                // Blank lines are not drift; only count genuinely unparseable,
                // non-empty lines as skipped so the telemetry means something.
                if !line.trim().is_empty() {
                    skipped += 1;
                }
            }
            Some(parsed) => {
                understood += 1;
                match parsed {
                    Line::ToolUses(ids) => {
                        for id in ids {
                            // A result already seen for this id wins (out-of-order).
                            if !resolved.contains(&id) {
                                outstanding.insert(id);
                            }
                        }
                    }
                    Line::ToolResults(ids) => {
                        for id in ids {
                            outstanding.remove(&id);
                            resolved.insert(id);
                        }
                    }
                    Line::Other => {}
                }
            }
        }
    }

    Corroboration {
        outstanding,
        skipped_lines: skipped,
        understood_lines: understood,
    }
}

#[cfg(test)]
mod tests {
    include!("transcript_tests.rs");
}
