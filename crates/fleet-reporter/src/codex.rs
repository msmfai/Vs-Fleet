//! Codex detection adapter.
//!
//! > ⚠️ **LIVE-UNTESTED (2026-06).** The Codex hook *config* (`fleet init`,
//! > composable `[[hooks.*]]`) and this adapter are aligned to OpenAI's official
//! > Codex-hooks docs and unit-tested against fixtures, but have **not** been run
//! > against a real `codex` binary yet (no Codex access on the dev machine). The
//! > Claude path *is* live-validated end-to-end. Before relying on Codex: run
//! > `fleet init`, trust the Fleet hook via Codex's `/hooks` (never
//! > `--dangerously-bypass-hook-trust`), run `codex exec`, and confirm the real
//! > payload matches the fixtures here.
//!
//! This module is the **default, hooks-first** Codex integration. Fleet does not
//! tap a hand-launched Codex TUI's `app-server` because passive observation of a
//! hand-launched TUI is infeasible on stock Codex: each client starts its own
//! app-server instance, one active client per thread. Instead Fleet consumes the
//! **Codex hooks** that cmux ships on `main` (a CLI-driven `~/.codex/hooks.json`
//! install; canonical key `[features] hooks`, default-on):
//!
//! | Hook event | Meaning | Fleet state |
//! |---|---|---|
//! | `SessionStart` | a thread opened | `idle` (alive, awaiting prompt) |
//! | `UserPromptSubmit` | the user submitted a prompt | `working` |
//! | `PreToolUse` | the agent is about to run a tool | `working` (+ liveness) |
//! | `PostToolUse` | a tool finished (telemetry) | liveness only, no state flip |
//! | `PermissionRequest` | the agent is blocked on an approval | `waiting`+`approval`, **`high`** |
//! | `Stop` | the turn finished (the event IS the signal) | `done` |
//!
//! ## No `SessionEnd`, no inbound approval response (the 2026 Codex contract)
//!
//! The 2026 Codex hook event set has **no `SessionEnd`** and **no inbound
//! "PermissionRequest response"** — a Codex run's death is observable only via
//! process-exit / liveness timeout (driven by [`crate::reporter::ReporterCore`],
//! not a hook), and a `PermissionRequest` `decision` is a hook *output*, not a
//! second inbound event (developers.openai.com/codex/hooks). So this adapter never
//! emits `dead` itself and never parses an inbound decision; the auto-resolve of
//! an approval happens purely through the *subsequent activity* hooks.
//!
//! ## Durable identity (D4 / §7.5)
//!
//! Every Codex hook payload carries `session_id`, which is the Codex **`thread.id`**
//! anchor — stable across `codex resume <id>`. We use it verbatim as the run's
//! [`AgentRun::native_id`], which is the reporter framework's durable-identity
//! anchor ([`crate::identity`]). No broker, no derived id.
//!
//! ## Confidence honesty (invariant 5)
//!
//! Only the **`PermissionRequest`** hook is an authoritative `waiting` signal, so
//! it is the *only* path that yields [`Confidence::High`]. Every other state
//! (`working`/`idle`/`done`) carries [`Confidence::Inferred`] — they are derived
//! from activity hooks, not from an authoritative "I am blocked" signal. The state
//! machine **never** emits `High` for a non-`PermissionRequest`-derived state, and
//! a property test enforces this.
//!
//! ## Auto-resolve (S13)
//!
//! When the user answers the approval **in the real terminal**, Codex resumes and
//! fires the next activity hook (`PreToolUse`/`UserPromptSubmit`/`Stop`). That
//! subsequent activity drives the run out of `waiting` back to `working`/`done`
//! with **no Fleet interaction** — exactly the "auto-resolve" the spec requires.
//! There is no inbound decision event to model; the activity hook is the signal.
//!
//! The whole module is pure and sync (no I/O, no async, no Hub dependency at the
//! state-machine layer) so every transition, the approval, and the auto-resolve
//! are exhaustively unit-testable against **recorded hook-event JSON fixtures**.

use serde::Deserialize;

use fleet_protocol::{AgentKind, AgentRun, Confidence, Extra, State, Urgency, SCHEMA_VERSION};

/// The set of Codex hook events Fleet understands. Unknown event names are
/// preserved as [`CodexHookKind::Other`] so a Codex version that adds a hook
/// never makes the parser panic (schema-drift tolerance, mirrors invariant 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexHookKind {
    /// A thread opened — `idle` (alive, awaiting the first prompt).
    SessionStart,
    /// The user submitted a prompt — `working`.
    UserPromptSubmit,
    /// The agent is about to run a tool — `working` + liveness.
    PreToolUse,
    /// A tool finished — telemetry/liveness only, no state flip.
    PostToolUse,
    /// The agent is blocked on an approval — `waiting`+`approval`, authoritative.
    PermissionRequest,
    /// The turn finished — the event firing IS the turn-complete signal → `done`.
    Stop,
    /// A context-compaction boundary (telemetry only).
    PreCompact,
    /// A context-compaction boundary (telemetry only).
    PostCompact,
    /// Any hook name this build does not model (forward-compatible).
    Other(String),
}

impl CodexHookKind {
    fn from_name(name: &str) -> Self {
        match name {
            "SessionStart" => CodexHookKind::SessionStart,
            "UserPromptSubmit" => CodexHookKind::UserPromptSubmit,
            "PreToolUse" => CodexHookKind::PreToolUse,
            "PostToolUse" => CodexHookKind::PostToolUse,
            "PermissionRequest" => CodexHookKind::PermissionRequest,
            "Stop" => CodexHookKind::Stop,
            "PreCompact" => CodexHookKind::PreCompact,
            "PostCompact" => CodexHookKind::PostCompact,
            other => CodexHookKind::Other(other.to_string()),
        }
    }

    /// The canonical Codex hook-event name (the `hook_event_name` wire token).
    pub fn name(&self) -> &str {
        match self {
            CodexHookKind::SessionStart => "SessionStart",
            CodexHookKind::UserPromptSubmit => "UserPromptSubmit",
            CodexHookKind::PreToolUse => "PreToolUse",
            CodexHookKind::PostToolUse => "PostToolUse",
            CodexHookKind::PermissionRequest => "PermissionRequest",
            CodexHookKind::Stop => "Stop",
            CodexHookKind::PreCompact => "PreCompact",
            CodexHookKind::PostCompact => "PostCompact",
            CodexHookKind::Other(s) => s,
        }
    }
}

/// A parsed Codex hook event. Constructed from the recorded hook-event JSON via
/// [`CodexHookEvent::parse`]. Only the fields Fleet needs are surfaced; everything
/// else in the payload is ignored (never errors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexHookEvent {
    /// Which hook fired.
    pub kind: CodexHookKind,
    /// Codex `thread.id` (the `session_id` field) — the durable identity anchor.
    pub thread_id: String,
    /// The turn this event belongs to, if present (`turn_id`).
    pub turn_id: Option<String>,
    /// The working directory of the thread, if present.
    pub cwd: Option<String>,
    /// The tool name, when the hook concerns a tool (`PreToolUse`,
    /// `PermissionRequest`, `PostToolUse`).
    pub tool_name: Option<String>,
    /// `true` when a `Stop` fired from *within* a stop hook's own continuation
    /// (`stop_hook_active`), i.e. **not** a real turn boundary.
    pub stop_hook_active: bool,
    /// The assistant's last message (real `Stop` payloads carry
    /// `last_assistant_message`) — surfaced as the idle/done inbox preview.
    pub last_message: Option<String>,
    /// The `tool_use_id` for a `PreToolUse`/`PermissionRequest` — the durable
    /// correlation anchor to the transcript.
    pub tool_use_id: Option<String>,
}

/// The raw on-the-wire shape of a Codex hook payload, as recorded from the hook
/// stdin. Validated against the cmux Codex-hook regression fixtures: the canonical
/// fields are `hook_event_name`, `session_id`, `turn_id`, `cwd`, `tool_name`,
/// `tool_input`. We additionally tolerate camelCase aliases (`hookEventName`,
/// `sessionId`) that some Codex builds emit.
#[derive(Debug, Deserialize)]
struct RawCodexHook {
    #[serde(alias = "hookEventName")]
    hook_event_name: Option<String>,
    #[serde(alias = "sessionId", alias = "thread_id", alias = "threadId")]
    session_id: Option<String>,
    #[serde(alias = "turnId")]
    turn_id: Option<String>,
    cwd: Option<String>,
    #[serde(alias = "toolName")]
    tool_name: Option<String>,
    /// `true` when a `Stop` fired from within a stop hook's own continuation, i.e.
    /// not a real turn end (matches Claude's semantics).
    #[serde(alias = "stopHookActive")]
    stop_hook_active: Option<bool>,
    /// Real `Stop` payloads carry the assistant's final text here.
    #[serde(alias = "lastAssistantMessage")]
    last_assistant_message: Option<String>,
    /// Real `PreToolUse`/`PermissionRequest` payloads carry the tool-call id here.
    #[serde(alias = "toolUseId")]
    tool_use_id: Option<String>,
}

/// Error parsing a Codex hook payload. The only hard requirements are valid JSON
/// and the two identity fields; everything else degrades to `None`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CodexParseError {
    /// The payload was not valid JSON.
    #[error("codex hook payload was not valid JSON: {0}")]
    InvalidJson(String),
    /// No `hook_event_name` / `hookEventName` field.
    #[error("codex hook payload missing hook_event_name")]
    MissingEventName,
    /// No `session_id` / `sessionId` field (no durable anchor — identity honesty).
    #[error("codex hook payload missing session_id (no durable identity anchor)")]
    MissingThreadId,
}

impl CodexHookEvent {
    /// Parse a recorded Codex hook-event JSON string into a [`CodexHookEvent`].
    ///
    /// Returns [`CodexParseError`] for malformed JSON, a missing event name, or a
    /// missing thread anchor. Unknown event names parse to
    /// [`CodexHookKind::Other`] rather than erroring (schema-drift tolerance).
    pub fn parse(json: &str) -> Result<Self, CodexParseError> {
        let raw: RawCodexHook =
            serde_json::from_str(json).map_err(|e| CodexParseError::InvalidJson(e.to_string()))?;
        Self::from_raw(raw)
    }

    /// Parse from an already-deserialized JSON value (e.g. when the socket layer
    /// hands us a `serde_json::Value`).
    pub fn from_value(v: serde_json::Value) -> Result<Self, CodexParseError> {
        let raw: RawCodexHook =
            serde_json::from_value(v).map_err(|e| CodexParseError::InvalidJson(e.to_string()))?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawCodexHook) -> Result<Self, CodexParseError> {
        let name = raw
            .hook_event_name
            .ok_or(CodexParseError::MissingEventName)?;
        let thread_id = raw
            .session_id
            .filter(|s| !s.is_empty())
            .ok_or(CodexParseError::MissingThreadId)?;
        let kind = CodexHookKind::from_name(&name);
        let stop_hook_active = raw.stop_hook_active.unwrap_or(false);
        Ok(CodexHookEvent {
            kind,
            thread_id,
            turn_id: raw.turn_id,
            cwd: raw.cwd,
            tool_name: raw.tool_name,
            stop_hook_active,
            last_message: raw.last_assistant_message.filter(|m| !m.is_empty()),
            tool_use_id: raw.tool_use_id.filter(|s| !s.is_empty()),
        })
    }
}

/// The pure Codex detection **state machine**: one per thread. It consumes parsed
/// [`CodexHookEvent`]s and produces the run's authoritative [`State`] / [`Urgency`]
/// / [`Confidence`], guaranteeing the state-model invariants G2 requires:
///
/// - **No illegal transition** — every edge is one of the modelled ones; an event
///   that doesn't apply in the current state is a no-op (idempotent), never a
///   panic and never an out-of-band jump.
/// - **Confidence honesty** — `High` is reachable **only** through
///   `PermissionRequest`; every other state is `Inferred`.
/// - **`waiting` is sticky until resolved** — only fresh activity (auto-resolve)
///   or a `Stop` leaves `waiting`; there is no inbound approval-response event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexStateMachine {
    thread_id: String,
    cwd: String,
    state: State,
    urgency: Option<Urgency>,
    confidence: Confidence,
    last_tool: Option<String>,
    /// Set when a `PermissionRequest` is outstanding, so a later activity hook can
    /// recognise it as the *auto-resolve* of that approval.
    pending_approval: bool,
    /// The assistant's last message (from a `Stop`'s `last_assistant_message`),
    /// surfaced as the idle/done inbox preview.
    last_assistant_message: Option<String>,
}

/// A single state transition the machine decided, returned by
/// [`CodexStateMachine::apply`]. `changed` is `false` for a no-op (the event was
/// understood but did not move the run — e.g. telemetry, or a duplicate).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// The run's state after the event.
    pub state: State,
    /// The run's urgency after the event (`None` ⇒ no urgency).
    pub urgency: Option<Urgency>,
    /// The run's confidence after the event.
    pub confidence: Confidence,
    /// Whether the run-relevant fields actually changed (vs. a no-op).
    pub changed: bool,
    /// Whether this transition *cleared* a pending approval (auto-resolve / answer).
    pub resolved_approval: bool,
    /// Whether this event is a pure liveness signal (refresh the timeout window).
    pub liveness: bool,
}

impl CodexStateMachine {
    /// A new machine for a thread, starting in `idle` (a thread that has just been
    /// observed is alive but has not yet been seen to do work). `cwd` is a
    /// best-effort default until a hook supplies one.
    pub fn new(thread_id: impl Into<String>) -> Self {
        CodexStateMachine {
            thread_id: thread_id.into(),
            cwd: "/".to_string(),
            state: State::Idle,
            urgency: None,
            // Idle is not an authoritative *waiting* signal; honesty ⇒ Inferred.
            confidence: Confidence::Inferred,
            last_tool: None,
            pending_approval: false,
            last_assistant_message: None,
        }
    }

    /// The thread's durable id (Codex `thread.id`).
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }
    /// The run's current state.
    pub fn state(&self) -> State {
        self.state
    }
    /// The run's current urgency.
    pub fn urgency(&self) -> Option<Urgency> {
        self.urgency
    }
    /// The run's current confidence.
    pub fn confidence(&self) -> Confidence {
        self.confidence
    }
    /// Whether an approval is outstanding.
    pub fn awaiting_approval(&self) -> bool {
        self.pending_approval
    }
    /// The thread's last-known working directory.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Apply a parsed hook event, mutating the machine and returning the
    /// [`Transition`]. Pure and total: every event is handled; an inapplicable
    /// event is an idempotent no-op (`changed == false`).
    pub fn apply(&mut self, ev: &CodexHookEvent) -> Transition {
        // Thread-id mismatch: the caller routes per thread, but guard anyway —
        // a foreign event must never mutate this machine.
        if ev.thread_id != self.thread_id {
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
            CodexHookKind::UserPromptSubmit | CodexHookKind::PreToolUse => {
                // Fresh activity auto-resolves any stale pending approval.
                let resolved = self.pending_approval;
                let was_working = self.state == State::Working && self.urgency.is_none();
                self.enter_working();
                Transition {
                    state: self.state,
                    urgency: self.urgency,
                    confidence: self.confidence,
                    // Changed iff we actually moved (a working→working repeat with
                    // no pending approval is a no-op, just a liveness ping).
                    changed: !was_working || resolved,
                    resolved_approval: resolved,
                    liveness: true,
                }
            }

            // ── permission gate ──────────────────────────────────────────────
            CodexHookKind::PermissionRequest => {
                // A `PermissionRequest` is always a fresh request (there is no
                // inbound response event) → waiting+approval, authoritative. It
                // auto-resolves later via the subsequent activity hooks.
                self.enter_waiting_approval();
                Transition {
                    state: self.state,
                    urgency: self.urgency,
                    confidence: self.confidence,
                    changed: true,
                    resolved_approval: false,
                    liveness: true,
                }
            }

            // ── turn complete → done ─────────────────────────────────────────
            CodexHookKind::Stop => {
                let resolved = self.pending_approval;
                if ev.stop_hook_active {
                    // A continuation Stop is not a real turn boundary — activity,
                    // not completion: cancel any pending approval, stay working.
                    let was_working = self.state == State::Working && self.urgency.is_none();
                    self.enter_working();
                    return Transition {
                        state: self.state,
                        urgency: self.urgency,
                        confidence: self.confidence,
                        changed: !was_working || resolved,
                        resolved_approval: resolved,
                        liveness: true,
                    };
                }
                // The Stop event firing IS the turn-complete signal → Done.
                let changed = self.state != State::Done || self.urgency.is_some();
                self.state = State::Done;
                self.urgency = None;
                self.confidence = Confidence::Inferred;
                self.pending_approval = false;
                Transition {
                    state: self.state,
                    urgency: self.urgency,
                    confidence: self.confidence,
                    changed: changed || resolved,
                    resolved_approval: resolved,
                    liveness: false,
                }
            }

            // ── thread opened → idle (liveness) ──────────────────────────────
            //
            // Codex has no `SessionEnd` hook, so this machine never enters `dead`
            // itself — a run's death is driven by the reporter's liveness timeout
            // / process-exit. A `SessionStart` on a live thread is an idempotent
            // no-op that still refreshes liveness.
            CodexHookKind::SessionStart => self.no_op(true),

            // ── telemetry-only hooks: liveness, no state flip ────────────────
            CodexHookKind::PostToolUse => self.no_op(true),
            CodexHookKind::PreCompact | CodexHookKind::PostCompact => self.no_op(true),
            CodexHookKind::Other(_) => self.no_op(false),
        }
    }

    /// Build the current [`AgentRun`] snapshot for this thread, stamped with the
    /// given timestamp. The run's `native_id` is the Codex `thread.id` (durable
    /// anchor), so the reporter framework keys identity on it.
    pub fn to_run(&self, run_id: impl Into<String>, updated_at: impl Into<String>) -> AgentRun {
        let updated_at = updated_at.into();
        AgentRun {
            schema_version: SCHEMA_VERSION,
            run_id: run_id.into(),
            agent_kind: AgentKind::Codex,
            native_id: self.thread_id.clone(),
            cwd: self.cwd.clone(),
            state: self.state,
            urgency: self.urgency,
            last_message: self.last_message(),
            waiting_since: if self.state == State::Waiting {
                Some(updated_at.clone())
            } else {
                None
            },
            confidence: self.confidence,
            diff_summary: None,
            updated_at,
            extra: Extra::new(),
        }
    }

    // ── internal transitions ─────────────────────────────────────────────────

    fn enter_working(&mut self) {
        self.state = State::Working;
        self.urgency = None;
        // Honesty: working is inferred from activity, not an authoritative signal.
        self.confidence = Confidence::Inferred;
        self.pending_approval = false;
    }

    fn enter_waiting_approval(&mut self) {
        self.state = State::Waiting;
        self.urgency = Some(Urgency::Approval);
        // The ONLY path to High confidence: an authoritative PermissionRequest.
        self.confidence = Confidence::High;
        self.pending_approval = true;
    }

    fn no_op(&self, liveness: bool) -> Transition {
        Transition {
            state: self.state,
            urgency: self.urgency,
            confidence: self.confidence,
            changed: false,
            resolved_approval: false,
            liveness,
        }
    }

    fn last_message(&self) -> Option<String> {
        match self.state {
            State::Waiting => Some(match &self.last_tool {
                Some(t) => format!("Approve {t}?"),
                None => "Approval required".to_string(),
            }),
            State::Working => self.last_tool.as_ref().map(|t| format!("Running {t}…")),
            // A real `Stop` ends the turn → Done, carrying what Codex said (real
            // `Stop` carries `last_assistant_message`); falls back when absent.
            State::Done => Some(
                self.last_assistant_message
                    .as_deref()
                    .map(crate::claude::preview)
                    .unwrap_or_else(|| "Task complete.".to_string()),
            ),
            // Idle = alive but nothing produced yet; Dead is driven at the reporter
            // level (no Codex `SessionEnd`), so it never previews here.
            _ => None,
        }
    }
}

/// The Codex **adapter**: a thin map from a stream of Codex hook events
/// (potentially multiplexed across threads, since one reporter shell can host
/// several `codex` invocations) to [`ReporterCommand`]s the reporter framework
/// already knows how to deliver, buffer, and reclaim.
///
/// It owns one [`CodexStateMachine`] per Codex `thread.id`, mints a stable Fleet
/// `run_id` per thread, and translates each transition into the right command:
///
/// - any state change → [`ReporterCommand::UpsertRun`] (the framework stamps it
///   `(durable_id=thread.id, epoch, seq)` and refreshes liveness);
/// - a pure-liveness telemetry hook with no state change →
///   [`ReporterCommand::Liveness`] (no Hub delta, just the timeout window).
///
/// Codex has no `SessionEnd` hook, so this adapter never emits a `dead` delta
/// itself — a Codex run's death is driven by the reporter's liveness timeout
/// ([`crate::reporter::ReporterCore::reap_timeouts`]) or a confirmed process exit
/// ([`ReporterCommand::ConfirmExit`]).
///
/// The adapter is sync and Hub-free; it returns commands for the caller to feed
/// into a [`crate::reporter::ReporterHandle`]. This keeps it exhaustively testable
/// without a runtime, while reusing the gated REPCORE/IDENTITY framework verbatim.
#[derive(Debug, Default)]
pub struct CodexAdapter {
    threads: std::collections::HashMap<String, CodexThread>,
    run_counter: u64,
}

#[derive(Debug)]
struct CodexThread {
    machine: CodexStateMachine,
    run_id: String,
}

use crate::reporter::ReporterCommand;

impl CodexAdapter {
    /// A fresh adapter tracking no threads.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct Codex threads currently tracked.
    pub fn thread_count(&self) -> usize {
        self.threads.len()
    }

    /// The current run state for a thread, if tracked (for assertions/UX).
    pub fn state_of(&self, thread_id: &str) -> Option<State> {
        self.threads.get(thread_id).map(|t| t.machine.state())
    }

    /// The Fleet run-id minted for a thread, if tracked.
    pub fn run_id_of(&self, thread_id: &str) -> Option<&str> {
        self.threads.get(thread_id).map(|t| t.run_id.as_str())
    }

    /// Borrow a thread's state machine (tests).
    pub fn machine_of(&self, thread_id: &str) -> Option<&CodexStateMachine> {
        self.threads.get(thread_id).map(|t| &t.machine)
    }

    /// Ingest one **raw** recorded hook-event JSON line, producing the
    /// [`ReporterCommand`]s to forward (empty on a no-op or a parse error). Parse
    /// errors are swallowed at this layer — a malformed line must never crash the
    /// reporter or overstate state — but are surfaced via [`Self::ingest`] when the
    /// caller wants to log them.
    pub fn ingest_json(&mut self, json: &str) -> Vec<ReporterCommand> {
        match CodexHookEvent::parse(json) {
            Ok(ev) => self.ingest(&ev).0,
            Err(_) => Vec::new(),
        }
    }

    /// Ingest a parsed hook event, returning `(commands, transition)`. The
    /// transition is exposed so callers/tests can assert on the state-machine
    /// decision independently of the wire commands.
    pub fn ingest(&mut self, ev: &CodexHookEvent) -> (Vec<ReporterCommand>, Transition) {
        // First sighting of a thread mints its Fleet run-id and machine.
        if !self.threads.contains_key(&ev.thread_id) {
            self.run_counter += 1;
            let run_id = format!("codex:{}:run-{}", ev.thread_id, self.run_counter);
            self.threads.insert(
                ev.thread_id.clone(),
                CodexThread {
                    machine: CodexStateMachine::new(ev.thread_id.clone()),
                    run_id,
                },
            );
        }
        let thread = self.threads.get_mut(&ev.thread_id).expect("just inserted");
        let transition = thread.machine.apply(ev);
        let run_id = thread.run_id.clone();

        let mut cmds = Vec::new();
        if transition.changed {
            let run = thread
                .machine
                .to_run(run_id.clone(), crate::fake::now_iso8601());
            cmds.push(ReporterCommand::UpsertRun(run));
        } else if transition.liveness {
            // Pure liveness refresh (PostToolUse / compaction): no Hub delta, just
            // keep the run from being timed out while it is genuinely busy.
            cmds.push(ReporterCommand::Liveness { run_id });
        }
        (cmds, transition)
    }

    /// Forget a thread entirely (e.g. its run was reaped as dead). A later event
    /// for the same thread starts a fresh run.
    pub fn forget(&mut self, thread_id: &str) -> bool {
        self.threads.remove(thread_id).is_some()
    }
}

#[cfg(test)]
mod tests {
    include!("codex_tests.rs");
}
