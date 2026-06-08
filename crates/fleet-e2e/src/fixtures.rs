//! Recorded agent-hook fixtures for the DoD suite.
//!
//! These are **recorded** Codex / Claude hook-event payloads and Claude
//! transcript-JSONL bodies — the exact wire shapes the real adapters
//! ([`fleet_reporter::CodexAdapter`], [`fleet_reporter::ClaudeAdapter`],
//! [`fleet_reporter::ClaudeInferMachine`], [`fleet_reporter::ClaudeShimAdapter`])
//! parse. The DoD tests drive the real adapters with these, so the integration
//! suite is fixture-driven and requires no live agent (§3 invariant 3:
//! observer-not-owner; §21: "use fixtures + mocked OS focus").
//!
//! The field vocabulary matches the adapters' parsers verbatim: `hook_event_name`,
//! `session_id` (Codex `thread.id` / Claude `session_id`), `cwd`, `tool_name`,
//! `turn_id`, the `decision`/`permission` approval envelope, the `stop_hook_active`
//! / completion markers. Keeping them as `const &str` (not generated) means a
//! reviewer can read exactly what the agent "said".

// ── Codex hook fixtures (PLAN S11–S13; default hooks-first path) ───────────────

/// Codex `SessionStart` — a thread opened. `session_id` is the durable `thread.id`.
pub fn codex_session_start(thread: &str, cwd: &str) -> String {
    format!(r#"{{"hook_event_name":"SessionStart","session_id":"{thread}","cwd":"{cwd}"}}"#)
}

/// Codex `UserPromptSubmit` — the user submitted a prompt → working.
pub fn codex_prompt(thread: &str, cwd: &str) -> String {
    format!(
        r#"{{"hook_event_name":"UserPromptSubmit","session_id":"{thread}","turn_id":"t1","cwd":"{cwd}"}}"#
    )
}

/// Codex `PreToolUse` — the agent is about to run a tool → working + liveness.
pub fn codex_pre_tool(thread: &str, cwd: &str, tool: &str) -> String {
    format!(
        r#"{{"hook_event_name":"PreToolUse","session_id":"{thread}","turn_id":"t1","cwd":"{cwd}","tool_name":"{tool}"}}"#
    )
}

/// Codex `PermissionRequest` — the agent is blocked on an approval.
/// Authoritative `waiting`+`approval`, confidence **high** (the only Codex `high`
/// waiting path).
pub fn codex_permission_request(thread: &str, cwd: &str, tool: &str) -> String {
    format!(
        r#"{{"hook_event_name":"PermissionRequest","session_id":"{thread}","turn_id":"t1","cwd":"{cwd}","tool_name":"{tool}"}}"#
    )
}

/// Codex `PermissionRequest` **response** — the user answered in the terminal.
/// `allow`/`deny` both auto-resolve the run back to `working` (§21.4).
pub fn codex_permission_response(thread: &str, cwd: &str, tool: &str, allow: bool) -> String {
    let decision = if allow { "allow" } else { "deny" };
    format!(
        r#"{{"hook_event_name":"PermissionRequest","session_id":"{thread}","turn_id":"t1","cwd":"{cwd}","tool_name":"{tool}","decision":"{decision}"}}"#
    )
}

/// Codex `Stop` — the turn finished (idle).
pub fn codex_stop(thread: &str, cwd: &str) -> String {
    format!(r#"{{"hook_event_name":"Stop","session_id":"{thread}","cwd":"{cwd}"}}"#)
}

/// Codex `SessionEnd` — the thread closed → dead.
pub fn codex_session_end(thread: &str, cwd: &str) -> String {
    format!(r#"{{"hook_event_name":"SessionEnd","session_id":"{thread}","cwd":"{cwd}"}}"#)
}

// ── Claude hook fixtures (PLAN S15–S17) ───────────────────────────────────────

/// Claude `SessionStart` — a session opened → idle.
pub fn claude_session_start(session: &str, cwd: &str) -> String {
    format!(r#"{{"hook_event_name":"SessionStart","session_id":"{session}","cwd":"{cwd}"}}"#)
}

/// Claude `UserPromptSubmit` → working.
pub fn claude_prompt(session: &str, cwd: &str) -> String {
    format!(r#"{{"hook_event_name":"UserPromptSubmit","session_id":"{session}","cwd":"{cwd}"}}"#)
}

/// Claude `PreToolUse` → working (+ the trigger the S16 inference debounces on).
pub fn claude_pre_tool(session: &str, cwd: &str, tool: &str) -> String {
    format!(
        r#"{{"hook_event_name":"PreToolUse","session_id":"{session}","cwd":"{cwd}","tool_name":"{tool}"}}"#
    )
}

/// Claude `Stop` — the turn finished (idle). The completion signal Fleet derives
/// `done` from (never from `PostToolUse`, #31285).
pub fn claude_stop(session: &str, cwd: &str) -> String {
    format!(
        r#"{{"hook_event_name":"Stop","session_id":"{session}","cwd":"{cwd}","stop_hook_active":false}}"#
    )
}

/// Claude `SessionEnd` → dead.
pub fn claude_session_end(session: &str, cwd: &str) -> String {
    format!(r#"{{"hook_event_name":"SessionEnd","session_id":"{session}","cwd":"{cwd}"}}"#)
}

/// Claude `PermissionRequest` — **only fires under the integrated-terminal shim**
/// (Use-Terminal mode). The shim adapter renders this `high`; the *same* payload
/// in the native-UI surface can only ever be `inferred` (§21.3).
pub fn claude_permission_request(session: &str, cwd: &str, tool: &str) -> String {
    format!(
        r#"{{"hook_event_name":"PermissionRequest","session_id":"{session}","cwd":"{cwd}","tool_name":"{tool}"}}"#
    )
}

/// Claude `PermissionRequest` **response** under the shim — answered in terminal.
pub fn claude_permission_response(session: &str, cwd: &str, tool: &str, allow: bool) -> String {
    let perm = if allow { "allow" } else { "deny" };
    format!(
        r#"{{"hook_event_name":"PermissionRequest","session_id":"{session}","cwd":"{cwd}","tool_name":"{tool}","permission":"{perm}"}}"#
    )
}

/// A Claude transcript-JSONL body whose **last `tool_use` has no `tool_result`** —
/// corroborates a stuck `PreToolUse` (the S16 inference path → `inferred` waiting).
pub fn claude_transcript_stuck(tool_use_id: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"running a tool"}},{{"type":"tool_use","id":"{tool_use_id}","name":"Bash","input":{{"command":"ls"}}}}]}}}}"#
    )
}

/// A Claude transcript-JSONL body whose last `tool_use` **has** a matching
/// `tool_result` — vetoes the inference (the run is not actually blocked).
pub fn claude_transcript_resolved(tool_use_id: &str) -> String {
    let dispatch = format!(
        r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{tool_use_id}","name":"Bash","input":{{}}}}]}}}}"#
    );
    let result = format!(
        r#"{{"type":"user","message":{{"content":[{{"type":"tool_result","tool_use_id":"{tool_use_id}","content":"ok"}}]}}}}"#
    );
    format!("{dispatch}\n{result}")
}
