//! Claude **inferred waiting** detection.
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
//! panicking or overstating. `Unknown` never suppresses a real waiting;
//! only the positive `Resolved` verdict vetoes the debounce.
//!
//! ## Confidence honesty (invariant 5), structural
//!
//! No path here produces [`Confidence::High`] for a waiting state. The only `High`
//! it emits is on a confirmed `SessionEnd` exit (authoritative), like S15.

use fleet_protocol::{AgentKind, AgentRun, Confidence, State, Urgency};

use crate::claude::{ClaudeHookEvent, ClaudeHookKind};
pub use crate::machine::Transition;
use crate::machine::{AdapterCore, Core};
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
    /// The shared lifecycle state (session_id anchor, cwd, state, urgency,
    /// confidence, last tool). S16 tracks *waiting* via its own debounce fields
    /// below, not the core's `pending_approval`.
    core: Core,
    debounce_ms: u64,
    armed_since: Option<u64>,
    inferred_waiting: bool,
    /// The `tool_use_id` of the currently-armed `PreToolUse` (the precise
    /// transcript correlation anchor), if the hook carried one. Persists while
    /// the run is armed/waiting so corroboration checks *this exact* tool, not
    /// merely the transcript's last-dispatched one.
    armed_tool_use_id: Option<String>,
    /// The `transcript_path` pinned at arm time, so the serve layer can read the
    /// transcript JSONL and corroborate the inferred `waiting` before it fires.
    armed_transcript_path: Option<String>,
}

impl ClaudeInferMachine {
    /// A new machine for a session, starting `idle`, using [`DEFAULT_DEBOUNCE_MS`].
    pub fn new(session_id: impl Into<String>) -> Self {
        Self::with_debounce_ms(session_id, DEFAULT_DEBOUNCE_MS)
    }

    /// A new machine with an explicit debounce window (for tuning / tests).
    pub fn with_debounce_ms(session_id: impl Into<String>, debounce_ms: u64) -> Self {
        ClaudeInferMachine {
            core: Core::new(session_id),
            debounce_ms,
            armed_since: None,
            inferred_waiting: false,
            armed_tool_use_id: None,
            armed_transcript_path: None,
        }
    }

    /// The session's durable id (Claude `session_id`).
    pub fn session_id(&self) -> &str {
        self.core.native_id()
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
    /// The session's last-known working directory.
    pub fn cwd(&self) -> &str {
        self.core.cwd()
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
    /// The `transcript_path` pinned when the current `PreToolUse` armed, if any —
    /// the file the serve layer reads to corroborate before firing.
    pub fn armed_transcript_path(&self) -> Option<&str> {
        self.armed_transcript_path.as_deref()
    }
    /// Whether an armed `PreToolUse` has now been debouncing for at least the
    /// window at `now_ms` — i.e. a [`Self::tick`] at `now_ms` would fire the
    /// inferred `waiting`. Used by the serve layer to corroborate *before* firing.
    pub fn debounce_elapsed(&self, now_ms: u64) -> bool {
        self.armed_since
            .is_some_and(|a| now_ms.saturating_sub(a) >= self.debounce_ms)
    }

    /// Apply a parsed lifecycle hook event at monotonic millisecond time `now_ms`.
    pub fn apply(&mut self, ev: &ClaudeHookEvent, now_ms: u64) -> Transition {
        if ev.session_id != self.core.native_id() {
            return self.core.no_op(false);
        }
        self.core.note_cwd(ev.cwd.as_deref());
        self.core.note_tool(ev.tool_name.as_deref());

        match &ev.kind {
            ClaudeHookKind::PreToolUse => {
                let resolved = self.clear_inference();
                let was_working =
                    self.core.state() == State::Working && self.core.urgency().is_none();
                self.core.enter_working();
                self.armed_since = Some(now_ms);
                // Pin the precise correlation anchor + transcript for this arming.
                self.armed_tool_use_id = ev.tool_use_id.clone();
                self.armed_transcript_path = ev.transcript_path.clone();
                self.core
                    .transition(!was_working || resolved, resolved, true)
            }
            ClaudeHookKind::UserPromptSubmit => {
                let resolved = self.clear_inference();
                let was_working =
                    self.core.state() == State::Working && self.core.urgency().is_none();
                self.core.enter_working();
                self.core
                    .transition(!was_working || resolved, resolved, true)
            }
            ClaudeHookKind::Stop => {
                let resolved = self.clear_inference();
                if ev.stop_hook_active {
                    // A continuation Stop is not a real turn boundary: the agent is
                    // looping on, so it is activity — cancel any inference, stay
                    // working (never a completion claim).
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
                self.armed_since = None;
                self.inferred_waiting = false;
                let changed = self.core.state() != State::Dead;
                self.core.set_dead(); // confirmed exit is authoritative (High)
                self.core.transition(changed, false, false)
            }
            ClaudeHookKind::SessionStart => {
                if self.core.state() == State::Dead {
                    self.core.revive_idle();
                    self.armed_since = None;
                    self.inferred_waiting = false;
                    self.core.no_op(false).into_changed()
                } else {
                    self.core.no_op(false)
                }
            }
            ClaudeHookKind::PostToolUse => {
                let resolved = self.clear_inference();
                if resolved {
                    self.core.enter_working();
                    self.core.transition(true, true, true)
                } else {
                    self.armed_since = None;
                    self.core.no_op(true)
                }
            }
            ClaudeHookKind::SubagentStop | ClaudeHookKind::PreCompact => self.core.no_op(true),
            ClaudeHookKind::Other(_) => self.core.no_op(false),
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
            _ => self.core.no_op(false),
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
                    self.core.enter_working();
                    self.core.transition(true, true, false)
                } else {
                    self.armed_since = None;
                    self.core.no_op(false)
                }
            }
            Corroboration::Stuck | Corroboration::Unknown => self.core.no_op(false),
        }
    }

    /// Build the current [`AgentRun`] snapshot.
    pub fn to_run(&self, run_id: impl Into<String>, updated_at: impl Into<String>) -> AgentRun {
        self.core.to_run(
            AgentKind::ClaudeCode,
            run_id.into(),
            updated_at.into(),
            self.last_message(),
        )
    }

    fn fire_inference(&mut self) -> Transition {
        self.armed_since = None;
        self.inferred_waiting = true;
        // Inferred (never High) waiting — S16 is a heuristic by construction.
        self.core.set_waiting_approval(Confidence::Inferred);
        self.core.transition(true, false, false)
    }

    fn clear_inference(&mut self) -> bool {
        self.armed_since = None;
        self.armed_tool_use_id = None;
        self.armed_transcript_path = None;
        let was_waiting = self.inferred_waiting;
        self.inferred_waiting = false;
        was_waiting
    }

    fn last_message(&self) -> Option<String> {
        match self.core.state() {
            State::Waiting => Some(match self.core.last_tool() {
                Some(t) => format!("Possibly waiting on {t} (inferred)"),
                None => "Possibly waiting (inferred)".to_string(),
            }),
            State::Working => self.core.last_tool().map(|t| format!("Running {t}...")),
            State::Done => Some("Task complete.".to_string()),
            State::Dead => Some("Session closed.".to_string()),
            _ => None,
        }
    }
}

impl crate::machine::RunMachine for ClaudeInferMachine {
    fn run_state(&self) -> State {
        self.core.state()
    }
    fn build_run(&self, run_id: String, updated_at: String) -> AgentRun {
        self.to_run(run_id, updated_at)
    }
}

/// The S16 **adapter**: maps a multiplexed stream of native-UI Claude hook events
/// plus clock ticks to [`ReporterCommand`]s, owning one [`ClaudeInferMachine`] per
/// session id.
#[derive(Debug)]
pub struct ClaudeInferAdapter {
    core: AdapterCore<ClaudeInferMachine>,
    debounce_ms: u64,
}

impl Default for ClaudeInferAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeInferAdapter {
    /// A fresh adapter using [`DEFAULT_DEBOUNCE_MS`].
    pub fn new() -> Self {
        Self::with_debounce_ms(DEFAULT_DEBOUNCE_MS)
    }

    /// A fresh adapter with an explicit debounce window.
    pub fn with_debounce_ms(debounce_ms: u64) -> Self {
        ClaudeInferAdapter {
            core: AdapterCore::new("claude"),
            debounce_ms,
        }
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
    pub fn machine_of(&self, session_id: &str) -> Option<&ClaudeInferMachine> {
        self.core.machine_of(session_id)
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
        let debounce_ms = self.debounce_ms;
        self.core.apply_and_commands(
            &ev.session_id,
            || ClaudeInferMachine::with_debounce_ms(ev.session_id.clone(), debounce_ms),
            |m| m.apply(ev, now_ms),
        )
    }

    /// Advance every tracked session's debounce clock to `now_ms`, returning any
    /// `UpsertRun`/`Liveness` commands.
    pub fn tick(&mut self, now_ms: u64) -> Vec<ReporterCommand> {
        let mut cmds = Vec::new();
        for session_id in self.core.ids() {
            cmds.extend(self.core.with_existing(&session_id, |m| m.tick(now_ms)));
        }
        cmds
    }

    /// Sessions whose debounce would fire at `now_ms` (armed and elapsed), paired
    /// with the `transcript_path` pinned when they armed. The serve layer reads
    /// each transcript and corroborates the inferred `waiting` **before** the
    /// [`Self::tick`] fires it — the flagship native-UI corroboration path.
    pub fn pending_fires(&self, now_ms: u64) -> Vec<(String, Option<String>)> {
        self.core
            .iter()
            .filter(|(_, m)| m.debounce_elapsed(now_ms))
            .map(|(id, m)| (id.clone(), m.armed_transcript_path().map(String::from)))
            .collect()
    }

    /// Apply a transcript corroboration verdict to one session (by id), returning any
    /// resulting commands. A `Resolved` verdict that clears a raised waiting yields an
    /// `UpsertRun`.
    pub fn corroborate(
        &mut self,
        session_id: &str,
        verdict: Corroboration,
    ) -> Vec<ReporterCommand> {
        self.core
            .with_existing(session_id, |m| m.corroborate(verdict))
    }

    /// Corroborate a session against a raw transcript `blob`, automatically using
    /// the **precise** `tool_use_id` correlation ([`corroborate_jsonl_for`]) when
    /// the armed `PreToolUse` carried one, and falling back to the last-dispatched
    /// heuristic ([`corroborate_jsonl`]) otherwise. The caller just hands over the
    /// transcript; the right correlation strategy is chosen here.
    pub fn corroborate_blob(&mut self, session_id: &str, blob: &str) -> Vec<ReporterCommand> {
        let verdict = match self.core.machine_of(session_id) {
            Some(m) => match m.armed_tool_use_id() {
                Some(id) => corroborate_jsonl_for(blob, id),
                None => corroborate_jsonl(blob),
            },
            None => return Vec::new(),
        };
        self.corroborate(session_id, verdict)
    }

    /// Forget a session entirely.
    pub fn forget(&mut self, session_id: &str) -> bool {
        self.core.forget(session_id)
    }
}

#[cfg(test)]
mod tests {
    include!("claude_infer_tests.rs");
}
