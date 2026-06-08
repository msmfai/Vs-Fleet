//! Claude high-confidence detection in the **shim terminal** (PLAN S17 / node
//! CLUSETERM).
//!
//! This is the *Use-Terminal mode* Claude adapter. It is the high-confidence
//! sibling of the S15 [`crate::claude`] hooks adapter, and it depends — per the
//! work graph — on both `SHIM` (S10, the PATH-shim `claude` wrapper) and `CLHOOK`
//! (S15). What S15 deliberately *cannot* do — model `waiting`/approval — this
//! module does, but **only when the launch context proves the shim is in force**.
//!
//! ## Why a launch-context-aware adapter (PLAN §1, the confidence boundary)
//!
//! Claude's hook surface is **surface-dependent** (reproduced through ext
//! v2.1.143, May 2026):
//!
//! - **Integrated-terminal, launched via the Fleet PATH shim** (= "Use-Terminal
//!   mode", S17). The shim launches the *real* `claude` with injected
//!   `--session-id` / `--settings` (and any opt-in reliability flags surfaced to
//!   the user, never silent — §3 invariant 3 / PLAN §1). In this surface Claude's
//!   **`PermissionRequest` hook fires** and is an authoritative "I am blocked on
//!   you" signal. ⇒ `waiting` + `approval`, **[`Confidence::High`]**.
//! - **Native extension UI panel** (no shell, no shim) or **outside the editor**.
//!   `Notification`, `PermissionRequest`, *and* `PostToolUse` do **not** fire
//!   (anthropics/claude-code #31285 / PLAN §1). So waiting cannot be observed
//!   authoritatively; it is *inferred* by S16 (`CLINFER`) ⇒ **[`Confidence::Inferred`]**.
//!
//! The single locked invariant this module exists to enforce (confidence honesty,
//! §3 invariant 5): **the *same* `PermissionRequest` approval payload yields
//! `high` confidence under the shim and `inferred` confidence in the native-UI
//! surface.** That is the S17 acceptance, and it is asserted both as a unit test
//! and as a structural property (`High` is reachable **only** through
//! [`LaunchContext::ShimTerminal`] + `PermissionRequest`, plus the confirmed-exit
//! `SessionEnd` edge).
//!
//! ## Observer, not owner (§3 invariant 3)
//!
//! S17 only *reads* the launch environment the shim established (the injected
//! `FLEET_*` env + the `--session-id`/`--settings` the wrapper passed). It never
//! intercepts keystrokes, never launches Claude *through* Fleet, and never
//! silently flips a user into `bypassPermissions`. The reliability flags the shim
//! may pass are opt-in and surfaced by the extension (S10), not by this adapter.
//!
//! The module is pure and sync (no I/O, no async, no Hub dependency at the
//! state-machine layer) so every transition is exhaustively unit-testable against
//! **recorded Claude hook-event JSON fixtures** — both a native-UI fixture and a
//! shim fixture of the *same* approval.

use serde::Deserialize;

use fleet_protocol::{AgentKind, AgentRun, Confidence, Extra, State, Urgency, SCHEMA_VERSION};

use crate::claude::{ClaudeHookEvent, ClaudeHookKind, ClaudeParseError};
use crate::reporter::ReporterCommand;

/// How a Claude run was launched, which **determines whether `PermissionRequest`
/// is an authoritative signal** (PLAN §1 confidence boundary).
///
/// This is supplied out-of-band by the extension/shim (S9/S10): the integrated
/// terminal that ran the PATH-shim `claude` wrapper is `ShimTerminal`; everything
/// else (native UI panel, plain external shell with no shim) is `NativeUi`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchContext {
    /// Launched via the Fleet PATH-shim `claude` wrapper in the editor's
    /// integrated terminal (Use-Terminal mode). `PermissionRequest` fires and is
    /// authoritative ⇒ a fresh approval is **[`Confidence::High`]**.
    ShimTerminal,
    /// Launched in the native extension UI panel, or in a shell with no shim
    /// applied. `PermissionRequest` does **not** fire here; if a waiting signal is
    /// ever derived it is a heuristic ⇒ **[`Confidence::Inferred`]** (and is owned
    /// by S16/`CLINFER`, not this module).
    NativeUi,
}

impl LaunchContext {
    /// `true` when `PermissionRequest` is an authoritative waiting signal in this
    /// surface (i.e. only under the shim terminal). This is the *one* predicate
    /// that gates [`Confidence::High`] — confidence honesty made structural.
    pub fn permission_request_is_authoritative(self) -> bool {
        matches!(self, LaunchContext::ShimTerminal)
    }

    /// The confidence a `PermissionRequest`-derived `waiting` carries in this
    /// surface. **The only place a heuristic→`High` upgrade could leak, isolated to
    /// one branch and exhaustively tested.**
    pub fn approval_confidence(self) -> Confidence {
        if self.permission_request_is_authoritative() {
            Confidence::High
        } else {
            Confidence::Inferred
        }
    }

    /// Detect the launch context from the shim-injected environment. The S10 shim
    /// sets `FLEET_SHIM=claude` (and `FLEET_SESSION_ID`) in the integrated-terminal
    /// shell it wraps; its absence means we are not under the shim → `NativeUi`.
    ///
    /// Pure over an explicit lookup so it is testable without touching the real
    /// process environment.
    pub fn from_env<'a>(lookup: impl Fn(&str) -> Option<&'a str>) -> Self {
        match lookup("FLEET_SHIM") {
            Some(v) if v.eq_ignore_ascii_case("claude") || v.eq_ignore_ascii_case("1") => {
                LaunchContext::ShimTerminal
            }
            _ => LaunchContext::NativeUi,
        }
    }
}

/// A `PermissionRequest` event's approval payload, parsed from the recorded hook
/// JSON. Claude's `PermissionRequest` stdin carries the `tool_name` (and a
/// `permission` / `decision` envelope on the *response*), mirroring the Codex
/// shape so the two adapters stay structurally aligned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// Claude `session_id` — the durable identity anchor (same one S15 uses).
    pub session_id: String,
    /// The working directory, if present.
    pub cwd: Option<String>,
    /// The tool the agent is blocked on, if named.
    pub tool_name: Option<String>,
    /// `Some` when this event is the **response** to the approval (auto-resolve),
    /// `None` when it is a fresh request.
    pub decision: Option<ApprovalDecision>,
}

/// The user's answer to a Claude `PermissionRequest`. Its presence on an event
/// means "this resolves the approval" (auto-resolve, mirrors Codex S13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// The user allowed the tool to run.
    Allow,
    /// The user denied the tool.
    Deny,
}

/// Raw `PermissionRequest` payload shape. Only the approval-specific fields beyond
/// the common S15 hook fields are surfaced here; identity/cwd/tool reuse the S15
/// parser via [`ClaudeHookEvent`].
#[derive(Debug, Deserialize)]
struct RawPermission {
    #[serde(alias = "permission", alias = "response", alias = "answer")]
    decision: Option<RawDecision>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawDecision {
    Plain(String),
    Structured {
        permission: Option<String>,
        decision: Option<String>,
        behavior: Option<String>,
    },
}

impl RawDecision {
    fn into_decision(self) -> Option<ApprovalDecision> {
        let token = match self {
            RawDecision::Plain(s) => Some(s),
            RawDecision::Structured {
                permission,
                decision,
                behavior,
            } => permission.or(decision).or(behavior),
        }?;
        match token.to_ascii_lowercase().as_str() {
            "allow" | "approve" | "approved" | "accept" | "yes" | "once" | "always" => {
                Some(ApprovalDecision::Allow)
            }
            "deny" | "denied" | "reject" | "rejected" | "no" | "abort" => {
                Some(ApprovalDecision::Deny)
            }
            _ => None,
        }
    }
}

impl ApprovalRequest {
    /// Parse a recorded Claude `PermissionRequest` hook-event JSON into an
    /// [`ApprovalRequest`]. Reuses the S15 [`ClaudeHookEvent`] parser for the
    /// common fields (identity/cwd/tool, schema-drift tolerant), then layers the
    /// approval-specific `decision` envelope on top.
    ///
    /// Returns `None` (not an error) if the event is not a `PermissionRequest`, so
    /// a caller can cheaply filter a mixed hook stream. Hard parse failures
    /// (invalid JSON, missing identity) surface as [`ClaudeParseError`].
    pub fn parse(json: &str) -> Result<Option<Self>, ClaudeParseError> {
        let ev = ClaudeHookEvent::parse(json)?;
        // Only a PermissionRequest event yields an ApprovalRequest.
        if !is_permission_request(&ev.kind) {
            return Ok(None);
        }
        // Best-effort decision parse; a malformed/absent decision ⇒ fresh request.
        let decision = serde_json::from_str::<RawPermission>(json)
            .ok()
            .and_then(|r| r.decision)
            .and_then(RawDecision::into_decision);
        Ok(Some(ApprovalRequest {
            session_id: ev.session_id,
            cwd: ev.cwd,
            tool_name: ev.tool_name,
            decision,
        }))
    }

    /// `true` when this is the **response** to an approval (carries a decision),
    /// vs a fresh request.
    pub fn is_response(&self) -> bool {
        self.decision.is_some()
    }
}

/// Claude does not have a dedicated `PermissionRequest` variant in the S15
/// [`ClaudeHookKind`] enum (S15 never models waiting), so it parses to
/// `Other("PermissionRequest")`. This helper recognises it without S17 having to
/// fork the S15 enum.
fn is_permission_request(kind: &ClaudeHookKind) -> bool {
    matches!(kind, ClaudeHookKind::Other(n) if n == "PermissionRequest")
}

/// The shim-aware Claude **state machine** for one `session_id` (S17).
///
/// It is constructed *with* a [`LaunchContext`], which is fixed for the life of
/// the run (a run does not migrate between the native UI and the shim terminal).
/// It models the full `working`/`idle`/`done`/`dead` lifecycle exactly like S15
/// **plus** the `waiting`/approval path S15 cannot — and stamps the approval's
/// confidence from the launch context, which is the entire point of S17.
///
/// Guarantees (gate G2 + invariant 5):
/// - **No illegal transition** — every edge is modelled; an inapplicable event is
///   an idempotent no-op.
/// - **Confidence honesty (structural)** — [`Confidence::High`] for `waiting` is
///   reachable **only** when `ctx.permission_request_is_authoritative()` (i.e.
///   `ShimTerminal`). In `NativeUi` the *same* approval yields `Inferred`.
/// - **Auto-resolve** — answering in the terminal (the `PermissionRequest`
///   response, or any subsequent activity) clears `waiting` → `working`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeShimStateMachine {
    session_id: String,
    ctx: LaunchContext,
    cwd: String,
    state: State,
    urgency: Option<Urgency>,
    confidence: Confidence,
    last_tool: Option<String>,
    pending_approval: bool,
}

/// A single transition decision, mirroring the Codex/S15 transition shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// The run's state after the event.
    pub state: State,
    /// The run's urgency after the event.
    pub urgency: Option<Urgency>,
    /// The run's confidence after the event.
    pub confidence: Confidence,
    /// Whether the run-relevant fields actually changed (vs. a no-op).
    pub changed: bool,
    /// Whether this transition cleared a pending approval (auto-resolve / answer).
    pub resolved_approval: bool,
    /// Whether this event is a pure liveness signal.
    pub liveness: bool,
}

impl ClaudeShimStateMachine {
    /// A new machine for a session in the given launch context, starting `idle`.
    pub fn new(session_id: impl Into<String>, ctx: LaunchContext) -> Self {
        ClaudeShimStateMachine {
            session_id: session_id.into(),
            ctx,
            cwd: "/".to_string(),
            state: State::Idle,
            urgency: None,
            confidence: Confidence::Inferred,
            last_tool: None,
            pending_approval: false,
        }
    }

    /// The session's durable id (Claude `session_id`).
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    /// The launch context this run is fixed to.
    pub fn context(&self) -> LaunchContext {
        self.ctx
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
    /// The session's last-known working directory.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Apply a parsed S15 hook event (the lifecycle hooks: start/prompt/tool/stop/
    /// end). Approval events are applied separately via [`Self::apply_approval`]
    /// because S15's [`ClaudeHookEvent`] does not carry the decision envelope.
    pub fn apply(&mut self, ev: &ClaudeHookEvent) -> Transition {
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

        // A PermissionRequest reaching the lifecycle path (e.g. routed as Other)
        // is handled as a fresh approval request.
        if is_permission_request(&ev.kind) {
            return self.raise_approval();
        }

        match &ev.kind {
            ClaudeHookKind::UserPromptSubmit | ClaudeHookKind::PreToolUse => {
                let resolved = self.pending_approval;
                let was_working = self.state == State::Working && self.urgency.is_none();
                self.enter_working();
                Transition {
                    state: self.state,
                    urgency: self.urgency,
                    confidence: self.confidence,
                    changed: !was_working || resolved,
                    resolved_approval: resolved,
                    liveness: true,
                }
            }
            ClaudeHookKind::Stop => {
                let resolved = self.pending_approval;
                let next = if ev.turn_complete_done && !ev.stop_hook_active {
                    State::Done
                } else {
                    State::Idle
                };
                let changed = self.state != next || self.urgency.is_some();
                self.state = next;
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
            ClaudeHookKind::SessionEnd => {
                let changed = self.state != State::Dead;
                self.state = State::Dead;
                self.urgency = None;
                self.confidence = Confidence::High; // confirmed exit is authoritative
                self.pending_approval = false;
                Transition {
                    state: self.state,
                    urgency: self.urgency,
                    confidence: self.confidence,
                    changed,
                    resolved_approval: false,
                    liveness: false,
                }
            }
            ClaudeHookKind::SessionStart => {
                if self.state == State::Dead {
                    self.state = State::Idle;
                    self.urgency = None;
                    self.confidence = Confidence::Inferred;
                    self.pending_approval = false;
                    self.no_op(false).into_changed()
                } else {
                    self.no_op(false)
                }
            }
            ClaudeHookKind::PostToolUse
            | ClaudeHookKind::SubagentStop
            | ClaudeHookKind::PreCompact => self.no_op(true),
            ClaudeHookKind::Other(_) => self.no_op(false),
        }
    }

    /// Apply a parsed [`ApprovalRequest`] — the S17-specific path. A fresh request
    /// raises `waiting`+`approval` with the launch-context confidence; a response
    /// (decision present) auto-resolves it back to `working`.
    pub fn apply_approval(&mut self, req: &ApprovalRequest) -> Transition {
        if req.session_id != self.session_id {
            return self.no_op(false);
        }
        if let Some(c) = &req.cwd {
            if !c.is_empty() {
                self.cwd = c.clone();
            }
        }
        if let Some(t) = &req.tool_name {
            self.last_tool = Some(t.clone());
        }
        if let Some(decision) = req.decision {
            // The user's answer (auto-resolve, S13-analog): both allow/deny resume.
            let _ = decision;
            let was_pending = self.pending_approval;
            self.enter_working();
            Transition {
                state: self.state,
                urgency: self.urgency,
                confidence: self.confidence,
                changed: was_pending,
                resolved_approval: was_pending,
                liveness: true,
            }
        } else {
            self.raise_approval()
        }
    }

    /// Build the current [`AgentRun`] snapshot. `native_id` is the Claude
    /// `session_id` durable anchor (S15-identical), so the reporter framework keys
    /// identity on it regardless of launch context.
    pub fn to_run(&self, run_id: impl Into<String>, updated_at: impl Into<String>) -> AgentRun {
        let updated_at = updated_at.into();
        AgentRun {
            schema_version: SCHEMA_VERSION,
            run_id: run_id.into(),
            agent_kind: AgentKind::ClaudeCode,
            native_id: self.session_id.clone(),
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

    fn raise_approval(&mut self) -> Transition {
        self.state = State::Waiting;
        self.urgency = Some(Urgency::Approval);
        // THE confidence boundary: High iff the shim makes PermissionRequest
        // authoritative; Inferred in the native UI. This is the only High-from-
        // waiting path in the whole adapter.
        self.confidence = self.ctx.approval_confidence();
        self.pending_approval = true;
        Transition {
            state: self.state,
            urgency: self.urgency,
            confidence: self.confidence,
            changed: true,
            resolved_approval: false,
            liveness: true,
        }
    }

    fn enter_working(&mut self) {
        self.state = State::Working;
        self.urgency = None;
        self.confidence = Confidence::Inferred;
        self.pending_approval = false;
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
            State::Done => Some("Task complete.".to_string()),
            State::Dead => Some("Session closed.".to_string()),
            _ => None,
        }
    }
}

impl Transition {
    fn into_changed(mut self) -> Self {
        self.changed = true;
        self
    }
}

/// The shim-aware Claude **adapter** (S17): maps a multiplexed stream of Claude
/// hook events — lifecycle hooks *and* `PermissionRequest` approval payloads — to
/// [`ReporterCommand`]s, owning one [`ClaudeShimStateMachine`] per `session_id`.
///
/// The adapter is created with a [`LaunchContext`] that applies to every session
/// it tracks (one adapter instance per integrated-terminal shell, whose context
/// the extension knows). It mirrors [`crate::claude::ClaudeAdapter`] /
/// [`crate::codex::CodexAdapter`] so it reuses the gated REPCORE/IDENTITY plumbing
/// verbatim.
#[derive(Debug)]
pub struct ClaudeShimAdapter {
    ctx: LaunchContext,
    sessions: std::collections::HashMap<String, ClaudeShimSession>,
    run_counter: u64,
}

#[derive(Debug)]
struct ClaudeShimSession {
    machine: ClaudeShimStateMachine,
    run_id: String,
}

impl ClaudeShimAdapter {
    /// A fresh adapter for runs launched in the given context.
    pub fn new(ctx: LaunchContext) -> Self {
        ClaudeShimAdapter {
            ctx,
            sessions: std::collections::HashMap::new(),
            run_counter: 0,
        }
    }

    /// The launch context every session in this adapter is fixed to.
    pub fn context(&self) -> LaunchContext {
        self.ctx
    }

    /// Number of distinct sessions currently tracked.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// The current run state for a session, if tracked.
    pub fn state_of(&self, session_id: &str) -> Option<State> {
        self.sessions.get(session_id).map(|s| s.machine.state())
    }

    /// The current confidence for a session, if tracked.
    pub fn confidence_of(&self, session_id: &str) -> Option<Confidence> {
        self.sessions
            .get(session_id)
            .map(|s| s.machine.confidence())
    }

    /// The Fleet run-id minted for a session, if tracked.
    pub fn run_id_of(&self, session_id: &str) -> Option<&str> {
        self.sessions.get(session_id).map(|s| s.run_id.as_str())
    }

    /// Borrow a session's state machine (tests).
    pub fn machine_of(&self, session_id: &str) -> Option<&ClaudeShimStateMachine> {
        self.sessions.get(session_id).map(|s| &s.machine)
    }

    /// Ingest one **raw** recorded hook-event JSON line, dispatching by event kind:
    /// a `PermissionRequest` goes through the approval path; everything else through
    /// the lifecycle path. Parse errors are swallowed — a malformed line must never
    /// crash the reporter or overstate state.
    pub fn ingest_json(&mut self, json: &str) -> Vec<ReporterCommand> {
        // Try the approval path first (PermissionRequest only).
        match ApprovalRequest::parse(json) {
            Ok(Some(req)) => return self.ingest_approval(&req).0,
            Ok(None) => {}
            Err(_) => return Vec::new(),
        }
        match ClaudeHookEvent::parse(json) {
            Ok(ev) => self.ingest(&ev).0,
            Err(_) => Vec::new(),
        }
    }

    /// Ingest a parsed lifecycle hook event.
    pub fn ingest(&mut self, ev: &ClaudeHookEvent) -> (Vec<ReporterCommand>, Transition) {
        let session_id = ev.session_id.clone();
        self.ensure_session(&session_id);
        let session = self.sessions.get_mut(&session_id).expect("just inserted");
        let transition = session.machine.apply(ev);
        let run_id = session.run_id.clone();
        (
            self.commands_for(&transition, &session_id, run_id),
            transition,
        )
    }

    /// Ingest a parsed approval request/response.
    pub fn ingest_approval(&mut self, req: &ApprovalRequest) -> (Vec<ReporterCommand>, Transition) {
        let session_id = req.session_id.clone();
        self.ensure_session(&session_id);
        let session = self.sessions.get_mut(&session_id).expect("just inserted");
        let transition = session.machine.apply_approval(req);
        let run_id = session.run_id.clone();
        (
            self.commands_for(&transition, &session_id, run_id),
            transition,
        )
    }

    /// Forget a session entirely.
    pub fn forget(&mut self, session_id: &str) -> bool {
        self.sessions.remove(session_id).is_some()
    }

    fn ensure_session(&mut self, session_id: &str) {
        if !self.sessions.contains_key(session_id) {
            self.run_counter += 1;
            let run_id = format!("claude:{session_id}:run-{}", self.run_counter);
            self.sessions.insert(
                session_id.to_string(),
                ClaudeShimSession {
                    machine: ClaudeShimStateMachine::new(session_id.to_string(), self.ctx),
                    run_id,
                },
            );
        }
    }

    fn commands_for(
        &self,
        transition: &Transition,
        session_id: &str,
        run_id: String,
    ) -> Vec<ReporterCommand> {
        let mut cmds = Vec::new();
        if transition.changed {
            let machine = &self.sessions.get(session_id).expect("present").machine;
            let run = machine.to_run(run_id, crate::fake::now_iso8601());
            cmds.push(ReporterCommand::UpsertRun(run));
        } else if transition.liveness {
            cmds.push(ReporterCommand::Liveness { run_id });
        }
        cmds
    }
}

#[cfg(test)]
mod tests {
    include!("claude_shim_tests.rs");
}
