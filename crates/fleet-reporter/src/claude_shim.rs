//! Claude high-confidence detection in the **shim terminal** (the engineering spec / node
//! CLUSETERM).
//!
//! This is the *Use-Terminal mode* Claude adapter. It is the high-confidence
//! sibling of the S15 [`crate::claude`] hooks adapter, and it depends — per the
//! work graph — on both `SHIM` (S10, the PATH-shim `claude` wrapper) and `CLHOOK`
//! (S15). What S15 deliberately *cannot* do — model `waiting`/approval — this
//! module does, but **only when the launch context proves the shim is in force**.
//!
//! ## Why a launch-context-aware adapter (engineering spec §1, the confidence boundary)
//!
//! Claude's hook surface is **surface-dependent** (reproduced through ext
//! v2.1.143, May 2026):
//!
//! - **Integrated-terminal, launched via the Fleet PATH shim** (= "Use-Terminal
//!   mode", S17). The shim launches the *real* `claude` with injected
//!   `--session-id` / `--settings` (and any opt-in reliability flags surfaced to
//!   the user, never silent — §3 invariant 3 / engineering spec §1). In this surface Claude's
//!   **`PermissionRequest` hook fires** and is an authoritative "I am blocked on
//!   you" signal. ⇒ `waiting` + `approval`, **[`Confidence::High`]**.
//! - **Native extension UI panel** (no shell, no shim) or **outside the editor**.
//!   `Notification`, `PermissionRequest`, *and* `PostToolUse` do **not** fire
//!   (anthropics/claude-code #31285 / engineering spec §1). So waiting cannot be observed
//!   authoritatively; it is *inferred* by S16 ([`crate::claude_infer`], `CLINFER`)
//!   ⇒ **[`Confidence::Inferred`]**.
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

use fleet_protocol::{AgentKind, AgentRun, Confidence, State, Urgency};

use crate::claude::{ClaudeHookEvent, ClaudeHookKind, ClaudeParseError};
pub use crate::machine::Transition;
use crate::machine::{AdapterCore, Core};
use crate::reporter::ReporterCommand;

/// How a Claude run was launched, which **determines whether `PermissionRequest`
/// is an authoritative signal** (engineering spec §1 confidence boundary).
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
    /// by S16/`CLINFER` — [`crate::claude_infer`] — not this module).
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
/// JSON. Claude's `PermissionRequest` stdin carries the `tool_name` the agent is
/// blocked on, mirroring the Codex shape so the two adapters stay structurally
/// aligned.
///
/// **There is no inbound "response" event.** A `PermissionRequest` `decision` is
/// a hook *output* (`hookSpecificOutput.decision.behavior`), not a second inbound
/// hook. A real approval resolves via the *subsequent activity* hooks (a
/// `PreToolUse`/`UserPromptSubmit`/`Stop`), which the lifecycle path already
/// auto-resolves — so this type models the request only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// Claude `session_id` — the durable identity anchor (same one S15 uses).
    pub session_id: String,
    /// The working directory, if present.
    pub cwd: Option<String>,
    /// The tool the agent is blocked on, if named.
    pub tool_name: Option<String>,
}

impl ApprovalRequest {
    /// Parse a recorded Claude `PermissionRequest` hook-event JSON into an
    /// [`ApprovalRequest`]. Reuses the S15 [`ClaudeHookEvent`] parser for the
    /// common fields (identity/cwd/tool, schema-drift tolerant).
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
        Ok(Some(Self::from_event(ev)))
    }

    /// Build an [`ApprovalRequest`] from an already-parsed [`ClaudeHookEvent`]
    /// known to be a `PermissionRequest`.
    fn from_event(ev: ClaudeHookEvent) -> Self {
        ApprovalRequest {
            session_id: ev.session_id,
            cwd: ev.cwd,
            tool_name: ev.tool_name,
        }
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
/// Guarantees (invariant 5):
/// - **No illegal transition** — every edge is modelled; an inapplicable event is
///   an idempotent no-op.
/// - **Confidence honesty (structural)** — [`Confidence::High`] for `waiting` is
///   reachable **only** when `ctx.permission_request_is_authoritative()` (i.e.
///   `ShimTerminal`). In `NativeUi` the *same* approval yields `Inferred`.
/// - **Auto-resolve** — answering in the terminal produces the *next* activity
///   hook (a `PreToolUse`/`UserPromptSubmit`/`Stop`), which clears `waiting` →
///   `working`/`done`. There is no inbound approval-response event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeShimStateMachine {
    /// The shared lifecycle state (session_id anchor, cwd, state, urgency,
    /// confidence, last tool, pending-approval).
    core: Core,
    /// The launch context this run is fixed to — the confidence boundary for a
    /// raised approval (S17-specific).
    ctx: LaunchContext,
}

impl ClaudeShimStateMachine {
    /// A new machine for a session in the given launch context, starting `idle`.
    pub fn new(session_id: impl Into<String>, ctx: LaunchContext) -> Self {
        ClaudeShimStateMachine {
            core: Core::new(session_id),
            ctx,
        }
    }

    /// The session's durable id (Claude `session_id`).
    pub fn session_id(&self) -> &str {
        self.core.native_id()
    }
    /// The launch context this run is fixed to.
    pub fn context(&self) -> LaunchContext {
        self.ctx
    }
    /// The run's current state.
    pub fn state(&self) -> State {
        self.core.state()
    }
    /// The run's current urgency.
    pub fn urgency(&self) -> Option<Urgency> {
        self.core.urgency()
    }
    /// The run's current confidence.
    pub fn confidence(&self) -> Confidence {
        self.core.confidence()
    }
    /// Whether an approval is outstanding.
    pub fn awaiting_approval(&self) -> bool {
        self.core.pending_approval()
    }
    /// The session's last-known working directory.
    pub fn cwd(&self) -> &str {
        self.core.cwd()
    }

    /// Apply a parsed S15 hook event (the lifecycle hooks: start/prompt/tool/stop/
    /// end). A `PermissionRequest` is raised as a fresh approval here too; the
    /// dedicated [`Self::apply_approval`] path is the same request-only entry.
    pub fn apply(&mut self, ev: &ClaudeHookEvent) -> Transition {
        if ev.session_id != self.core.native_id() {
            return self.core.no_op(false);
        }
        self.core.note_cwd(ev.cwd.as_deref());
        self.core.note_tool(ev.tool_name.as_deref());

        // A PermissionRequest reaching the lifecycle path (e.g. routed as Other)
        // is handled as a fresh approval request.
        if is_permission_request(&ev.kind) {
            return self.raise_approval();
        }

        match &ev.kind {
            ClaudeHookKind::UserPromptSubmit | ClaudeHookKind::PreToolUse => {
                let resolved = self.core.pending_approval();
                let was_working =
                    self.core.state() == State::Working && self.core.urgency().is_none();
                self.core.enter_working();
                self.core
                    .transition(!was_working || resolved, resolved, true)
            }
            ClaudeHookKind::Stop => {
                let resolved = self.core.pending_approval();
                if ev.stop_hook_active {
                    // A continuation Stop is not a real turn boundary — activity,
                    // not completion: cancel any pending approval, stay working.
                    let was_working =
                        self.core.state() == State::Working && self.core.urgency().is_none();
                    self.core.enter_working();
                    return self
                        .core
                        .transition(!was_working || resolved, resolved, true);
                }
                // The Stop event firing IS the turn-complete signal → Done.
                let changed = self.core.state() != State::Done || self.core.urgency().is_some();
                self.core.set_done();
                self.core.transition(changed || resolved, resolved, false)
            }
            ClaudeHookKind::SessionEnd => {
                let changed = self.core.state() != State::Dead;
                self.core.set_dead(); // confirmed exit is authoritative (High)
                self.core.transition(changed, false, false)
            }
            ClaudeHookKind::SessionStart => {
                if self.core.state() == State::Dead {
                    self.core.revive_idle();
                    self.core.no_op(false).into_changed()
                } else {
                    self.core.no_op(false)
                }
            }
            ClaudeHookKind::PostToolUse
            | ClaudeHookKind::SubagentStop
            | ClaudeHookKind::PreCompact => self.core.no_op(true),
            ClaudeHookKind::Other(_) => self.core.no_op(false),
        }
    }

    /// Apply a parsed [`ApprovalRequest`] — the S17-specific path. A
    /// `PermissionRequest` is always a **fresh** request (there is no inbound
    /// response event); it raises `waiting`+`approval` with the launch-context
    /// confidence and auto-resolves later via the subsequent activity hooks
    /// (handled in [`Self::apply`]).
    pub fn apply_approval(&mut self, req: &ApprovalRequest) -> Transition {
        if req.session_id != self.core.native_id() {
            return self.core.no_op(false);
        }
        self.core.note_cwd(req.cwd.as_deref());
        self.core.note_tool(req.tool_name.as_deref());
        self.raise_approval()
    }

    /// Build the current [`AgentRun`] snapshot. `native_id` is the Claude
    /// `session_id` durable anchor (S15-identical), so the reporter framework keys
    /// identity on it regardless of launch context.
    pub fn to_run(&self, run_id: impl Into<String>, updated_at: impl Into<String>) -> AgentRun {
        self.core.to_run(
            AgentKind::ClaudeCode,
            run_id.into(),
            updated_at.into(),
            self.last_message(),
        )
    }

    // ── internal transitions ─────────────────────────────────────────────────

    fn raise_approval(&mut self) -> Transition {
        // THE confidence boundary: High iff the shim makes PermissionRequest
        // authoritative; Inferred in the native UI. This is the only High-from-
        // waiting path in the whole adapter.
        self.core
            .set_waiting_approval(self.ctx.approval_confidence());
        self.core.transition(true, false, true)
    }

    fn last_message(&self) -> Option<String> {
        match self.core.state() {
            State::Waiting => Some(match self.core.last_tool() {
                Some(t) => format!("Approve {t}?"),
                None => "Approval required".to_string(),
            }),
            State::Working => self.core.last_tool().map(|t| format!("Running {t}…")),
            State::Done => Some("Task complete.".to_string()),
            State::Dead => Some("Session closed.".to_string()),
            _ => None,
        }
    }
}

impl crate::machine::RunMachine for ClaudeShimStateMachine {
    fn run_state(&self) -> State {
        self.core.state()
    }
    fn build_run(&self, run_id: String, updated_at: String) -> AgentRun {
        self.to_run(run_id, updated_at)
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
    core: AdapterCore<ClaudeShimStateMachine>,
}

impl ClaudeShimAdapter {
    /// A fresh adapter for runs launched in the given context.
    pub fn new(ctx: LaunchContext) -> Self {
        ClaudeShimAdapter {
            ctx,
            core: AdapterCore::new("claude"),
        }
    }

    /// The launch context every session in this adapter is fixed to.
    pub fn context(&self) -> LaunchContext {
        self.ctx
    }

    /// Number of distinct sessions currently tracked.
    pub fn session_count(&self) -> usize {
        self.core.session_count()
    }

    /// The current run state for a session, if tracked.
    pub fn state_of(&self, session_id: &str) -> Option<State> {
        self.core.state_of(session_id)
    }

    /// The current confidence for a session, if tracked.
    pub fn confidence_of(&self, session_id: &str) -> Option<Confidence> {
        self.core.machine_of(session_id).map(|m| m.confidence())
    }

    /// The Fleet run-id minted for a session, if tracked.
    pub fn run_id_of(&self, session_id: &str) -> Option<&str> {
        self.core.run_id_of(session_id)
    }

    /// Borrow a session's state machine (tests).
    pub fn machine_of(&self, session_id: &str) -> Option<&ClaudeShimStateMachine> {
        self.core.machine_of(session_id)
    }

    /// Ingest one **raw** recorded hook-event JSON line, dispatching by event kind:
    /// a `PermissionRequest` goes through the approval path; everything else through
    /// the lifecycle path. Parse errors are swallowed — a malformed line must never
    /// crash the reporter or overstate state.
    pub fn ingest_json(&mut self, json: &str) -> Vec<ReporterCommand> {
        // Parse the common hook envelope exactly once. A malformed line (invalid
        // JSON / missing identity) is swallowed — it must never crash the reporter
        // or overstate state.
        let Ok(ev) = ClaudeHookEvent::parse(json) else {
            return Vec::new();
        };
        // A PermissionRequest goes through the approval (request-only) path;
        // everything else through the lifecycle path.
        if is_permission_request(&ev.kind) {
            let req = ApprovalRequest::from_event(ev);
            self.ingest_approval(&req).0
        } else {
            self.ingest(&ev).0
        }
    }

    /// Ingest a parsed lifecycle hook event.
    pub fn ingest(&mut self, ev: &ClaudeHookEvent) -> (Vec<ReporterCommand>, Transition) {
        let ctx = self.ctx;
        self.core.apply_and_commands(
            &ev.session_id,
            || ClaudeShimStateMachine::new(ev.session_id.clone(), ctx),
            |m| m.apply(ev),
        )
    }

    /// Ingest a parsed approval request.
    pub fn ingest_approval(&mut self, req: &ApprovalRequest) -> (Vec<ReporterCommand>, Transition) {
        let ctx = self.ctx;
        self.core.apply_and_commands(
            &req.session_id,
            || ClaudeShimStateMachine::new(req.session_id.clone(), ctx),
            |m| m.apply_approval(req),
        )
    }

    /// Forget a session entirely.
    pub fn forget(&mut self, session_id: &str) -> bool {
        self.core.forget(session_id)
    }
}

#[cfg(test)]
mod tests {
    include!("claude_shim_tests.rs");
}
