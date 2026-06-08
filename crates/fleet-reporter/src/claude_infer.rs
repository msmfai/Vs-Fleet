//! Claude **inferred waiting** detection (PLAN S16 / node `CLINFER`).
//!
//! Native-UI / no-shim path. In the native extension UI panel Claude's
//! `PermissionRequest` / `Notification` hooks do **not** fire (PLAN s1; reproduced
//! through ext v2.1.143). So `waiting` cannot be observed authoritatively the way
//! [`crate::claude_shim`] (S17) does under the shim. Only
//! `Stop`/`UserPromptSubmit`/`PreToolUse`/`SessionStart`/`SessionEnd` fire there.
//!
//! S16 **infers** waiting from a timing heuristic plus a transcript corroboration,
//! and -- by construction -- stamps it **[`Confidence::Inferred`]**, never
//! [`Confidence::High`].
//!
//! ## The `PreToolUse`-without-`Stop` debounce (the timing heuristic)
//!
//! When Claude is blocked on an approval in the native UI, no hook announces it, but
//! the hook stream is distinctive: a `PreToolUse` fires, then -- if the tool is
//! gated on a human approval -- **nothing else fires** until the human answers. So:
//! **a `PreToolUse` not followed by activity for a debounce window => infer
//! `waiting` + `approval`, `confidence: inferred`.** Any later activity (`Stop`,
//! another `PreToolUse`, `UserPromptSubmit`, `PostToolUse`) **cancels** the pending
//! inference and auto-resolves any already-raised `waiting`.
//!
//! Driven by an **injected monotonic clock** (ms timestamps on every event + an
//! explicit [`ClaudeInferMachine::tick`]). Pure and sync -- no real time, no sleeps
//! -- so the *debounce timing* is exhaustively unit-testable: a tick at
//! `t = window - 1` must not fire; a tick at `t = window` must.
//!
//! ## Transcript-JSONL corroboration (behind a schema-drift guard)
//!
//! A stuck `PreToolUse` can be corroborated or vetoed by [`corroborate_jsonl`] and
//! folded in via [`ClaudeInferMachine::corroborate`]: the last `tool_use` block
//! either has no matching `tool_result` ([`Corroboration::Stuck`]) or it does
//! ([`Corroboration::Resolved`], which **vetoes** the inference). This raises the
//! inference's *quality*, not its confidence (invariant 5). The JSONL schema is
//! community-documented/version-sensitive, so the parser is **best-effort behind a
//! schema-drift guard** that degrades to [`Corroboration::Unknown`] rather than
//! panicking or overstating (PLAN S16). `Unknown` never suppresses a real waiting;
//! only the positive `Resolved` verdict vetoes the debounce.
//!
//! ## Confidence honesty (invariant 5), structural
//!
//! No path here produces [`Confidence::High`] for a waiting state. The only `High`
//! it emits is on a confirmed `SessionEnd` exit (authoritative), like S15.

use fleet_protocol::{AgentKind, AgentRun, Confidence, Extra, State, Urgency, SCHEMA_VERSION};

use crate::claude::{ClaudeHookEvent, ClaudeHookKind};
use crate::reporter::ReporterCommand;

/// The default `PreToolUse`-without-`Stop` debounce window, in milliseconds.
pub const DEFAULT_DEBOUNCE_MS: u64 = 1_500;

/// The transcript-JSONL corroboration verdict for a pending debounce.
///
/// Produced by [`corroborate_jsonl`]. Advisory only -- never lifts confidence above
/// `Inferred`. `Unknown` is the safe degradation when the schema-drift guard cannot
/// read the transcript. Only the positive [`Resolved`](Corroboration::Resolved)
/// verdict vetoes the inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corroboration {
    /// Last `tool_use` has **no** matching `tool_result` -- consistent with "blocked
    /// on the user". Corroborates a stuck `PreToolUse`; the debounce stands.
    Stuck,
    /// Last `tool_use` **has** a matching `tool_result` -- not blocked. **Vetoes**
    /// the inference (cancels a pending debounce / auto-resolves a raised one).
    Resolved,
    /// The transcript could not be parsed into the expected shape (drift, truncation,
    /// unreadable path, no tool activity). Decide on timing alone -- `Unknown` must
    /// **never** suppress a genuine approval.
    Unknown,
}

/// Inspect a Claude transcript JSONL body and decide whether the **last** tool use
/// is still outstanding (a `tool_use` with no matching `tool_result`).
///
/// The **schema-drift guard**: parses line-by-line, skips any line it cannot
/// understand, and returns [`Corroboration::Unknown`] when it cannot reach a
/// confident verdict. Never panics, never overstates -- a `Stuck` verdict requires
/// *positively* seeing a `tool_use` whose `id` has no later `tool_result`. Tolerates
/// both `message.content[]` and a bare top-level `content[]`.
pub fn corroborate_jsonl(body: &str) -> Corroboration {
    let mut dispatched: Vec<String> = Vec::new();
    let mut completed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut saw_any_tool = false;

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let blocks = value
            .get("message")
            .and_then(|m| m.get("content"))
            .or_else(|| value.get("content"));
        let Some(serde_json::Value::Array(blocks)) = blocks else {
            continue;
        };
        for block in blocks {
            let Some(ty) = block.get("type").and_then(|t| t.as_str()) else {
                continue;
            };
            match ty {
                "tool_use" => {
                    if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                        saw_any_tool = true;
                        dispatched.push(id.to_string());
                    }
                }
                "tool_result" => {
                    if let Some(id) = block.get("tool_use_id").and_then(|i| i.as_str()) {
                        saw_any_tool = true;
                        completed.insert(id.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    if !saw_any_tool {
        return Corroboration::Unknown;
    }
    match dispatched.last() {
        Some(last) if !completed.contains(last) => Corroboration::Stuck,
        Some(_) => Corroboration::Resolved,
        None => Corroboration::Unknown,
    }
}

/// Convenience wrapper over [`corroborate_jsonl`] for a raw transcript blob.
pub fn corroborate_transcript(blob: &str) -> Corroboration {
    corroborate_jsonl(blob)
}

/// Like [`corroborate_jsonl`] but keyed on a **specific** `tool_use_id` (the
/// precise anchor a `PreToolUse` hook carries) rather than the transcript's
/// last-dispatched tool. Correct when tools run in parallel or the transcript
/// lags: we ask "did *the tool this hook armed on* get a `tool_result`?".
///
/// - dispatched (`tool_use` with this `id`) but no matching `tool_result` →
///   [`Corroboration::Stuck`];
/// - has a matching `tool_result` → [`Corroboration::Resolved`];
/// - the id is not seen at all → [`Corroboration::Unknown`] (decide on timing
///   alone — never suppress a genuine approval). Same schema-drift guard.
pub fn corroborate_jsonl_for(body: &str, tool_use_id: &str) -> Corroboration {
    let mut dispatched = false;
    let mut completed = false;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let blocks = value
            .get("message")
            .and_then(|m| m.get("content"))
            .or_else(|| value.get("content"));
        let Some(serde_json::Value::Array(blocks)) = blocks else {
            continue;
        };
        for block in blocks {
            let Some(ty) = block.get("type").and_then(|t| t.as_str()) else {
                continue;
            };
            match ty {
                "tool_use" if block.get("id").and_then(|i| i.as_str()) == Some(tool_use_id) => {
                    dispatched = true;
                }
                "tool_result"
                    if block.get("tool_use_id").and_then(|i| i.as_str()) == Some(tool_use_id) =>
                {
                    completed = true;
                }
                _ => {}
            }
        }
    }
    if !dispatched {
        return Corroboration::Unknown;
    }
    if completed {
        Corroboration::Resolved
    } else {
        Corroboration::Stuck
    }
}

/// The pure S16 **inference state machine** for one Claude `session_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeInferMachine {
    session_id: String,
    cwd: String,
    state: State,
    urgency: Option<Urgency>,
    confidence: Confidence,
    last_tool: Option<String>,
    debounce_ms: u64,
    armed_since: Option<u64>,
    inferred_waiting: bool,
    /// The `tool_use_id` of the currently-armed `PreToolUse` (the precise
    /// transcript correlation anchor), if the hook carried one. Persists while
    /// the run is armed/waiting so corroboration checks *this exact* tool, not
    /// merely the transcript's last-dispatched one.
    armed_tool_use_id: Option<String>,
}

/// A single state transition the machine decided. `changed` is `false` for a no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// The run's state after the event/tick.
    pub state: State,
    /// The run's urgency after the event/tick.
    pub urgency: Option<Urgency>,
    /// The run's confidence after the event/tick.
    pub confidence: Confidence,
    /// Whether the run-relevant fields actually changed (vs. a no-op).
    pub changed: bool,
    /// Whether this transition cleared an inferred `waiting` (auto-resolve).
    pub resolved_inference: bool,
    /// Whether this event is a pure liveness signal.
    pub liveness: bool,
}

impl ClaudeInferMachine {
    /// A new machine for a session, starting `idle`, using [`DEFAULT_DEBOUNCE_MS`].
    pub fn new(session_id: impl Into<String>) -> Self {
        Self::with_debounce_ms(session_id, DEFAULT_DEBOUNCE_MS)
    }

    /// A new machine with an explicit debounce window (for tuning / tests).
    pub fn with_debounce_ms(session_id: impl Into<String>, debounce_ms: u64) -> Self {
        ClaudeInferMachine {
            session_id: session_id.into(),
            cwd: "/".to_string(),
            state: State::Idle,
            urgency: None,
            confidence: Confidence::Inferred,
            last_tool: None,
            debounce_ms,
            armed_since: None,
            inferred_waiting: false,
            armed_tool_use_id: None,
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
    /// The run's current urgency.
    pub fn urgency(&self) -> Option<Urgency> {
        self.urgency
    }
    /// The run's current confidence.
    pub fn confidence(&self) -> Confidence {
        self.confidence
    }
    /// The session's last-known working directory.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }
    /// The configured debounce window (ms).
    pub fn debounce_ms(&self) -> u64 {
        self.debounce_ms
    }
    /// Whether a `PreToolUse` is currently armed within the debounce window.
    pub fn is_debouncing(&self) -> bool {
        self.armed_since.is_some()
    }
    /// Whether the machine is currently showing an inferred `waiting`.
    pub fn is_inferred_waiting(&self) -> bool {
        self.inferred_waiting
    }
    /// The `tool_use_id` of the currently-armed/waiting `PreToolUse`, if the hook
    /// carried one — the precise transcript correlation anchor.
    pub fn armed_tool_use_id(&self) -> Option<&str> {
        self.armed_tool_use_id.as_deref()
    }

    /// Apply a parsed lifecycle hook event at monotonic millisecond time `now_ms`.
    pub fn apply(&mut self, ev: &ClaudeHookEvent, now_ms: u64) -> Transition {
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

        match &ev.kind {
            ClaudeHookKind::PreToolUse => {
                let resolved = self.clear_inference();
                let was_working = self.state == State::Working && self.urgency.is_none();
                self.enter_working();
                self.armed_since = Some(now_ms);
                // Pin the precise correlation anchor for this arming (if present).
                self.armed_tool_use_id = ev.tool_use_id.clone();
                Transition {
                    state: self.state,
                    urgency: self.urgency,
                    confidence: self.confidence,
                    changed: !was_working || resolved,
                    resolved_inference: resolved,
                    liveness: true,
                }
            }
            ClaudeHookKind::UserPromptSubmit => {
                let resolved = self.clear_inference();
                let was_working = self.state == State::Working && self.urgency.is_none();
                self.enter_working();
                Transition {
                    state: self.state,
                    urgency: self.urgency,
                    confidence: self.confidence,
                    changed: !was_working || resolved,
                    resolved_inference: resolved,
                    liveness: true,
                }
            }
            ClaudeHookKind::Stop => {
                let resolved = self.clear_inference();
                let next = if ev.turn_complete_done && !ev.stop_hook_active {
                    State::Done
                } else {
                    State::Idle
                };
                let changed = self.state != next || self.urgency.is_some();
                self.state = next;
                self.urgency = None;
                self.confidence = Confidence::Inferred;
                Transition {
                    state: self.state,
                    urgency: self.urgency,
                    confidence: self.confidence,
                    changed: changed || resolved,
                    resolved_inference: resolved,
                    liveness: false,
                }
            }
            ClaudeHookKind::SessionEnd => {
                self.armed_since = None;
                self.inferred_waiting = false;
                let changed = self.state != State::Dead;
                self.state = State::Dead;
                self.urgency = None;
                self.confidence = Confidence::High; // confirmed exit is authoritative
                Transition {
                    state: self.state,
                    urgency: self.urgency,
                    confidence: self.confidence,
                    changed,
                    resolved_inference: false,
                    liveness: false,
                }
            }
            ClaudeHookKind::SessionStart => {
                if self.state == State::Dead {
                    self.state = State::Idle;
                    self.urgency = None;
                    self.confidence = Confidence::Inferred;
                    self.armed_since = None;
                    self.inferred_waiting = false;
                    self.no_op(false).into_changed()
                } else {
                    self.no_op(false)
                }
            }
            ClaudeHookKind::PostToolUse => {
                let resolved = self.clear_inference();
                if resolved {
                    self.enter_working();
                    Transition {
                        state: self.state,
                        urgency: self.urgency,
                        confidence: self.confidence,
                        changed: true,
                        resolved_inference: true,
                        liveness: true,
                    }
                } else {
                    self.armed_since = None;
                    self.no_op(true)
                }
            }
            ClaudeHookKind::SubagentStop | ClaudeHookKind::PreCompact => self.no_op(true),
            ClaudeHookKind::Other(_) => self.no_op(false),
        }
    }

    /// Advance the clock to `now_ms` with no new hook event. Fires the debounce: a
    /// `PreToolUse` armed for `>= debounce_ms` becomes inferred `waiting`. Idempotent
    /// once fired; a tick before the window is a no-op.
    pub fn tick(&mut self, now_ms: u64) -> Transition {
        match self.armed_since {
            Some(armed_at) if now_ms.saturating_sub(armed_at) >= self.debounce_ms => {
                self.fire_inference()
            }
            _ => self.no_op(false),
        }
    }

    /// Fold a transcript [`Corroboration`] verdict into the machine.
    ///
    /// [`Corroboration::Resolved`] **vetoes** the debounce (cancels a pending arm /
    /// auto-resolves a raised `waiting`). [`Corroboration::Stuck`] /
    /// [`Corroboration::Unknown`] leave the debounce to timing alone (never change
    /// state themselves).
    pub fn corroborate(&mut self, verdict: Corroboration) -> Transition {
        match verdict {
            Corroboration::Resolved => {
                let resolved = self.clear_inference();
                if resolved {
                    self.enter_working();
                    Transition {
                        state: self.state,
                        urgency: self.urgency,
                        confidence: self.confidence,
                        changed: true,
                        resolved_inference: true,
                        liveness: false,
                    }
                } else {
                    self.armed_since = None;
                    self.no_op(false)
                }
            }
            Corroboration::Stuck | Corroboration::Unknown => self.no_op(false),
        }
    }

    /// Build the current [`AgentRun`] snapshot.
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

    fn fire_inference(&mut self) -> Transition {
        self.armed_since = None;
        self.inferred_waiting = true;
        self.state = State::Waiting;
        self.urgency = Some(Urgency::Approval);
        self.confidence = Confidence::Inferred;
        Transition {
            state: self.state,
            urgency: self.urgency,
            confidence: self.confidence,
            changed: true,
            resolved_inference: false,
            liveness: false,
        }
    }

    fn clear_inference(&mut self) -> bool {
        self.armed_since = None;
        self.armed_tool_use_id = None;
        let was_waiting = self.inferred_waiting;
        self.inferred_waiting = false;
        was_waiting
    }

    fn enter_working(&mut self) {
        self.state = State::Working;
        self.urgency = None;
        self.confidence = Confidence::Inferred;
        self.inferred_waiting = false;
    }

    fn no_op(&self, liveness: bool) -> Transition {
        Transition {
            state: self.state,
            urgency: self.urgency,
            confidence: self.confidence,
            changed: false,
            resolved_inference: false,
            liveness,
        }
    }

    fn last_message(&self) -> Option<String> {
        match self.state {
            State::Waiting => Some(match &self.last_tool {
                Some(t) => format!("Possibly waiting on {t} (inferred)"),
                None => "Possibly waiting (inferred)".to_string(),
            }),
            State::Working => self.last_tool.as_ref().map(|t| format!("Running {t}...")),
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

/// The S16 **adapter**: maps a multiplexed stream of native-UI Claude hook events
/// plus clock ticks to [`ReporterCommand`]s, owning one [`ClaudeInferMachine`] per
/// session id.
#[derive(Debug)]
pub struct ClaudeInferAdapter {
    sessions: std::collections::HashMap<String, ClaudeInferSession>,
    run_counter: u64,
    debounce_ms: u64,
}

impl Default for ClaudeInferAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct ClaudeInferSession {
    machine: ClaudeInferMachine,
    run_id: String,
}

impl ClaudeInferAdapter {
    /// A fresh adapter using [`DEFAULT_DEBOUNCE_MS`].
    pub fn new() -> Self {
        Self::with_debounce_ms(DEFAULT_DEBOUNCE_MS)
    }

    /// A fresh adapter with an explicit debounce window.
    pub fn with_debounce_ms(debounce_ms: u64) -> Self {
        ClaudeInferAdapter {
            sessions: std::collections::HashMap::new(),
            run_counter: 0,
            debounce_ms,
        }
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
    pub fn machine_of(&self, session_id: &str) -> Option<&ClaudeInferMachine> {
        self.sessions.get(session_id).map(|s| &s.machine)
    }

    /// Ingest one **raw** recorded hook-event JSON line at time `now_ms`. Parse
    /// errors are swallowed -- a malformed line never crashes the reporter.
    pub fn ingest_json(&mut self, json: &str, now_ms: u64) -> Vec<ReporterCommand> {
        match ClaudeHookEvent::parse(json) {
            Ok(ev) => self.ingest(&ev, now_ms).0,
            Err(_) => Vec::new(),
        }
    }

    /// Ingest a parsed lifecycle hook event at time `now_ms`.
    pub fn ingest(
        &mut self,
        ev: &ClaudeHookEvent,
        now_ms: u64,
    ) -> (Vec<ReporterCommand>, Transition) {
        let session_id = ev.session_id.clone();
        self.ensure_session(&session_id);
        let session = self.sessions.get_mut(&session_id).expect("just inserted");
        let transition = session.machine.apply(ev, now_ms);
        let run_id = session.run_id.clone();
        (
            self.commands_for(&transition, &session_id, run_id),
            transition,
        )
    }

    /// Advance every tracked session's debounce clock to `now_ms`, returning any
    /// `UpsertRun`/`Liveness` commands.
    pub fn tick(&mut self, now_ms: u64) -> Vec<ReporterCommand> {
        let mut cmds = Vec::new();
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        for session_id in ids {
            let (transition, run_id) = {
                let session = self.sessions.get_mut(&session_id).expect("present");
                (session.machine.tick(now_ms), session.run_id.clone())
            };
            cmds.extend(self.commands_for(&transition, &session_id, run_id));
        }
        cmds
    }

    /// Apply a transcript corroboration verdict to one session (by id), returning any
    /// resulting commands. A `Resolved` verdict that clears a raised waiting yields an
    /// `UpsertRun`.
    pub fn corroborate(
        &mut self,
        session_id: &str,
        verdict: Corroboration,
    ) -> Vec<ReporterCommand> {
        let (transition, run_id) = match self.sessions.get_mut(session_id) {
            Some(session) => (session.machine.corroborate(verdict), session.run_id.clone()),
            None => return Vec::new(),
        };
        self.commands_for(&transition, session_id, run_id)
    }

    /// Corroborate a session against a raw transcript `blob`, automatically using
    /// the **precise** `tool_use_id` correlation ([`corroborate_jsonl_for`]) when
    /// the armed `PreToolUse` carried one, and falling back to the last-dispatched
    /// heuristic ([`corroborate_jsonl`]) otherwise. The caller just hands over the
    /// transcript; the right correlation strategy is chosen here.
    pub fn corroborate_blob(&mut self, session_id: &str, blob: &str) -> Vec<ReporterCommand> {
        let verdict = match self.sessions.get(session_id) {
            Some(s) => match s.machine.armed_tool_use_id() {
                Some(id) => corroborate_jsonl_for(blob, id),
                None => corroborate_jsonl(blob),
            },
            None => return Vec::new(),
        };
        self.corroborate(session_id, verdict)
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
                ClaudeInferSession {
                    machine: ClaudeInferMachine::with_debounce_ms(
                        session_id.to_string(),
                        self.debounce_ms,
                    ),
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
    include!("claude_infer_tests.rs");
}
