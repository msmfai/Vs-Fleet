//! The shared agent-detection lifecycle core (2026-06 audit T2.2).
//!
//! The four agent adapters — [`crate::claude`] (S15 hooks), [`crate::codex`]
//! (S12), [`crate::claude_shim`] (S17 shim-terminal) and [`crate::claude_infer`]
//! (S16 inferred waiting) — were ~80% copy-paste: an identical `Transition`
//! struct, the same `enter_working`/`no_op`/`into_changed` primitives, a byte-for-
//! byte identical `AgentRun` builder, and identical adapter boilerplate
//! (`ensure_session` + `commands_for` + run-id minting `"{agent}:{id}:run-{n}"`).
//!
//! This module holds that shared skeleton exactly once:
//!
//! - [`Transition`] — the single transition-decision type, re-exported under the
//!   per-agent alias each adapter already published.
//! - [`Core`] — the shared mutable lifecycle state (durable id, cwd, state,
//!   urgency, confidence, last tool, pending-approval) plus the common transition
//!   primitives and the one `AgentRun` builder. Each agent's state machine embeds
//!   one `Core` and keeps only its agent-specific extras (Claude's last-assistant
//!   message preview, the shim's launch context, the infer machine's debounce
//!   state). The **per-agent `apply` sequencing** — which hook drives which edge —
//!   stays in each thin machine, because that is the one genuinely different part.
//! - [`AdapterCore`] — the generic per-session bookkeeping every adapter shares:
//!   the `session_id → (machine, run_id)` map, run-id minting, and the
//!   transition→[`ReporterCommand`] mapping.

use std::collections::HashMap;

use fleet_protocol::{AgentKind, AgentRun, Confidence, Extra, State, Urgency, SCHEMA_VERSION};

use crate::reporter::ReporterCommand;

/// A single state transition an agent machine decided, returned by its `apply`.
///
/// One shared shape across all four adapters. `changed` is `false` for a no-op
/// (the event was understood but did not move the run — telemetry, a duplicate, a
/// foreign-id guard). `resolved_approval` marks a transition that *cleared* a
/// pending waiting (a Codex/shim approval auto-resolve, or an S16 inference veto);
/// it is always `false` for the S15 machine, which never models waiting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// The run's state after the event.
    pub state: State,
    /// The run's urgency after the event (`None` ⇒ no urgency). Always `None` for
    /// the S15 machine, which never enters `waiting`.
    pub urgency: Option<Urgency>,
    /// The run's confidence after the event.
    pub confidence: Confidence,
    /// Whether the run-relevant fields actually changed (vs. a no-op).
    pub changed: bool,
    /// Whether this transition cleared a pending waiting (approval auto-resolve /
    /// inference veto).
    pub resolved_approval: bool,
    /// Whether this event is a pure liveness signal (refresh the timeout window).
    pub liveness: bool,
}

impl Transition {
    /// Mark a no-op transition as having changed (used for the resume/revive edge).
    pub(crate) fn into_changed(mut self) -> Self {
        self.changed = true;
        self
    }
}

/// The shared mutable lifecycle state + transition primitives every agent machine
/// carries. Embedded (not inherited) by each `*StateMachine`, which forwards its
/// public accessors here and keeps its own agent-specific fields alongside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Core {
    /// The durable identity anchor (Claude `session_id` / Codex `thread.id`).
    native_id: String,
    cwd: String,
    state: State,
    urgency: Option<Urgency>,
    confidence: Confidence,
    last_tool: Option<String>,
    pending_approval: bool,
}

impl Core {
    /// A new core for a run, starting `idle` (observed alive but not yet seen to do
    /// work) with `inferred` confidence (idle is not an authoritative signal).
    pub fn new(native_id: impl Into<String>) -> Self {
        Core {
            native_id: native_id.into(),
            cwd: "/".to_string(),
            state: State::Idle,
            urgency: None,
            confidence: Confidence::Inferred,
            last_tool: None,
            pending_approval: false,
        }
    }

    // ── accessors (each machine forwards its public getters here) ─────────────
    /// The durable identity anchor.
    pub fn native_id(&self) -> &str {
        &self.native_id
    }
    /// The run's last-known working directory.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }
    /// The run's current state.
    pub fn state(&self) -> State {
        self.state
    }
    /// Test-only: force an arbitrary state, to exercise un-modelled-state handling
    /// (the machines never emit e.g. `Error` via hooks, so it can't be reached by
    /// applying real events).
    #[cfg(test)]
    pub(crate) fn set_state_for_test(&mut self, s: State) {
        self.state = s;
    }
    /// The run's current urgency.
    pub fn urgency(&self) -> Option<Urgency> {
        self.urgency
    }
    /// The run's current confidence.
    pub fn confidence(&self) -> Confidence {
        self.confidence
    }
    /// The last tool name observed, if any.
    pub fn last_tool(&self) -> Option<&str> {
        self.last_tool.as_deref()
    }
    /// Whether an approval is outstanding.
    pub fn pending_approval(&self) -> bool {
        self.pending_approval
    }

    // ── event-field absorption ────────────────────────────────────────────────
    /// Absorb the working directory an event carries (a non-empty `cwd`).
    pub fn note_cwd(&mut self, cwd: Option<&str>) {
        if let Some(c) = cwd {
            if !c.is_empty() {
                self.cwd = c.to_string();
            }
        }
    }
    /// Absorb the tool name an event carries.
    pub fn note_tool(&mut self, tool: Option<&str>) {
        if let Some(t) = tool {
            self.last_tool = Some(t.to_string());
        }
    }

    // ── state mutators (pure; the caller builds the Transition) ───────────────
    /// Enter `working` — inferred from activity, clears any pending approval.
    pub fn enter_working(&mut self) {
        self.state = State::Working;
        self.urgency = None;
        self.confidence = Confidence::Inferred;
        self.pending_approval = false;
    }
    /// Enter `waiting`+`approval` at the given confidence and mark the approval
    /// pending. The confidence is the *only* authoritative-vs-inferred boundary
    /// (Codex `PermissionRequest` ⇒ `High`; native-UI inference ⇒ `Inferred`).
    pub fn set_waiting_approval(&mut self, confidence: Confidence) {
        self.state = State::Waiting;
        self.urgency = Some(Urgency::Approval);
        self.confidence = confidence;
        self.pending_approval = true;
    }
    /// Enter `done` — a real turn boundary (`inferred`; hooks cannot prove
    /// task-vs-turn completion).
    pub fn set_done(&mut self) {
        self.state = State::Done;
        self.urgency = None;
        self.confidence = Confidence::Inferred;
        self.pending_approval = false;
    }
    /// Enter `dead` — a confirmed exit is authoritative (`high`).
    pub fn set_dead(&mut self) {
        self.state = State::Dead;
        self.urgency = None;
        self.confidence = Confidence::High;
        self.pending_approval = false;
    }
    /// Revive a closed run back to `idle` (a `SessionStart` on a dead session).
    pub fn revive_idle(&mut self) {
        self.state = State::Idle;
        self.urgency = None;
        self.confidence = Confidence::Inferred;
        self.pending_approval = false;
    }

    // ── Transition builders ───────────────────────────────────────────────────
    /// A no-op transition at the current state (optionally a liveness ping).
    pub fn no_op(&self, liveness: bool) -> Transition {
        Transition {
            state: self.state,
            urgency: self.urgency,
            confidence: self.confidence,
            changed: false,
            resolved_approval: false,
            liveness,
        }
    }
    /// A transition at the current (already-mutated) state with explicit flags.
    pub fn transition(&self, changed: bool, resolved_approval: bool, liveness: bool) -> Transition {
        Transition {
            state: self.state,
            urgency: self.urgency,
            confidence: self.confidence,
            changed,
            resolved_approval,
            liveness,
        }
    }

    /// Build the current [`AgentRun`] snapshot. `native_id` is the durable anchor,
    /// so the reporter framework keys identity on it; `waiting_since` is stamped
    /// iff the run is currently `waiting`.
    pub fn to_run(
        &self,
        agent_kind: AgentKind,
        run_id: String,
        updated_at: String,
        last_message: Option<String>,
    ) -> AgentRun {
        let waiting_since = if self.state == State::Waiting {
            Some(updated_at.clone())
        } else {
            None
        };
        AgentRun {
            schema_version: SCHEMA_VERSION,
            run_id,
            agent_kind,
            native_id: self.native_id.clone(),
            cwd: self.cwd.clone(),
            state: self.state,
            urgency: self.urgency,
            last_message,
            waiting_since,
            confidence: self.confidence,
            diff_summary: None,
            updated_at,
            extra: Extra::new(),
        }
    }
}

/// The per-session bookkeeping every adapter needs from one of its machines.
pub trait RunMachine {
    /// The machine's current run state (for the adapter's `state_of` accessor).
    fn run_state(&self) -> State;
    /// Build the current [`AgentRun`] snapshot (for an `UpsertRun` command).
    fn build_run(&self, run_id: String, updated_at: String) -> AgentRun;
}

/// One tracked run: its state machine plus the Fleet run-id minted for it.
#[derive(Debug)]
pub struct Session<M> {
    /// The per-session state machine.
    pub machine: M,
    /// The stable Fleet run-id minted on first sighting.
    pub run_id: String,
}

/// The generic per-adapter bookkeeping: the `native_id → (machine, run_id)` map,
/// run-id minting (`"{prefix}:{id}:run-{n}"`), and the transition→command mapping.
/// Every adapter embeds one and forwards its public surface here.
#[derive(Debug)]
pub struct AdapterCore<M> {
    sessions: HashMap<String, Session<M>>,
    run_counter: u64,
    prefix: &'static str,
}

impl<M> AdapterCore<M> {
    /// A fresh adapter core minting run-ids under the given agent prefix
    /// (`"claude"` / `"codex"`).
    pub fn new(prefix: &'static str) -> Self {
        AdapterCore {
            sessions: HashMap::new(),
            run_counter: 0,
            prefix,
        }
    }

    /// Number of distinct runs currently tracked.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// The Fleet run-id minted for an id, if tracked.
    pub fn run_id_of(&self, id: &str) -> Option<&str> {
        self.sessions.get(id).map(|s| s.run_id.as_str())
    }

    /// Borrow a tracked machine.
    pub fn machine_of(&self, id: &str) -> Option<&M> {
        self.sessions.get(id).map(|s| &s.machine)
    }

    /// Forget a run entirely. A later event for the same id starts a fresh run.
    pub fn forget(&mut self, id: &str) -> bool {
        self.sessions.remove(id).is_some()
    }

    /// The tracked ids (snapshot, for iterating under `&mut self`).
    pub fn ids(&self) -> Vec<String> {
        self.sessions.keys().cloned().collect()
    }

    /// Iterate the tracked `(id, machine)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &M)> {
        self.sessions.iter().map(|(k, s)| (k, &s.machine))
    }

    /// Mint a machine + run-id for `id` on first sighting (idempotent).
    fn ensure(&mut self, id: &str, make: impl FnOnce() -> M) {
        if !self.sessions.contains_key(id) {
            self.run_counter += 1;
            let run_id = format!("{}:{id}:run-{}", self.prefix, self.run_counter);
            self.sessions.insert(
                id.to_string(),
                Session {
                    machine: make(),
                    run_id,
                },
            );
        }
    }
}

impl<M: RunMachine> AdapterCore<M> {
    /// The current run state for an id, if tracked.
    pub fn state_of(&self, id: &str) -> Option<State> {
        self.sessions.get(id).map(|s| s.machine.run_state())
    }

    /// Ensure a machine for `id`, apply `apply` to it, and map the resulting
    /// transition to commands. The canonical `ingest`: first sighting mints the
    /// machine via `make`.
    pub fn apply_and_commands(
        &mut self,
        id: &str,
        make: impl FnOnce() -> M,
        apply: impl FnOnce(&mut M) -> Transition,
    ) -> (Vec<ReporterCommand>, Transition) {
        self.ensure(id, make);
        let (transition, run_id) = {
            let session = self.sessions.get_mut(id).expect("just inserted");
            (apply(&mut session.machine), session.run_id.clone())
        };
        let cmds = self.commands_for(&transition, id, run_id);
        (cmds, transition)
    }

    /// Apply `apply` to an **already-tracked** machine (no minting), mapping the
    /// transition to commands. Unknown id ⇒ no machine, no commands.
    pub fn with_existing(
        &mut self,
        id: &str,
        apply: impl FnOnce(&mut M) -> Transition,
    ) -> Vec<ReporterCommand> {
        let Some(session) = self.sessions.get_mut(id) else {
            return Vec::new();
        };
        let transition = apply(&mut session.machine);
        let run_id = session.run_id.clone();
        self.commands_for(&transition, id, run_id)
    }

    /// Map a decided transition to reporter commands: a state change → an
    /// `UpsertRun` (freshly stamped); a pure-liveness no-op → a `Liveness` refresh.
    fn commands_for(
        &self,
        transition: &Transition,
        id: &str,
        run_id: String,
    ) -> Vec<ReporterCommand> {
        let mut cmds = Vec::new();
        if transition.changed {
            let machine = &self.sessions.get(id).expect("present").machine;
            let run = machine.build_run(run_id, fleet_protocol::now_iso8601());
            cmds.push(ReporterCommand::UpsertRun(run));
        } else if transition.liveness {
            cmds.push(ReporterCommand::Liveness { run_id });
        }
        cmds
    }
}
