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

// ── Codex hook fixtures (the engineering spec; default hooks-first path) ───────────────

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

// ── Claude hook fixtures (the engineering spec) ───────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Parse a fixture line as JSON, asserting it is well-formed and returning it.
    fn parse(line: &str) -> Value {
        serde_json::from_str(line).unwrap_or_else(|e| panic!("invalid fixture JSON: {e}\n{line}"))
    }

    #[test]
    fn codex_lifecycle_fixtures_have_expected_shape() {
        let v = parse(&codex_session_start("th-1", "/work"));
        assert_eq!(v["hook_event_name"], "SessionStart");
        assert_eq!(v["session_id"], "th-1");
        assert_eq!(v["cwd"], "/work");

        let v = parse(&codex_prompt("th-1", "/work"));
        assert_eq!(v["hook_event_name"], "UserPromptSubmit");
        assert_eq!(v["turn_id"], "t1");

        let v = parse(&codex_pre_tool("th-1", "/work", "shell"));
        assert_eq!(v["hook_event_name"], "PreToolUse");
        assert_eq!(v["tool_name"], "shell");

        let v = parse(&codex_stop("th-1", "/work"));
        assert_eq!(v["hook_event_name"], "Stop");

        // codex_session_end — previously uncovered.
        let v = parse(&codex_session_end("th-1", "/work"));
        assert_eq!(v["hook_event_name"], "SessionEnd");
        assert_eq!(v["session_id"], "th-1");
        assert_eq!(v["cwd"], "/work");
    }

    #[test]
    fn codex_permission_fixtures_carry_decision() {
        let v = parse(&codex_permission_request("th", "/w", "shell"));
        assert_eq!(v["hook_event_name"], "PermissionRequest");
        assert!(v.get("decision").is_none(), "the request carries no decision");

        let allow = parse(&codex_permission_response("th", "/w", "shell", true));
        assert_eq!(allow["decision"], "allow");
        let deny = parse(&codex_permission_response("th", "/w", "shell", false));
        assert_eq!(deny["decision"], "deny");
    }

    #[test]
    fn claude_lifecycle_fixtures_have_expected_shape() {
        let v = parse(&claude_session_start("s-1", "/ui"));
        assert_eq!(v["hook_event_name"], "SessionStart");
        assert_eq!(v["session_id"], "s-1");

        let v = parse(&claude_prompt("s-1", "/ui"));
        assert_eq!(v["hook_event_name"], "UserPromptSubmit");

        let v = parse(&claude_pre_tool("s-1", "/ui", "Edit"));
        assert_eq!(v["hook_event_name"], "PreToolUse");
        assert_eq!(v["tool_name"], "Edit");

        // claude_stop — previously uncovered; carries the completion marker.
        let v = parse(&claude_stop("s-1", "/ui"));
        assert_eq!(v["hook_event_name"], "Stop");
        assert_eq!(v["stop_hook_active"], false);

        // claude_session_end — previously uncovered.
        let v = parse(&claude_session_end("s-1", "/ui"));
        assert_eq!(v["hook_event_name"], "SessionEnd");
        assert_eq!(v["session_id"], "s-1");
        assert_eq!(v["cwd"], "/ui");
    }

    #[test]
    fn claude_permission_fixtures_carry_permission() {
        let req = parse(&claude_permission_request("s", "/ui", "Edit"));
        assert_eq!(req["hook_event_name"], "PermissionRequest");
        assert!(req.get("permission").is_none(), "the request has no verdict yet");

        // claude_permission_response — previously uncovered (both verdicts).
        let allow = parse(&claude_permission_response("s", "/ui", "Edit", true));
        assert_eq!(allow["permission"], "allow");
        assert_eq!(allow["tool_name"], "Edit");
        let deny = parse(&claude_permission_response("s", "/ui", "Edit", false));
        assert_eq!(deny["permission"], "deny");
    }

    #[test]
    fn claude_transcript_fixtures_round_trip_as_jsonl() {
        // The stuck transcript is a single assistant line whose tool_use has an id.
        let stuck = claude_transcript_stuck("tu-9");
        let v = parse(&stuck);
        assert_eq!(v["type"], "assistant");
        let content = v["message"]["content"].as_array().unwrap();
        assert!(content.iter().any(|c| c["type"] == "tool_use" && c["id"] == "tu-9"));

        // The resolved transcript is TWO JSONL lines: a tool_use then a tool_result.
        let resolved = claude_transcript_resolved("tu-9");
        let mut lines = resolved.lines();
        let dispatch = parse(lines.next().unwrap());
        assert_eq!(dispatch["type"], "assistant");
        let result = parse(lines.next().unwrap());
        assert_eq!(result["type"], "user");
        assert_eq!(result["message"]["content"][0]["tool_use_id"], "tu-9");
        assert!(lines.next().is_none(), "exactly two JSONL lines");
    }
}
