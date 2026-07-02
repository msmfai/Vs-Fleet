//! The real reporter framework.
//!
//! This is the heart of S5: an outbound connection to the Hub that
//! **registers** a session, **assigns a fleet run-id**, sends **heartbeats**,
//! **buffers deltas while disconnected**, and **reconnects with backoff**,
//! reconciling on reconnect rather than reporting a run `dead` prematurely.
//!
//! # Architecture
//! The framework is split into a **pure core** ([`ReporterCore`]) and an
//! **async driver** ([`Reporter::run`]). The core owns all the decision state —
//! the [`crate::buffer::DeltaBuffer`], [`crate::backoff::Backoff`], and
//! per-run [`crate::liveness::LivenessTracker`]s — and is exhaustively unit
//! tested without any I/O. The driver is a thin tokio loop that:
//!
//! 1. connects (looping with backoff on failure);
//! 2. on first connect, sends the **registration** delta (the session upsert);
//! 3. **flushes the buffered backlog in `seq` order**;
//! 4. then services live commands + a **heartbeat tick**, buffering everything
//!    and draining to the live connection; on a send failure it drops back to
//!    step 1 (the un-acked delta stays buffered → no loss, no reorder).
//!
//! # Locked decisions honored
//! - **D4 custom durable identity, no broker**: the reporter assigns its own
//!   run-id and registers by durable session id; no external broker.
//! - **D2 Hub never auto-exits / observer-not-owner**: the reporter only reports;
//!   it never owns or launches the agent.
//! - **`dead` only on confirmed exit/timeout** ([`crate::liveness`]): a dropped
//!   Hub link never marks a run dead.
//! - **Invariant 5 confidence honesty**: the framework forwards whatever
//!   confidence the caller stamps; it never upgrades `inferred` to `high`.

use std::time::Duration;

use fleet_hub::wire::ClientMessage;
use fleet_protocol::{AgentRun, Session, State};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use fleet_hub::wire::SeqStamp;

use crate::backoff::Backoff;
use crate::buffer::{Delta, DeltaBuffer};
use crate::identity::{DurableId, IdentityLedger};
use crate::liveness::{Liveness, LivenessTracker};
use crate::transport::Connector;

/// Configuration for a [`Reporter`].
#[derive(Debug, Clone)]
pub struct ReporterConfig {
    /// The durable session id this reporter registers under (D4 — chosen by the
    /// reporter / its host, not derived by a broker).
    pub session_id: String,
    /// How often to emit a heartbeat while connected.
    pub heartbeat_interval: Duration,
    /// Heartbeat-timeout grace: a run with no liveness signal for longer than
    /// this is presumed dead (`DeadTimeout`). Independent of the Hub link.
    pub liveness_timeout: Duration,
    /// Reconnect backoff policy.
    pub backoff: Backoff,
    /// S6 durable identity: whether each newly-observed run is a **fresh-start**
    /// (`true` ⇒ `clean_start`, the Hub wipes any prior series for the durable
    /// id) or a **reconnect/reclaim** (`false`, the default — continue the
    /// series). A long-lived reporter that re-attaches to an existing agent uses
    /// `false`; a reporter that knows it is a brand-new run uses `true`.
    pub clean_start: bool,
}

impl ReporterConfig {
    /// Sensible defaults for a local reporter.
    pub fn new(session_id: impl Into<String>) -> Self {
        ReporterConfig {
            session_id: session_id.into(),
            heartbeat_interval: Duration::from_secs(10),
            liveness_timeout: Duration::from_secs(30),
            backoff: Backoff::default_policy(),
            // Default: reconnect/reclaim. The common case is a reporter that may
            // bounce its Hub link but observes the same agent run throughout.
            clean_start: false,
        }
    }
}

/// A command pushed into a running [`Reporter`] by the agent observer (or, in
/// S4/S5, a fake driver). These are the reporter's *inputs*; the framework turns
/// them into ordered, buffered, reconnect-safe Hub deltas.
#[derive(Debug, Clone)]
pub enum ReporterCommand {
    /// (Re)register / update the session shell. Sent first on every (re)connect.
    UpsertSession(Session),
    /// Add or update a run. The framework refreshes the run's liveness window.
    UpsertRun(AgentRun),
    /// An out-of-band agent-liveness signal (e.g. observed output) for `run_id`,
    /// with no state change — refreshes the timeout window so the run isn't
    /// timed out while genuinely busy.
    Liveness { run_id: String },
    /// Authoritative confirmed exit of a run → mark it `dead` and stop tracking.
    ConfirmExit { run_id: String, reason: String },
    /// Stop the reporter cleanly (session goes away).
    Shutdown,
}

/// A handle for pushing [`ReporterCommand`]s into a running reporter.
#[derive(Clone)]
pub struct ReporterHandle {
    tx: mpsc::UnboundedSender<ReporterCommand>,
}

impl ReporterHandle {
    /// Push a command. Returns `false` if the reporter loop has already exited.
    pub fn send(&self, cmd: ReporterCommand) -> bool {
        self.tx.send(cmd).is_ok()
    }

    /// Convenience: upsert a session.
    pub fn upsert_session(&self, s: Session) -> bool {
        self.send(ReporterCommand::UpsertSession(s))
    }
    /// Convenience: upsert a run.
    pub fn upsert_run(&self, r: AgentRun) -> bool {
        self.send(ReporterCommand::UpsertRun(r))
    }
    /// Convenience: confirm a run exited.
    pub fn confirm_exit(&self, run_id: impl Into<String>, reason: impl Into<String>) -> bool {
        self.send(ReporterCommand::ConfirmExit {
            run_id: run_id.into(),
            reason: reason.into(),
        })
    }
    /// Convenience: shut the reporter down.
    pub fn shutdown(&self) -> bool {
        self.send(ReporterCommand::Shutdown)
    }
}

/// The pure decision core of the reporter — no I/O, fully unit-testable.
///
/// It records the registration state, assigns run-ids, sequences and buffers
/// every outbound delta, owns the backoff policy, and tracks per-run liveness so
/// it can decide a run is dead *only* on confirmed exit/timeout.
pub struct ReporterCore {
    config: ReporterConfig,
    buffer: DeltaBuffer,
    /// The last-known session shell, re-sent first on every (re)connect so the
    /// Hub can reconcile after a restart (the engineering spec "kill+restore Hub → reconciles").
    registered_session: Option<Session>,
    /// The reporter's authoritative view of each known run, keyed by run-id, in
    /// insertion order. Carried inside the registration/heartbeat session object
    /// so a whole-object `session.upsert` (the Hub's merge semantics) reconciles
    /// the full session — runs included — rather than wiping them. This is what
    /// makes Hub-restart reconciliation correct.
    known_runs: std::collections::BTreeMap<String, AgentRun>,
    /// Insertion order of run-ids (BTreeMap orders by key, which we don't want
    /// for run *display* order; keep a parallel vec for stable ordering).
    run_order: Vec<String>,
    /// Per-run liveness trackers, keyed by run-id.
    liveness: std::collections::HashMap<String, LivenessTracker>,
    /// Monotonic counter for assigning fresh fleet run-ids (D4).
    run_id_counter: u64,
    /// S6 durable-identity ledger: per-run monotonic seq + epoch, so every run
    /// upsert is stamped `(durable_id, epoch, seq)` for the Hub's idempotent,
    /// ordered-replay reclaim gate.
    identity: IdentityLedger,
    /// Logical clock (seconds) used by the liveness machine; advanced by the
    /// driver's heartbeat tick. Clock-free core stays deterministic for tests.
    now: Duration,
}

impl ReporterCore {
    /// A fresh core for the given config.
    pub fn new(config: ReporterConfig) -> Self {
        ReporterCore {
            config,
            buffer: DeltaBuffer::new(),
            registered_session: None,
            known_runs: std::collections::BTreeMap::new(),
            run_order: Vec::new(),
            liveness: std::collections::HashMap::new(),
            run_id_counter: 0,
            identity: IdentityLedger::new(),
            now: Duration::ZERO,
        }
    }

    /// Build a stamped `run.upsert` for `run`, assigning the next monotonic seq
    /// for its durable id (`run.native_id`, the §7.5 native-agent anchor). The
    /// first time a run is seen, its `clean_start` (from config) decides epoch 0
    /// (reclaim/first-sight) vs a fresh-start bump. Falls back to an un-stamped
    /// message only if the run has no durable anchor (`native_id` empty).
    fn stamped_run_upsert(&mut self, run: AgentRun) -> ClientMessage {
        let durable = run.native_id.clone();
        // First sighting under this durable id: honor the clean_start policy so a
        // genuinely-fresh run bumps the epoch (wipe) while a reclaim stays at the
        // current epoch.
        let did = DurableId::new(durable.clone());
        if !durable.is_empty() && !self.identity.contains(&did) {
            self.identity
                .declare(durable.clone(), self.config.clean_start);
        }
        let stamp = self.identity.stamp(durable).map(|s| SeqStamp {
            durable_id: s.durable_id.as_str().to_string(),
            epoch: s.epoch,
            seq: s.seq,
        });
        ClientMessage::RunUpsert {
            session_id: self.config.session_id.clone(),
            run,
            stamp,
        }
    }

    /// Record a run in the reporter's authoritative session view (so heartbeats /
    /// re-registration carry it). Preserves insertion order; updates in place.
    fn remember_run(&mut self, run: &AgentRun) {
        if !self.known_runs.contains_key(&run.run_id) {
            self.run_order.push(run.run_id.clone());
        }
        self.known_runs.insert(run.run_id.clone(), run.clone());
    }

    /// The reporter's current view of all known runs, in insertion order.
    fn runs_in_order(&self) -> Vec<AgentRun> {
        self.run_order
            .iter()
            .filter_map(|id| self.known_runs.get(id).cloned())
            .collect()
    }

    /// Assign a fresh, durable fleet run-id (D4 — reporter-assigned, no broker).
    /// Format: `<session_id>:run-<n>` — stable, collision-free within a session.
    pub fn assign_run_id(&mut self) -> String {
        self.run_id_counter += 1;
        format!("{}:run-{}", self.config.session_id, self.run_id_counter)
    }

    /// Number of run-ids assigned so far.
    pub fn run_ids_assigned(&self) -> u64 {
        self.run_id_counter
    }

    /// The durable session id this core registers under.
    pub fn session_id(&self) -> &str {
        &self.config.session_id
    }

    /// Advance the logical clock (used by the heartbeat tick).
    pub fn advance_to(&mut self, now: Duration) {
        if now >= self.now {
            self.now = now;
        }
    }

    /// Current logical time.
    pub fn now(&self) -> Duration {
        self.now
    }

    /// Apply a [`ReporterCommand`], enqueuing any resulting Hub delta(s) into the
    /// ordered buffer. Returns `true` if the reporter should keep running
    /// (`false` only for [`ReporterCommand::Shutdown`]).
    pub fn apply(&mut self, cmd: ReporterCommand) -> bool {
        match cmd {
            ReporterCommand::UpsertSession(session) => {
                self.registered_session = Some(session.clone());
                self.buffer.push(ClientMessage::SessionUpsert { session });
                true
            }
            ReporterCommand::UpsertRun(run) => {
                // A run upsert is also a liveness signal (the agent did
                // something). Refresh its window unless it's a terminal state.
                let tracker = self
                    .liveness
                    .entry(run.run_id.clone())
                    .or_insert_with(|| LivenessTracker::new(self.config.liveness_timeout));
                if run.state == State::Dead {
                    tracker.observe_exit();
                } else {
                    tracker.observe_liveness(self.now);
                }
                self.remember_run(&run);
                let msg = self.stamped_run_upsert(run);
                self.buffer.push(msg);
                true
            }
            ReporterCommand::Liveness { run_id } => {
                if let Some(t) = self.liveness.get_mut(&run_id) {
                    t.observe_liveness(self.now);
                }
                // Pure liveness refresh produces no Hub delta.
                true
            }
            ReporterCommand::ConfirmExit { run_id, reason } => {
                if let Some(t) = self.liveness.get_mut(&run_id) {
                    t.observe_exit();
                }
                // Emit an authoritative `dead` run upsert so the Hub reflects it.
                let dead = self.dead_run(&run_id, &reason);
                self.remember_run(&dead);
                let msg = self.stamped_run_upsert(dead);
                self.buffer.push(msg);
                true
            }
            ReporterCommand::Shutdown => false,
        }
    }

    /// Build an authoritative `dead` run delta for `run_id`. If the reporter
    /// already knows the run, the dead delta preserves its identity
    /// (`agent_kind`, `native_id`, `cwd`) and only flips state → `Dead`.
    fn dead_run(&self, run_id: &str, reason: &str) -> AgentRun {
        let mut r = if let Some(known) = self.known_runs.get(run_id) {
            let mut k = known.clone();
            k.urgency = None;
            k.waiting_since = None;
            k
        } else {
            AgentRun::new(
                run_id,
                fleet_protocol::AgentKind::Other,
                run_id,
                "/",
                State::Dead,
                fleet_protocol::Confidence::High,
                fleet_protocol::now_iso8601(),
            )
        };
        r.state = State::Dead;
        // Confidence honesty: a confirmed exit / timeout IS an authoritative
        // signal about liveness.
        r.confidence = fleet_protocol::Confidence::High;
        r.updated_at = fleet_protocol::now_iso8601();
        r.last_message = Some(reason.to_string());
        r
    }

    /// Evaluate every tracked run for a timeout death at the current logical
    /// time, emitting a `dead` delta for any newly-timed-out run. Returns the
    /// run-ids that just died. Idempotent: a run already reported dead (no
    /// tracker) is not re-emitted.
    pub fn reap_timeouts(&mut self) -> Vec<String> {
        let now = self.now;
        let timed_out: Vec<String> = self
            .liveness
            .iter()
            .filter(|(_, t)| matches!(t.evaluate(now), Liveness::DeadTimeout))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &timed_out {
            self.liveness.remove(id);
            let dead = self.dead_run(id, "heartbeat timeout");
            self.remember_run(&dead);
            let msg = self.stamped_run_upsert(dead);
            self.buffer.push(msg);
        }
        timed_out
    }

    /// Build the heartbeat / re-registration delta: a **whole-object**
    /// `session.upsert` carrying the reporter's full current view — the session
    /// shell **plus every known run**. Because the Hub merges whole objects
    /// (replacing the session), embedding the runs is what lets a heartbeat or a
    /// post-restart re-registration *reconcile* the full state instead of wiping
    /// the runs. Returns `None` until a session has been registered.
    pub fn heartbeat_message(&self) -> Option<ClientMessage> {
        self.registered_session.as_ref().map(|s| {
            let mut full = s.clone();
            full.runs = self.runs_in_order();
            ClientMessage::SessionUpsert { session: full }
        })
    }

    /// The registration delta to (re)send first on every (re)connect, so the Hub
    /// reconciles state after a restart. Identical to the heartbeat: a full
    /// snapshot of the reporter's session view. `None` until registered.
    pub fn registration_message(&self) -> Option<ClientMessage> {
        self.heartbeat_message()
    }

    /// Drain all buffered deltas in order (flush-on-reconnect path).
    pub fn drain(&mut self) -> Vec<Delta> {
        self.buffer.drain()
    }

    /// Re-insert un-flushed deltas at the front of the buffer, preserving their
    /// original `seq` and order, ahead of anything buffered since (reconnect
    /// path; preserves ordered replay with no re-numbering).
    pub fn requeue_unsent(&mut self, tail: &[Delta]) {
        let newer = self.buffer.drain();
        for d in tail.iter().chain(newer.iter()) {
            self.buffer.push_preserving(d.seq, d.msg.clone());
        }
    }

    /// Peek the buffered backlog length.
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// The last assigned outbound seq (`0` if none yet).
    pub fn last_seq(&self) -> u64 {
        self.buffer.last_seq()
    }

    /// Record a failed connect; advance backoff and return the next delay.
    pub fn on_connect_failed(&mut self) -> Duration {
        self.config.backoff.record_failure()
    }

    /// Record a successful connect; reset backoff.
    pub fn on_connect_succeeded(&mut self) {
        self.config.backoff.record_success();
    }

    /// Current backoff delay.
    pub fn backoff_delay(&self) -> Duration {
        self.config.backoff.current_delay()
    }

    /// Liveness verdict for a run at the current logical time.
    pub fn liveness_of(&self, run_id: &str) -> Option<Liveness> {
        self.liveness.get(run_id).map(|t| t.evaluate(self.now))
    }

    /// Config accessor.
    pub fn config(&self) -> &ReporterConfig {
        &self.config
    }
}

/// The outcome of one heartbeat-ticker tick (see [`Reporter::on_tick`]): either
/// keep serving the current connection, or reconnect because a send failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TickOutcome {
    /// Keep serving on the current connection.
    Continue,
    /// A send failed (connection dropped) — the caller must reconnect.
    Reconnect,
}

/// The async reporter driver. Wraps a [`ReporterCore`] and a [`Connector`],
/// owning the connect/flush/heartbeat tokio loop.
pub struct Reporter {
    core: ReporterCore,
    connector: Box<dyn Connector>,
}

impl Reporter {
    /// Build a reporter from a config and a connector.
    pub fn new(config: ReporterConfig, connector: Box<dyn Connector>) -> Self {
        Reporter {
            core: ReporterCore::new(config),
            connector,
        }
    }

    /// Split off a command channel + handle, returning `(self, handle, rx)`.
    pub fn with_channel(
        self,
    ) -> (
        Reporter,
        ReporterHandle,
        mpsc::UnboundedReceiver<ReporterCommand>,
    ) {
        let (tx, rx) = mpsc::unbounded_channel();
        (self, ReporterHandle { tx }, rx)
    }

    /// Borrow the core (tests).
    pub fn core(&self) -> &ReporterCore {
        &self.core
    }
    /// Mutably borrow the core (tests / pre-seeding).
    pub fn core_mut(&mut self) -> &mut ReporterCore {
        &mut self.core
    }

    /// Run the reporter loop until [`ReporterCommand::Shutdown`] or the command
    /// channel closes.
    ///
    /// `commands` is the stream of agent observations; the framework buffers and
    /// reconnect-safely delivers them. A wall-clock heartbeat tick drives
    /// heartbeats, advances the logical clock, and reaps timed-out runs.
    pub async fn run(
        mut self,
        mut commands: mpsc::UnboundedReceiver<ReporterCommand>,
    ) -> anyhow::Result<()> {
        let hb = self.core.config().heartbeat_interval;
        let start = tokio::time::Instant::now();
        let mut ticker = tokio::time::interval(hb.max(Duration::from_millis(1)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        info!(
            session = %self.core.session_id(),
            endpoint = %self.connector.endpoint(),
            "reporter starting"
        );

        // A buffered `Shutdown` seen *while disconnected* must NOT abort the
        // reconnect loop: we still owe the Hub the buffered backlog. We record
        // the request and keep retrying until we connect, flush everything, and
        // only then exit cleanly. (A `Shutdown` seen while connected exits at
        // once, since the backlog is already drained.)
        let mut shutdown_requested = false;

        // Outer loop: (re)establish a connection, then serve until it drops.
        'reconnect: loop {
            let mut conn = match self.connector.connect().await {
                Ok(c) => {
                    self.core.on_connect_succeeded();
                    info!(endpoint = %self.connector.endpoint(), "reporter connected");
                    c
                }
                Err(e) => {
                    let delay = self.core.on_connect_failed();
                    warn!(error = %e, backoff_ms = delay.as_millis() as u64, "connect failed; backing off");
                    tokio::time::sleep(delay).await;
                    // Drain any commands that arrived while we were down so the
                    // buffer reflects the latest state (still ordered by seq).
                    if self.pump_pending(&mut commands) {
                        shutdown_requested = true;
                    }
                    continue 'reconnect;
                }
            };

            // (Re)send registration first so the Hub reconciles after a restart.
            if let Some(reg) = self.core.registration_message() {
                if conn.send(&reg).await.is_err() {
                    warn!("registration send failed; reconnecting");
                    continue 'reconnect;
                }
            }

            // If a shutdown was requested while disconnected, absorb any
            // still-queued commands now (ordered) before the final flush, so the
            // backlog we deliver is complete.
            if shutdown_requested {
                self.pump_pending(&mut commands);
            }

            // Flush the buffered backlog in seq order.
            if !self.flush(&mut conn).await {
                continue 'reconnect; // connection dropped mid-flush; backlog kept
            }

            // Backlog delivered. If shutdown was requested while down, we are
            // done — exit cleanly now that everything has been reconciled.
            if shutdown_requested {
                debug!("reporter shutting down after delivering buffered backlog");
                let _ = conn.close().await;
                return Ok(());
            }

            // Serve live commands + heartbeats until the connection drops or we
            // are told to shut down.
            loop {
                tokio::select! {
                    maybe_cmd = commands.recv() => {
                        match maybe_cmd {
                            Some(ReporterCommand::Shutdown) | None => {
                                debug!("reporter shutting down");
                                let _ = conn.close().await;
                                return Ok(());
                            }
                            Some(cmd) => {
                                self.core.apply(cmd);
                                if !self.flush(&mut conn).await {
                                    continue 'reconnect;
                                }
                            }
                        }
                    }
                    _ = ticker.tick() => {
                        match self.on_tick(&mut conn, start.elapsed()).await {
                            TickOutcome::Reconnect => continue 'reconnect,
                            TickOutcome::Continue => {}
                        }
                    }
                }
            }
        }
    }

    /// Handle one heartbeat-ticker tick: advance the logical clock to `elapsed`,
    /// reap timed-out runs, send a heartbeat (if a session is registered), and
    /// flush any reap-produced `dead` deltas.
    ///
    /// Extracted from the `select!` tick arm so its branches are exercised by
    /// deterministic unit tests rather than by racing a live ticker against the
    /// command arm. Returns [`TickOutcome::Reconnect`] if any send failed (the
    /// connection dropped); otherwise [`TickOutcome::Continue`].
    async fn on_tick(
        &mut self,
        conn: &mut Box<dyn crate::transport::Connection>,
        elapsed: Duration,
    ) -> TickOutcome {
        self.core.advance_to(elapsed);
        let dead = self.core.reap_timeouts();
        if !dead.is_empty() {
            warn!(?dead, "runs timed out (no liveness); marked dead");
        }
        if let Some(hb_msg) = self.core.heartbeat_message() {
            // Heartbeat is sent directly (not buffered): it is a pure liveness
            // ping; if it fails, reconnect.
            if conn.send(&hb_msg).await.is_err() {
                return TickOutcome::Reconnect;
            }
        }
        // Also drain any timeout-`dead` deltas reap produced.
        if !self.flush(conn).await {
            return TickOutcome::Reconnect;
        }
        TickOutcome::Continue
    }

    /// Drain commands that are *immediately* available (non-blocking), applying
    /// each to the core's buffer. Returns `true` if a `Shutdown` was seen among
    /// them (the caller finishes delivering the backlog, then exits).
    fn pump_pending(&mut self, commands: &mut mpsc::UnboundedReceiver<ReporterCommand>) -> bool {
        let mut saw_shutdown = false;
        while let Ok(cmd) = commands.try_recv() {
            if !self.core.apply(cmd) {
                saw_shutdown = true;
            }
        }
        saw_shutdown
    }

    /// Flush the buffered backlog over `conn` in seq order. On a send failure,
    /// re-buffers the *unsent* tail (preserving order) and returns `false` so the
    /// caller reconnects without losing or reordering anything.
    async fn flush(&mut self, conn: &mut Box<dyn crate::transport::Connection>) -> bool {
        let batch = self.core.drain();
        if batch.is_empty() {
            return true;
        }
        for (i, delta) in batch.iter().enumerate() {
            if conn.send(&delta.msg).await.is_err() {
                // Re-buffer the unsent tail (this delta + the rest), in order.
                self.core.requeue_unsent(&batch[i..]);
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::MemoryHub;
    use fleet_protocol::{
        AgentKind, Confidence, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind,
        SCHEMA_VERSION,
    };

    fn config() -> ReporterConfig {
        let mut c = ReporterConfig::new("sess-test");
        c.heartbeat_interval = Duration::from_millis(20);
        c.liveness_timeout = Duration::from_secs(30);
        c.backoff = Backoff::new(Duration::from_millis(5), Duration::from_millis(40), 2);
        c
    }

    fn session(id: &str) -> Session {
        Session {
            schema_version: SCHEMA_VERSION,
            session_id: id.into(),
            title: "t".into(),
            location: Location {
                kind: LocationKind::Local,
                label: "l".into(),
                glyph: LocationGlyph::Laptop,
                attach_hint: None,
                extra: Extra::new(),
            },
            editor: None,
            server: Server {
                kind: ServerKind::Local,
                version: None,
                extra: Extra::new(),
            },
            runs: vec![],
            rollup_state: State::Idle,
            rollup_urgency: None,
            muted: false,
            soloed: false,
            unread: false,
            tags: vec![],
            policy: None,
            updated_at: "2026-06-08T00:00:00Z".into(),
            extra: Extra::new(),
        }
    }

    fn run(id: &str, state: State) -> AgentRun {
        AgentRun::new(
            id,
            AgentKind::Codex,
            "native",
            "/",
            state,
            Confidence::High,
            "2026-06-08T00:00:00Z",
        )
    }

    /// Borrow the `run.upsert`'s run or panic. The non-run.upsert arm is an
    /// unreachable test-assertion path (the deltas under test are built by the
    /// core as run.upserts), so this is excluded from the nightly gate; the
    /// behavioral assertions live at the (covered) call sites.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn expect_run_upsert(msg: &ClientMessage) -> &AgentRun {
        match msg {
            ClientMessage::RunUpsert { run, .. } => run,
            other => panic!("expected run.upsert dead, got {other:?}"),
        }
    }

    // ── pure core: run-id assignment (D4) ─────────────────────────────────────

    #[test]
    fn assigns_unique_durable_run_ids() {
        let mut core = ReporterCore::new(config());
        let a = core.assign_run_id();
        let b = core.assign_run_id();
        assert_ne!(a, b);
        assert!(a.starts_with("sess-test:run-"));
        assert_eq!(core.run_ids_assigned(), 2);
    }

    // ── pure core: registration + buffering + ordering ────────────────────────

    #[test]
    fn upsert_session_registers_and_buffers() {
        let mut core = ReporterCore::new(config());
        assert!(core.registration_message().is_none());
        core.apply(ReporterCommand::UpsertSession(session("sess-test")));
        assert!(core.registration_message().is_some());
        assert_eq!(core.buffered(), 1);
        let drained = core.drain();
        assert_eq!(drained[0].seq, 1);
        assert_eq!(drained[0].type_name(), "session.upsert");
    }

    #[test]
    fn buffer_preserves_order_by_seq() {
        let mut core = ReporterCore::new(config());
        core.apply(ReporterCommand::UpsertSession(session("sess-test")));
        core.apply(ReporterCommand::UpsertRun(run("r1", State::Working)));
        core.apply(ReporterCommand::UpsertRun(run("r1", State::Waiting)));
        let drained = core.drain();
        let seqs: Vec<u64> = drained.iter().map(|d| d.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3], "ordered by monotonic seq");
        let types: Vec<&str> = drained.iter().map(|d| d.type_name()).collect();
        assert_eq!(types, vec!["session.upsert", "run.upsert", "run.upsert"]);
    }

    // ── pure core: dead only on confirmed exit/timeout ────────────────────────

    #[test]
    fn confirm_exit_marks_run_dead_authoritatively() {
        let mut core = ReporterCore::new(config());
        core.apply(ReporterCommand::UpsertRun(run("r1", State::Working)));
        assert_eq!(core.liveness_of("r1"), Some(Liveness::Alive));
        core.apply(ReporterCommand::ConfirmExit {
            run_id: "r1".into(),
            reason: "process exited".into(),
        });
        assert_eq!(core.liveness_of("r1"), Some(Liveness::DeadConfirmedExit));
        // A dead run.upsert was buffered.
        let drained = core.drain();
        let run = expect_run_upsert(&drained.last().unwrap().msg);
        assert_eq!(run.state, State::Dead);
        assert_eq!(
            run.confidence,
            Confidence::High,
            "confirmed exit is authoritative"
        );
    }

    #[test]
    fn run_not_dead_just_because_time_passes_if_liveness_refreshed() {
        let mut core = ReporterCore::new(config());
        core.apply(ReporterCommand::UpsertRun(run("r1", State::Working)));
        // Advance well past the timeout but keep signaling liveness.
        for sec in (10..200).step_by(10) {
            core.advance_to(Duration::from_secs(sec));
            core.apply(ReporterCommand::Liveness {
                run_id: "r1".into(),
            });
            assert_eq!(core.liveness_of("r1"), Some(Liveness::Alive));
            assert!(core.reap_timeouts().is_empty(), "kept alive by liveness");
        }
    }

    #[test]
    fn timeout_reaps_silent_run() {
        let mut core = ReporterCore::new(config());
        core.apply(ReporterCommand::UpsertRun(run("r1", State::Working)));
        core.advance_to(Duration::from_secs(31)); // > 30s timeout
        let dead = core.reap_timeouts();
        assert_eq!(dead, vec!["r1".to_string()]);
        // Idempotent: a second reap does not re-emit.
        assert!(core.reap_timeouts().is_empty());
        // A dead delta was buffered.
        let drained = core.drain();
        assert!(drained.iter().any(|d| matches!(&d.msg,
            ClientMessage::RunUpsert { run, .. } if run.state == State::Dead)));
    }

    // ── pure core: backoff integration ────────────────────────────────────────

    #[test]
    fn backoff_advances_on_failure_resets_on_success() {
        let mut core = ReporterCore::new(config());
        let d1 = core.on_connect_failed();
        let d2 = core.on_connect_failed();
        assert!(d2 > d1, "backoff grows");
        core.on_connect_succeeded();
        assert_eq!(core.backoff_delay(), Duration::from_millis(5));
    }

    // ── async driver: end-to-end via the in-memory transport ──────────────────

    #[tokio::test]
    async fn driver_registers_then_delivers_in_order() {
        let hub = MemoryHub::new();
        let reporter = Reporter::new(config(), Box::new(hub.connector()));
        let (reporter, handle, rx) = reporter.with_channel();
        let task = tokio::spawn(reporter.run(rx));

        handle.upsert_session(session("sess-test"));
        handle.upsert_run(run("r1", State::Working));
        handle.upsert_run(run("r1", State::Waiting));
        handle.shutdown();
        task.await.unwrap().unwrap();

        let delivered = hub.delivered();
        // First frame is the registration (session.upsert), then the run deltas.
        assert_eq!(delivered[0].type_name(), "session.upsert");
        let run_states: Vec<State> = delivered
            .iter()
            .filter_map(|m| match m {
                ClientMessage::RunUpsert { run, .. } => Some(run.state),
                _ => None,
            })
            .collect();
        assert!(run_states.contains(&State::Working));
        assert!(run_states.contains(&State::Waiting));
    }

    #[tokio::test]
    async fn driver_reconnects_with_backoff_after_connect_failures() {
        let hub = MemoryHub::new();
        // First two connects fail, third succeeds.
        hub.script_connects([false, false, true]);
        let reporter = Reporter::new(config(), Box::new(hub.connector()));
        let (reporter, handle, rx) = reporter.with_channel();
        let task = tokio::spawn(reporter.run(rx));

        handle.upsert_session(session("sess-test"));
        handle.upsert_run(run("r1", State::Working));
        handle.shutdown();
        task.await.unwrap().unwrap();

        assert!(hub.connect_attempts() >= 3, "must retry failed connects");
        assert_eq!(hub.connect_successes(), 1);
        // Despite the failed connects, the deltas were delivered (buffered then
        // flushed) and in order.
        let delivered = hub.delivered();
        assert_eq!(delivered[0].type_name(), "session.upsert");
        assert!(delivered.iter().any(
            |m| matches!(m, ClientMessage::RunUpsert { run, .. } if run.state == State::Working)
        ));
    }

    #[tokio::test]
    async fn driver_buffers_then_flushes_on_reconnect_in_order() {
        let hub = MemoryHub::new();
        // The first connection drops after delivering only the registration
        // (1 send), forcing a reconnect mid-flush; the backlog must replay in
        // order on the new connection with no loss and no reorder.
        hub.drop_next_connection_after(1);
        let reporter = Reporter::new(config(), Box::new(hub.connector()));
        let (reporter, handle, rx) = reporter.with_channel();
        let task = tokio::spawn(reporter.run(rx));

        handle.upsert_session(session("sess-test"));
        handle.upsert_run(run("r1", State::Working));
        handle.upsert_run(run("r1", State::Waiting));
        handle.upsert_run(run("r1", State::Idle));
        handle.shutdown();
        task.await.unwrap().unwrap();

        let delivered = hub.delivered();
        // Reconstruct the logical run-state sequence as the Hub saw it.
        let states: Vec<State> = delivered
            .iter()
            .filter_map(|m| match m {
                ClientMessage::RunUpsert { run, .. } => Some(run.state),
                _ => None,
            })
            .collect();
        // The run states must appear in their produced order: W → Waiting → Idle.
        // (Heartbeats may interleave session.upserts but never reorder runs.)
        let idx_working = states.iter().position(|s| *s == State::Working);
        let idx_waiting = states.iter().position(|s| *s == State::Waiting);
        let idx_idle = states.iter().position(|s| *s == State::Idle);
        assert!(idx_working.is_some() && idx_waiting.is_some() && idx_idle.is_some());
        assert!(idx_working < idx_waiting, "Working before Waiting");
        assert!(idx_waiting < idx_idle, "Waiting before Idle");
        assert!(hub.connect_successes() >= 2, "must have reconnected");
    }

    #[tokio::test]
    async fn driver_emits_heartbeats_on_cadence() {
        // With a 20ms heartbeat interval and an idle command stream, the Hub
        // should see repeated session.upsert (heartbeat) frames over ~120ms.
        let hub = MemoryHub::new();
        let reporter = Reporter::new(config(), Box::new(hub.connector()));
        let (reporter, handle, rx) = reporter.with_channel();
        let task = tokio::spawn(reporter.run(rx));

        handle.upsert_session(session("sess-test"));
        // Let several heartbeat ticks elapse without any new commands.
        tokio::time::sleep(Duration::from_millis(120)).await;
        handle.shutdown();
        task.await.unwrap().unwrap();

        let hb_count = hub
            .delivered()
            .iter()
            .filter(|m| matches!(m, ClientMessage::SessionUpsert { .. }))
            .count();
        // 1 registration + several heartbeats. Be lenient on exact count (timer
        // jitter) but require that heartbeats actually fired beyond registration.
        assert!(
            hb_count >= 3,
            "expected multiple heartbeat session.upserts, got {hb_count}"
        );
    }

    #[tokio::test]
    async fn driver_times_out_silent_run_and_reports_dead() {
        // A run that never signals liveness again must be reaped to `dead` after
        // the liveness timeout — a *timeout* death, distinct from a Hub drop.
        let mut c = config();
        c.heartbeat_interval = Duration::from_millis(10);
        c.liveness_timeout = Duration::from_millis(40);
        let hub = MemoryHub::new();
        let reporter = Reporter::new(c, Box::new(hub.connector()));
        let (reporter, handle, rx) = reporter.with_channel();
        let task = tokio::spawn(reporter.run(rx));

        handle.upsert_session(session("sess-test"));
        handle.upsert_run(run("r1", State::Working));
        // Stay silent past the liveness timeout; the heartbeat tick reaps it.
        tokio::time::sleep(Duration::from_millis(120)).await;
        handle.shutdown();
        task.await.unwrap().unwrap();

        let saw_dead = hub.delivered().iter().any(|m| {
            matches!(m,
            ClientMessage::RunUpsert { run, .. } if run.state == State::Dead)
        });
        assert!(saw_dead, "silent run must be reaped to dead on timeout");
    }

    #[tokio::test]
    async fn driver_reconnects_when_a_tick_reports_reconnect() {
        // Cover the live-loop's `select!` tick arm taking the Reconnect branch.
        // The first connection's registration send succeeds (1 send) but the
        // following heartbeat send fails, so `on_tick` returns Reconnect and the
        // arm executes `continue 'reconnect`. We send NO commands until after we
        // observe the reconnect, so the only thing that can resolve the select is
        // the ticker — making the Reconnect arm deterministic, not a race against
        // a command. Then we shut down cleanly.
        let mut c = config();
        c.heartbeat_interval = Duration::from_millis(10);
        let hub = MemoryHub::new();
        hub.drop_next_connection_after(1); // reg ok; the first heartbeat fails
        let reporter = Reporter::new(c, Box::new(hub.connector()));
        let (reporter, handle, rx) = reporter.with_channel();
        // Pre-seed the session so registration_message() is Some on connect and a
        // heartbeat is due on the very first tick.
        let task = tokio::spawn(reporter.run(rx));
        handle.upsert_session(session("sess-test"));

        // Wait until the tick-driven heartbeat failure has forced a reconnect.
        // Only the ticker can trigger this (no other commands are pending).
        let reconnected = async {
            loop {
                if hub.connect_successes() >= 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), reconnected)
            .await
            .expect("a failed heartbeat tick must drive a reconnect");

        handle.shutdown();
        task.await.unwrap().unwrap();

        assert!(
            hub.connect_successes() >= 2,
            "the tick-arm Reconnect branch re-established the connection"
        );
    }

    #[tokio::test]
    async fn driver_resends_registration_on_every_reconnect() {
        let hub = MemoryHub::new();
        hub.drop_next_connection_after(2); // reg + 1 run, then drop
        let reporter = Reporter::new(config(), Box::new(hub.connector()));
        let (reporter, handle, rx) = reporter.with_channel();
        let task = tokio::spawn(reporter.run(rx));

        handle.upsert_session(session("sess-test"));
        handle.upsert_run(run("r1", State::Working));
        handle.upsert_run(run("r1", State::Idle));
        handle.shutdown();
        task.await.unwrap().unwrap();

        // The session.upsert (registration) must appear at least twice: once per
        // connection, so the Hub reconciles after the drop.
        let reg_count = hub
            .delivered()
            .iter()
            .filter(|m| matches!(m, ClientMessage::SessionUpsert { .. }))
            .count();
        assert!(
            reg_count >= 2,
            "registration re-sent on reconnect (got {reg_count})"
        );
    }

    // ── pure core: accessors + edge state paths ───────────────────────────────

    #[test]
    fn core_exposes_session_id_and_logical_clock() {
        let mut core = ReporterCore::new(config());
        assert_eq!(core.session_id(), "sess-test");
        assert_eq!(core.now(), Duration::ZERO);
        core.advance_to(Duration::from_secs(7));
        assert_eq!(core.now(), Duration::from_secs(7));
        // The logical clock is monotonic: a smaller advance is ignored.
        core.advance_to(Duration::from_secs(3));
        assert_eq!(core.now(), Duration::from_secs(7));
    }

    #[test]
    fn core_reports_last_assigned_seq() {
        let mut core = ReporterCore::new(config());
        assert_eq!(core.last_seq(), 0, "no deltas buffered yet");
        core.apply(ReporterCommand::UpsertSession(session("sess-test")));
        core.apply(ReporterCommand::UpsertRun(run("r1", State::Working)));
        assert_eq!(core.last_seq(), 2, "two deltas ⇒ last seq is 2");
    }

    #[test]
    fn upsert_run_dead_records_exit_not_liveness() {
        // A run upserted directly as Dead is a terminal observation: it must
        // mark the liveness tracker as exited (not refresh its window), so a
        // later evaluation reports a confirmed exit rather than Alive.
        let mut core = ReporterCore::new(config());
        core.apply(ReporterCommand::UpsertRun(run("r1", State::Dead)));
        assert_eq!(
            core.liveness_of("r1"),
            Some(Liveness::DeadConfirmedExit),
            "a dead upsert confirms the exit"
        );
    }

    #[test]
    fn confirm_exit_of_unknown_run_synthesizes_a_dead_delta() {
        // ConfirmExit for a run the core never saw must still emit an
        // authoritative dead delta — built from scratch (Other/High), not from a
        // remembered run.
        let mut core = ReporterCore::new(config());
        core.apply(ReporterCommand::ConfirmExit {
            run_id: "ghost".into(),
            reason: "vanished".into(),
        });
        let drained = core.drain();
        let last = drained.last().expect("a dead delta was buffered");
        let run = expect_run_upsert(&last.msg);
        assert_eq!(run.run_id, "ghost");
        assert_eq!(run.state, State::Dead);
        assert_eq!(run.agent_kind, AgentKind::Other, "synthesized identity");
        assert_eq!(run.confidence, Confidence::High, "confirmed exit");
        assert_eq!(run.last_message.as_deref(), Some("vanished"));
    }

    // ── async driver: connection failure on the registration / flush sends ─────

    #[tokio::test]
    async fn driver_logs_session_and_endpoint_on_start() {
        // The `reporter starting` info! event names the session and endpoint via
        // lazily-evaluated fields. Drive run() under an active subscriber (so the
        // fields are actually formatted) on the current thread, with all commands
        // pre-queued and the sender dropped so run() reaches a clean shutdown.
        let hub = MemoryHub::new();
        let reporter = Reporter::new(config(), Box::new(hub.connector()));
        let (reporter, handle, rx) = reporter.with_channel();
        handle.upsert_session(session("sess-test"));
        handle.upsert_run(run("r1", State::Working));
        drop(handle); // closing the channel ⇒ run() shuts down after the backlog

        // A scoped subscriber that records events on *this* thread. run() is
        // awaited inline (not spawned), so its `info!` fires under this guard and
        // the field closures evaluate.
        let subscriber = tracing_subscriber::fmt()
            .with_test_writer()
            .with_max_level(tracing::Level::INFO)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        reporter.run(rx).await.unwrap();

        let delivered = hub.delivered();
        assert_eq!(delivered[0].type_name(), "session.upsert");
    }

    #[test]
    fn reporter_exposes_its_core_for_pre_seeding() {
        // The driver lends out its core (shared & mutable) so a test or the
        // binary can pre-seed durable state before run().
        let mut reporter = Reporter::new(config(), Box::new(MemoryHub::new().connector()));
        assert_eq!(reporter.core().session_id(), "sess-test");
        reporter
            .core_mut()
            .apply(ReporterCommand::UpsertSession(session("sess-test")));
        assert!(
            reporter.core().registration_message().is_some(),
            "the pre-seeded session is visible through the borrowed core"
        );
    }

    #[tokio::test]
    async fn driver_reconnects_when_registration_send_fails() {
        // Pre-seed a session so the very first send on a connection is the
        // *registration*. Then script that first send to fail: the driver must
        // drop the connection and reconnect, ultimately re-registering on the
        // surviving connection (the `registration send failed` arm).
        let hub = MemoryHub::new();
        hub.drop_next_connection_after(0); // the registration send fails outright
        let mut reporter = Reporter::new(config(), Box::new(hub.connector()));
        reporter
            .core_mut()
            .apply(ReporterCommand::UpsertSession(session("sess-test")));
        let (reporter, handle, rx) = reporter.with_channel();
        let task = tokio::spawn(reporter.run(rx));

        handle.upsert_run(run("r1", State::Working));
        handle.shutdown();
        task.await.unwrap().unwrap();

        assert!(
            hub.connect_successes() >= 2,
            "must reconnect after the registration send fails"
        );
        let delivered = hub.delivered();
        assert_eq!(
            delivered[0].type_name(),
            "session.upsert",
            "registration is re-sent on the surviving connection"
        );
    }

    #[tokio::test]
    async fn driver_reconnects_when_backlog_flush_drops_after_registration() {
        // Pre-seed a session *and* a run so, on connect, registration sends first
        // (1 send) and the run is in the backlog. Script a drop after exactly 1
        // send: the registration lands, but the backlog flush's first send fails
        // — exercising the `flush returns false during the backlog flush`
        // reconnect arm. The backlog must still replay on the next connection.
        let hub = MemoryHub::new();
        hub.drop_next_connection_after(1); // reg lands, backlog flush send fails
        let mut reporter = Reporter::new(config(), Box::new(hub.connector()));
        reporter
            .core_mut()
            .apply(ReporterCommand::UpsertSession(session("sess-test")));
        reporter
            .core_mut()
            .apply(ReporterCommand::UpsertRun(run("r1", State::Working)));
        let (reporter, handle, rx) = reporter.with_channel();
        let task = tokio::spawn(reporter.run(rx));

        handle.shutdown();
        task.await.unwrap().unwrap();

        assert!(
            hub.connect_successes() >= 2,
            "the dropped backlog flush must force a reconnect"
        );
        let delivered = hub.delivered();
        assert!(
            delivered.iter().any(
                |m| matches!(m, ClientMessage::RunUpsert { run, .. } if run.state == State::Working)
            ),
            "the backlogged run must still be delivered after the reconnect"
        );
    }

    // ── on_tick: the heartbeat-ticker arm, exercised deterministically ─────────
    //
    // These call `on_tick` directly (no live ticker, no command-arm race) so every
    // branch — heartbeat present/absent, send ok/err, post-reap flush ok/err — is
    // covered without any timing dependence.

    async fn open_conn(hub: &MemoryHub) -> Box<dyn crate::transport::Connection> {
        use crate::transport::Connector;
        hub.connector().connect().await.expect("connect ok")
    }

    #[tokio::test]
    async fn on_tick_sends_heartbeat_and_continues_on_success() {
        // A registered session ⇒ heartbeat_message() is Some. With a connection
        // whose send succeeds, on_tick sends the heartbeat (a session.upsert),
        // flushes (nothing buffered), and returns Continue. Covers the
        // heartbeat-present + send-OK + post-flush-OK path (incl. line 576).
        let hub = MemoryHub::new();
        let mut reporter = Reporter::new(config(), Box::new(hub.connector()));
        reporter
            .core_mut()
            .apply(ReporterCommand::UpsertSession(session("sess-test")));
        // Drain the buffered registration delta so only the heartbeat is in play.
        reporter.core_mut().drain();
        let mut conn = open_conn(&hub).await;

        let outcome = reporter.on_tick(&mut conn, Duration::from_secs(1)).await;

        assert_eq!(outcome, TickOutcome::Continue, "a clean tick keeps serving");
        let delivered = hub.delivered();
        assert_eq!(delivered.len(), 1, "exactly the heartbeat was sent");
        assert_eq!(
            delivered[0].type_name(),
            "session.upsert",
            "the heartbeat is a whole-session upsert"
        );
        // The logical clock advanced to the tick's elapsed time.
        assert_eq!(reporter.core().now(), Duration::from_secs(1));
    }

    #[tokio::test]
    async fn on_tick_reconnects_when_the_heartbeat_send_fails() {
        // A registered session, but the connection's first send fails: on_tick
        // must surface Reconnect from the heartbeat-send arm (line 573→574).
        let hub = MemoryHub::new();
        hub.drop_next_connection_after(0); // the heartbeat send (send #1) fails
        let mut reporter = Reporter::new(config(), Box::new(hub.connector()));
        reporter
            .core_mut()
            .apply(ReporterCommand::UpsertSession(session("sess-test")));
        let mut conn = open_conn(&hub).await;

        let outcome = reporter.on_tick(&mut conn, Duration::from_secs(1)).await;

        assert_eq!(
            outcome,
            TickOutcome::Reconnect,
            "a failed heartbeat send must request a reconnect"
        );
        assert_eq!(hub.delivered_count(), 0, "nothing was delivered");
    }

    #[tokio::test]
    async fn on_tick_without_a_registered_session_skips_the_heartbeat() {
        // No session registered ⇒ heartbeat_message() is None: on_tick skips the
        // heartbeat block entirely (line 570 false), flushes nothing, and returns
        // Continue. No send occurs.
        let hub = MemoryHub::new();
        let mut reporter = Reporter::new(config(), Box::new(hub.connector()));
        let mut conn = open_conn(&hub).await;

        let outcome = reporter.on_tick(&mut conn, Duration::from_secs(1)).await;

        assert_eq!(outcome, TickOutcome::Continue);
        assert_eq!(
            hub.delivered_count(),
            0,
            "no heartbeat without a registered session"
        );
    }

    #[tokio::test]
    async fn on_tick_reaps_timed_out_run_and_reconnects_if_the_dead_flush_fails() {
        // A registered session with a silent, timed-out run: on_tick reaps it to
        // `dead` (lines 566–568), the heartbeat send succeeds, but the trailing
        // flush of the reaped `dead` delta fails ⇒ Reconnect (lines 578–580). The
        // dead delta stays buffered (re-queued) for replay after the reconnect.
        let mut c = config();
        c.liveness_timeout = Duration::from_millis(10);
        let hub = MemoryHub::new();
        // heartbeat (send #1) succeeds; the dead-delta flush (send #2) fails.
        hub.drop_next_connection_after(1);
        let mut reporter = Reporter::new(c, Box::new(hub.connector()));
        reporter
            .core_mut()
            .apply(ReporterCommand::UpsertSession(session("sess-test")));
        reporter
            .core_mut()
            .apply(ReporterCommand::UpsertRun(run("r1", State::Working)));
        // Drain the buffered run delta so only the reap output is in play.
        reporter.core_mut().drain();
        let mut conn = open_conn(&hub).await;

        // Advance well past the liveness timeout so the reap marks r1 dead.
        let outcome = reporter.on_tick(&mut conn, Duration::from_secs(1)).await;

        assert_eq!(
            outcome,
            TickOutcome::Reconnect,
            "a failed post-reap flush must request a reconnect"
        );
        // The heartbeat (session.upsert) was delivered; the dead delta was not
        // (its send failed) and remains buffered for replay.
        let delivered = hub.delivered();
        assert_eq!(delivered.len(), 1, "only the heartbeat got through");
        assert_eq!(delivered[0].type_name(), "session.upsert");
        let buffered = reporter.core_mut().drain();
        assert!(
            buffered.iter().any(|d| matches!(
                &d.msg,
                ClientMessage::RunUpsert { run, .. } if run.state == State::Dead
            )),
            "the reaped dead delta is re-buffered after the failed flush"
        );
    }

    #[tokio::test]
    async fn on_tick_flushes_a_reaped_dead_delta_on_success() {
        // The happy reap path: a timed-out run is reaped to `dead`, the heartbeat
        // sends, and the trailing flush delivers the dead delta — Continue.
        let mut c = config();
        c.liveness_timeout = Duration::from_millis(10);
        let hub = MemoryHub::new();
        let mut reporter = Reporter::new(c, Box::new(hub.connector()));
        reporter
            .core_mut()
            .apply(ReporterCommand::UpsertSession(session("sess-test")));
        reporter
            .core_mut()
            .apply(ReporterCommand::UpsertRun(run("r1", State::Working)));
        reporter.core_mut().drain();
        let mut conn = open_conn(&hub).await;

        let outcome = reporter.on_tick(&mut conn, Duration::from_secs(1)).await;

        assert_eq!(outcome, TickOutcome::Continue);
        let delivered = hub.delivered();
        // Heartbeat (session.upsert) + the reaped dead run.upsert.
        assert!(
            delivered.iter().any(|m| m.type_name() == "session.upsert"),
            "the heartbeat was sent"
        );
        assert!(
            delivered.iter().any(|m| matches!(
                m,
                ClientMessage::RunUpsert { run, .. } if run.state == State::Dead
            )),
            "the reaped dead delta was flushed"
        );
    }
}
