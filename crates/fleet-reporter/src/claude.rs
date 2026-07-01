//! Claude Code detection adapter.
//!
//! This is the **hooks-first** Claude Code integration for `working`/`idle`/`done`
//! detection. It is the sibling of [`crate::codex`] in the same crate and reuses
//! the same gated reporter framework verbatim (REPCORE/IDENTITY commands), but the
//! Claude hook surface and its reliability boundary differ in ways the spec calls
//! out explicitly (engineering spec §1, S15):
//!
//! ### The hooks Fleet consumes (reliable in **all** surfaces, incl. native UI)
//!
//! | Hook event | Meaning | Fleet state |
//! |---|---|---|
//! | `SessionStart` | a Claude session opened (start / `--resume` / `--continue`) | `idle` |
//! | `UserPromptSubmit` | the user submitted a prompt | `working` |
//! | `PreToolUse` | the agent is about to run a tool | `working` (+ liveness) |
//! | `Stop` | the turn finished — **the** completion signal | `idle` or `done` |
//! | `SessionEnd` | the session closed | `dead` |
//!
//! ### Hooks Fleet deliberately does **NOT** depend on here
//!
//! - **`PostToolUse`** — does **not** fire in the native extension UI
//!   (anthropics/claude-code **#31285**). Relying on it for completion would make
//!   `done` silently wrong in the native-UI surface, so this adapter **derives
//!   `done` from `Stop`**, never from `PostToolUse`. A stray `PostToolUse` is
//!   accepted as a pure liveness ping (it is harmless corroboration when it *does*
//!   fire), but it never flips state.
//! - **`PermissionRequest` / `Notification`** — these do **not** fire in the
//!   native extension UI either (engineering spec §1; reproduced through ext v2.1.143). So this
//!   S15 adapter does **not** model `waiting`/approval at all — that is the job of
//!   [`crate::claude_infer`] (`CLINFER` / S16, inferred from
//!   `PreToolUse`-without-`Stop` + JSONL drift-guard) and
//!   `CLUSETERM` (S17, authoritative `PermissionRequest` only under the shim). By
//!   construction, **S15 never emits [`State::Waiting`] and never emits
//!   [`Confidence::High`] for a heuristic** — confidence honesty (invariant 5) is
//!   structural here, not just enforced by a test.
//!
//! ## Distinguishing `done` from `idle` (D9)
//!
//! The Claude Stop hook input carries **no** task-vs-turn-complete field — the
//! `task_complete` / `reason` / `subtype` markers earlier builds of this adapter
//! parsed were phantom (they are not in the 2026 hook contract:
//! code.claude.com/docs/en/hooks). The **real** signal is the *Stop event itself*:
//! its firing **is** the turn-complete signal. So a real `Stop`
//! (`stop_hook_active == false`) is the honest "the assistant finished this turn
//! and is awaiting the human" → [`State::Done`] (D9-distinct from idle). Hooks
//! cannot distinguish *task*-complete from *turn*-complete, so every real turn
//! boundary is `Done`; that is the honest ceiling of what the hook stream proves.
//!
//! [`State::Idle`] is reached the other honest way — a `SessionStart` (a session
//! opened / resumed but not yet seen to do work). `stop_hook_active == true`
//! denotes a `Stop` fired from *within* a Stop hook's own continuation loop, i.e.
//! **not** a real turn boundary → it is a pure liveness ping, no completion claim.
//!
//! ## Durable identity (D4 / §7.5)
//!
//! Every Claude hook payload carries `session_id`, validated stable across
//! `--continue`/`--resume` on the current CLI. We use it verbatim as the
//! run's [`AgentRun::native_id`] durable anchor. No broker, no derived id.
//!
//! The module is pure and sync (no I/O, no async, no Hub dependency at the
//! state-machine layer) so every transition is exhaustively unit-testable against
//! **recorded Claude hook-event JSON fixtures**.

use serde::Deserialize;

use fleet_protocol::{AgentKind, AgentRun, Confidence, Extra, State, SCHEMA_VERSION};

use crate::reporter::ReporterCommand;

/// The set of Claude hook events this S15 adapter understands. Unknown event
/// names are preserved as [`ClaudeHookKind::Other`] so a Claude version that adds
/// a hook never makes the parser panic (schema-drift tolerance, invariant 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeHookKind {
    /// A session opened (fresh / `--resume` / `--continue`) → `idle`.
    SessionStart,
    /// The user submitted a prompt → `working`.
    UserPromptSubmit,
    /// The agent is about to run a tool → `working` + liveness.
    PreToolUse,
    /// A tool finished. **NOT used for completion** (does not fire in native UI,
    /// #31285): pure liveness only, never a state flip.
    PostToolUse,
    /// The turn finished — **the** completion signal → `idle` or `done`.
    Stop,
    /// A subagent's turn finished — liveness only (does not end the main run).
    SubagentStop,
    /// The session closed → `dead`.
    SessionEnd,
    /// A context-compaction boundary (telemetry only).
    PreCompact,
    /// Any hook name this build does not model (forward-compatible).
    Other(String),
}

impl ClaudeHookKind {
    fn from_name(name: &str) -> Self {
        match name {
            "SessionStart" => ClaudeHookKind::SessionStart,
            "UserPromptSubmit" => ClaudeHookKind::UserPromptSubmit,
            "PreToolUse" => ClaudeHookKind::PreToolUse,
            "PostToolUse" => ClaudeHookKind::PostToolUse,
            "Stop" => ClaudeHookKind::Stop,
            "SubagentStop" => ClaudeHookKind::SubagentStop,
            "SessionEnd" => ClaudeHookKind::SessionEnd,
            "PreCompact" => ClaudeHookKind::PreCompact,
            other => ClaudeHookKind::Other(other.to_string()),
        }
    }

    /// The canonical Claude hook-event name (the `hook_event_name` wire token).
    pub fn name(&self) -> &str {
        match self {
            ClaudeHookKind::SessionStart => "SessionStart",
            ClaudeHookKind::UserPromptSubmit => "UserPromptSubmit",
            ClaudeHookKind::PreToolUse => "PreToolUse",
            ClaudeHookKind::PostToolUse => "PostToolUse",
            ClaudeHookKind::Stop => "Stop",
            ClaudeHookKind::SubagentStop => "SubagentStop",
            ClaudeHookKind::SessionEnd => "SessionEnd",
            ClaudeHookKind::PreCompact => "PreCompact",
            ClaudeHookKind::Other(s) => s,
        }
    }
}

/// A parsed Claude hook event. Constructed from the recorded hook-event JSON via
/// [`ClaudeHookEvent::parse`]. Only the fields Fleet needs are surfaced; everything
/// else in the payload is ignored (never errors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeHookEvent {
    /// Which hook fired.
    pub kind: ClaudeHookKind,
    /// Claude `session_id` — the durable identity anchor (stable across
    /// `--continue`/`--resume`).
    pub session_id: String,
    /// The session's working directory, if present (`cwd`).
    pub cwd: Option<String>,
    /// The tool name, when the hook concerns a tool (`PreToolUse`,`PostToolUse`).
    pub tool_name: Option<String>,
    /// `true` when a `Stop`/`SubagentStop` fired from *within* a stop hook's own
    /// continuation (`stop_hook_active`), i.e. **not** a real end-of-task turn.
    pub stop_hook_active: bool,
    /// The assistant's last message text, when the payload carries it (real
    /// `Stop` payloads include `last_assistant_message`). Surfaced as the inbox
    /// preview so a finished/idle run shows *what Claude said*, not a generic line.
    pub last_message: Option<String>,
    /// The tool-call id for a `PreToolUse` (`tool_use_id`, a `toolu_…` value).
    /// The durable correlation anchor between this hook and the transcript's
    /// `tool_use`/`tool_result` blocks (used by [`crate::claude_infer`]).
    pub tool_use_id: Option<String>,
    /// The append-only transcript JSONL path Claude writes (`transcript_path`).
    /// The S16 inference path reads it to corroborate an inferred `waiting`
    /// (an armed `tool_use` with no `tool_result`) before firing.
    pub transcript_path: Option<String>,
}

/// The raw on-the-wire shape of a Claude hook payload, as recorded from the hook
/// stdin. The canonical Claude fields are `hook_event_name`, `session_id`, `cwd`,
/// `tool_name`, `stop_hook_active`, `transcript_path`. We additionally tolerate
/// camelCase aliases (`hookEventName`, `sessionId`) defensively.
#[derive(Debug, Deserialize)]
struct RawClaudeHook {
    #[serde(alias = "hookEventName")]
    hook_event_name: Option<String>,
    #[serde(alias = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    #[serde(alias = "toolName")]
    tool_name: Option<String>,
    /// Claude sets this `true` when a `Stop` fires from a Stop hook's own
    /// continuation, not a real task end. Absent/`false` ⇒ a real turn boundary.
    #[serde(alias = "stopHookActive")]
    stop_hook_active: Option<bool>,
    /// Real `Stop` payloads carry the assistant's final text here.
    #[serde(alias = "lastAssistantMessage")]
    last_assistant_message: Option<String>,
    /// Real `PreToolUse` payloads carry the tool-call id here.
    #[serde(alias = "toolUseId")]
    tool_use_id: Option<String>,
    /// The append-only transcript JSONL path (`transcript_path`) — the S16
    /// corroboration source.
    #[serde(alias = "transcriptPath")]
    transcript_path: Option<String>,
}

/// Error parsing a Claude hook payload. The only hard requirements are valid JSON
/// and the two identity fields; everything else degrades to `None`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ClaudeParseError {
    /// The payload was not valid JSON.
    #[error("claude hook payload was not valid JSON: {0}")]
    InvalidJson(String),
    /// No `hook_event_name` / `hookEventName` field.
    #[error("claude hook payload missing hook_event_name")]
    MissingEventName,
    /// No `session_id` / `sessionId` field (no durable anchor — identity honesty).
    #[error("claude hook payload missing session_id (no durable identity anchor)")]
    MissingSessionId,
}

impl ClaudeHookEvent {
    /// Parse a recorded Claude hook-event JSON string into a [`ClaudeHookEvent`].
    ///
    /// Returns [`ClaudeParseError`] for malformed JSON, a missing event name, or a
    /// missing session anchor. Unknown event names parse to
    /// [`ClaudeHookKind::Other`] rather than erroring (schema-drift tolerance).
    pub fn parse(json: &str) -> Result<Self, ClaudeParseError> {
        let raw: RawClaudeHook =
            serde_json::from_str(json).map_err(|e| ClaudeParseError::InvalidJson(e.to_string()))?;
        Self::from_raw(raw)
    }

    /// Parse from an already-deserialized JSON value (e.g. when the socket layer
    /// hands us a `serde_json::Value`).
    pub fn from_value(v: serde_json::Value) -> Result<Self, ClaudeParseError> {
        let raw: RawClaudeHook =
            serde_json::from_value(v).map_err(|e| ClaudeParseError::InvalidJson(e.to_string()))?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawClaudeHook) -> Result<Self, ClaudeParseError> {
        let name = raw
            .hook_event_name
            .ok_or(ClaudeParseError::MissingEventName)?;
        let session_id = raw
            .session_id
            .filter(|s| !s.is_empty())
            .ok_or(ClaudeParseError::MissingSessionId)?;
        let kind = ClaudeHookKind::from_name(&name);
        let stop_hook_active = raw.stop_hook_active.unwrap_or(false);
        Ok(ClaudeHookEvent {
            kind,
            session_id,
            cwd: raw.cwd,
            tool_name: raw.tool_name,
            stop_hook_active,
            last_message: raw.last_assistant_message.filter(|m| !m.is_empty()),
            tool_use_id: raw.tool_use_id.filter(|s| !s.is_empty()),
            transcript_path: raw.transcript_path.filter(|s| !s.is_empty()),
        })
    }
}

/// The pure Claude detection **state machine** (S15): one per Claude `session_id`.
/// It consumes parsed [`ClaudeHookEvent`]s and produces the run's authoritative
/// [`State`] / [`Confidence`], guaranteeing the state-model invariants G2 requires:
///
/// - **No illegal transition** — every edge is one of the modelled ones; an event
///   that doesn't apply is an idempotent no-op, never a panic.
/// - **Confidence honesty (structural)** — this S15 machine never enters
///   [`State::Waiting`] and never emits [`Confidence::High`] except on a confirmed
///   `SessionEnd` exit. There is no heuristic `High` path here at all.
/// - **`done` is derived from `Stop`, never from `PostToolUse`** (#31285).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeStateMachine {
    session_id: String,
    cwd: String,
    state: State,
    confidence: Confidence,
    last_tool: Option<String>,
    /// The assistant's last message (from a `Stop`'s `last_assistant_message`),
    /// surfaced as the idle/done inbox preview.
    last_assistant_message: Option<String>,
}

/// A single state transition the machine decided, returned by
/// [`ClaudeStateMachine::apply`]. `changed` is `false` for a no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// The run's state after the event.
    pub state: State,
    /// The run's confidence after the event.
    pub confidence: Confidence,
    /// Whether the run-relevant fields actually changed (vs. a no-op).
    pub changed: bool,
    /// Whether this event is a pure liveness signal (refresh the timeout window).
    pub liveness: bool,
}

impl ClaudeStateMachine {
    /// A new machine for a session, starting in `idle` (a session just observed is
    /// alive but has not yet been seen to do work).
    pub fn new(session_id: impl Into<String>) -> Self {
        ClaudeStateMachine {
            session_id: session_id.into(),
            cwd: "/".to_string(),
            state: State::Idle,
            confidence: Confidence::Inferred,
            last_tool: None,
            last_assistant_message: None,
        }
    }

    /// The session's durable id (Claude `session_id`).
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    /// The run's current state.
    pub fn state(&self) -> State {
        self.state
    }
    /// The run's current confidence.
    pub fn confidence(&self) -> Confidence {
        self.confidence
    }
    /// The session's last-known working directory.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Apply a parsed hook event, mutating the machine and returning the
    /// [`Transition`]. Pure and total: every event is handled; an inapplicable
    /// event is an idempotent no-op (`changed == false`).
    pub fn apply(&mut self, ev: &ClaudeHookEvent) -> Transition {
        // Session-id mismatch: the caller routes per session, but guard anyway —
        // a foreign event must never mutate this machine.
        if ev.session_id != self.session_id {
            return self.no_op(false);
        }
        if let Some(c) = &ev.cwd {
            if !c.is_empty() {
                self.cwd = c.clone();
            }
        }
        if let Some(t) = &ev.tool_name {
            self.last_tool = Some(t.clone());
        }
        if let Some(m) = &ev.last_message {
            self.last_assistant_message = Some(m.clone());
        }

        match &ev.kind {
            // ── activity hooks → working ─────────────────────────────────────
            ClaudeHookKind::UserPromptSubmit | ClaudeHookKind::PreToolUse => {
                let was_working = self.state == State::Working;
                self.enter_working();
                Transition {
                    state: self.state,
                    confidence: self.confidence,
                    changed: !was_working,
                    liveness: true,
                }
            }

            // ── turn complete → done (NEVER from PostToolUse) ─────────────────
            ClaudeHookKind::Stop => {
                if ev.stop_hook_active {
                    // A Stop fired from inside a Stop hook's own continuation loop
                    // is NOT a real turn boundary: pure liveness, no completion.
                    return self.no_op(true);
                }
                // The Stop event firing IS the turn-complete signal (hooks carry no
                // task-vs-turn field), so a real Stop is the honest "assistant
                // finished this turn" → Done (D9-distinct from idle).
                let changed = self.state != State::Done;
                self.state = State::Done;
                self.confidence = Confidence::Inferred;
                Transition {
                    state: self.state,
                    confidence: self.confidence,
                    changed,
                    liveness: false,
                }
            }

            // ── session closed → dead ────────────────────────────────────────
            ClaudeHookKind::SessionEnd => {
                let changed = self.state != State::Dead;
                self.state = State::Dead;
                self.confidence = Confidence::High; // confirmed exit is authoritative
                Transition {
                    state: self.state,
                    confidence: self.confidence,
                    changed,
                    liveness: false,
                }
            }

            // ── session opened → idle ────────────────────────────────────────
            ClaudeHookKind::SessionStart => {
                // Re-opening a closed session (resume/continue) revives it to idle;
                // an already-live session stays where it is (no spurious reset).
                if self.state == State::Dead {
                    self.state = State::Idle;
                    self.confidence = Confidence::Inferred;
                    self.no_op(false).into_changed()
                } else {
                    self.no_op(false)
                }
            }

            // ── telemetry-only hooks: liveness, no state flip ────────────────
            //
            // PostToolUse is deliberately liveness-only — it does NOT fire in the
            // native UI (#31285), so `done` is derived from `Stop` instead.
            // SubagentStop ends a *subagent*, not the main run, so it must not flip
            // the run to idle/done.
            ClaudeHookKind::PostToolUse
            | ClaudeHookKind::SubagentStop
            | ClaudeHookKind::PreCompact => self.no_op(true),
            ClaudeHookKind::Other(_) => self.no_op(false),
        }
    }

    /// Build the current [`AgentRun`] snapshot for this session, stamped with the
    /// given timestamp. The run's `native_id` is the Claude `session_id` (durable
    /// anchor), so the reporter framework keys identity on it.
    pub fn to_run(&self, run_id: impl Into<String>, updated_at: impl Into<String>) -> AgentRun {
        let updated_at = updated_at.into();
        AgentRun {
            schema_version: SCHEMA_VERSION,
            run_id: run_id.into(),
            agent_kind: AgentKind::ClaudeCode,
            native_id: self.session_id.clone(),
            cwd: self.cwd.clone(),
            state: self.state,
            // S15 never produces a waiting/urgent state (see module docs).
            urgency: None,
            last_message: self.last_message(),
            // S15 never produces a waiting state, so never a waiting_since.
            waiting_since: None,
            confidence: self.confidence,
            diff_summary: None,
            updated_at,
            extra: Extra::new(),
        }
    }

    // ── internal transitions ─────────────────────────────────────────────────

    fn enter_working(&mut self) {
        self.state = State::Working;
        // Honesty: working is inferred from activity, not an authoritative signal.
        self.confidence = Confidence::Inferred;
    }

    fn no_op(&self, liveness: bool) -> Transition {
        Transition {
            state: self.state,
            confidence: self.confidence,
            changed: false,
            liveness,
        }
    }

    fn last_message(&self) -> Option<String> {
        match self.state {
            State::Working => self.last_tool.as_ref().map(|t| format!("Running {t}…")),
            // After a turn ends, show what Claude actually said (real `Stop`
            // payloads carry `last_assistant_message`) — a far better inbox
            // preview than a generic line. Falls back to the generic when absent.
            State::Idle => self.last_assistant_message.as_deref().map(preview),
            State::Done => Some(
                self.last_assistant_message
                    .as_deref()
                    .map(preview)
                    .unwrap_or_else(|| "Task complete.".to_string()),
            ),
            State::Dead => Some("Session closed.".to_string()),
            _ => None,
        }
    }
}

/// Truncate an assistant message to a single-line inbox preview (≤ 100 chars).
/// Shared with the Codex adapter (same inbox-preview semantics).
pub(crate) fn preview(msg: &str) -> String {
    const MAX: usize = 100;
    let one_line = msg.replace(['\n', '\r'], " ");
    let trimmed = one_line.trim();
    if trimmed.chars().count() <= MAX {
        trimmed.to_string()
    } else {
        let cut: String = trimmed.chars().take(MAX).collect();
        format!("{cut}…")
    }
}

impl Transition {
    /// Mark a no-op transition as having changed (used for the resume edge).
    fn into_changed(mut self) -> Self {
        self.changed = true;
        self
    }
}

/// The Claude **adapter** (S15): a thin map from a stream of Claude hook events
/// (multiplexed across sessions, since one reporter shell can host several
/// `claude` invocations) to [`ReporterCommand`]s the reporter framework already
/// knows how to deliver, buffer, and reclaim.
///
/// It owns one [`ClaudeStateMachine`] per Claude `session_id`, mints a stable Fleet
/// `run_id` per session, and translates each transition into the right command —
/// exactly mirroring [`crate::codex::CodexAdapter`] so the two adapters share the
/// gated REPCORE/IDENTITY plumbing verbatim:
///
/// - any state change → [`ReporterCommand::UpsertRun`];
/// - a pure-liveness telemetry hook with no state change →
///   [`ReporterCommand::Liveness`];
/// - a `SessionEnd` → an `UpsertRun(dead)` (confirmed-exit confidence).
#[derive(Debug, Default)]
pub struct ClaudeAdapter {
    sessions: std::collections::HashMap<String, ClaudeSession>,
    run_counter: u64,
}

#[derive(Debug)]
struct ClaudeSession {
    machine: ClaudeStateMachine,
    run_id: String,
}

impl ClaudeAdapter {
    /// A fresh adapter tracking no sessions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct Claude sessions currently tracked.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// The current run state for a session, if tracked (for assertions/UX).
    pub fn state_of(&self, session_id: &str) -> Option<State> {
        self.sessions.get(session_id).map(|s| s.machine.state())
    }

    /// The Fleet run-id minted for a session, if tracked.
    pub fn run_id_of(&self, session_id: &str) -> Option<&str> {
        self.sessions.get(session_id).map(|s| s.run_id.as_str())
    }

    /// Borrow a session's state machine (tests).
    pub fn machine_of(&self, session_id: &str) -> Option<&ClaudeStateMachine> {
        self.sessions.get(session_id).map(|s| &s.machine)
    }

    /// Ingest one **raw** recorded hook-event JSON line, producing the
    /// [`ReporterCommand`]s to forward (empty on a no-op or a parse error). Parse
    /// errors are swallowed at this layer — a malformed line must never crash the
    /// reporter or overstate state.
    pub fn ingest_json(&mut self, json: &str) -> Vec<ReporterCommand> {
        match ClaudeHookEvent::parse(json) {
            Ok(ev) => self.ingest(&ev).0,
            Err(_) => Vec::new(),
        }
    }

    /// Ingest a parsed hook event, returning `(commands, transition)`.
    pub fn ingest(&mut self, ev: &ClaudeHookEvent) -> (Vec<ReporterCommand>, Transition) {
        // First sighting of a session mints its Fleet run-id and machine.
        if !self.sessions.contains_key(&ev.session_id) {
            self.run_counter += 1;
            let run_id = format!("claude:{}:run-{}", ev.session_id, self.run_counter);
            self.sessions.insert(
                ev.session_id.clone(),
                ClaudeSession {
                    machine: ClaudeStateMachine::new(ev.session_id.clone()),
                    run_id,
                },
            );
        }
        let session = self
            .sessions
            .get_mut(&ev.session_id)
            .expect("just inserted");
        let transition = session.machine.apply(ev);
        let run_id = session.run_id.clone();

        let mut cmds = Vec::new();
        if transition.changed {
            let run = session
                .machine
                .to_run(run_id.clone(), crate::fake::now_iso8601());
            cmds.push(ReporterCommand::UpsertRun(run));
        } else if transition.liveness {
            cmds.push(ReporterCommand::Liveness { run_id });
        }
        (cmds, transition)
    }

    /// Forget a session entirely (e.g. its `SessionEnd` was already delivered and
    /// the run reaped). A later event for the same session starts a fresh run.
    pub fn forget(&mut self, session_id: &str) -> bool {
        self.sessions.remove(session_id).is_some()
    }
}

#[cfg(test)]
mod tests {
    include!("claude_tests.rs");
}
