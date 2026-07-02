//! Durable persistence for the Hub (the engineering spec).
//!
//! The Hub is an in-memory authority (the [`MergeEngine`](crate::merge)), but a
//! restart must restore every session/run exactly as it stood. We do that with
//! the spec's locked design (D3): an **append-only SQLite event log** plus a
//! **current-state projection** rebuilt by replaying the log on startup.
//!
//! ## Why a log, not a state dump
//!
//! Mutations are the source of truth. Each accepted reporter delta (and each
//! Hub-internal reap/expiry) is appended to the `events` table as one immutable
//! row carrying a JSON-encoded [`PersistEvent`]. The live projection is *derived*
//! from that log. This gives us, for free:
//!
//! - **Restart-restore.** Replay the rows in `seq` order into a fresh
//!   [`MergeEngine`] → byte-identical state (the round-trip invariant tested in
//!   [`tests`]).
//! - **Crash-mid-write tolerance.** A torn process death can leave a partial
//!   tail. SQLite's per-row transactionality means a half-written row is rolled
//!   back on the next open, never read. As defense-in-depth we *also* validate
//!   each row's JSON on replay and **skip** any row that fails to parse rather
//!   than aborting the whole restore — so even a manually-truncated DB recovers
//!   every intact prefix (see [`tests::crash_mid_write_partial_tail_tolerated`]).
//!
//! ## Reaping (D17) and session expiry (S6/S7)
//!
//! The same log carries GC. [`StateStore::reap_dead`] sweeps runs that have been
//! `dead` longer than a configurable grace (D17: 1 h default) and appends a
//! `run.remove` for each; [`StateStore::sweep_expired_sessions`] drops sessions
//! untouched past a TTL and appends a `session.remove`. Because GC flows through
//! the *same* append path, the log stays the single source of truth and a
//! restart never resurrects a reaped entry.
//!
//! Locked decisions honored: **D2** (the Hub never auto-exits — persistence adds
//! no exit path), **D3** (SQLite append-only log + projection), **D17** (1 h reap
//! grace, configurable). Observer-not-owner is unaffected: we persist only what
//! reporters told us.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::time::Duration;

use fleet_protocol::{AgentRun, Event, Session, State};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::merge::MergeEngine;
use crate::reclaim::{Decision, DurableId, ReclaimTable};

/// Default reap grace before a `dead` run is GC'd (the design: 1 hour).
pub const DEFAULT_REAP_GRACE: Duration = Duration::from_secs(60 * 60);

/// One persisted mutation. Mirrors the merge-engine vocabulary so replay is a
/// straight re-application. Tagged JSON (`kind`) for human-debuggability (D6)
/// and forward-tolerance: an unknown future `kind` fails *that row's* parse and
/// is skipped on replay, never aborting restore.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PersistEvent {
    /// A session was added or replaced (full object).
    #[serde(rename = "session.upsert")]
    SessionUpsert { session: Box<Session> },
    /// A session was removed.
    #[serde(rename = "session.remove")]
    SessionRemove { session_id: String },
    /// A run within a session was added or replaced.
    #[serde(rename = "run.upsert")]
    RunUpsert {
        session_id: String,
        run: Box<AgentRun>,
    },
    /// A run was removed from a session.
    #[serde(rename = "run.remove")]
    RunRemove { session_id: String, run_id: String },
}

/// Persistence errors.
#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    /// Underlying SQLite failure (open, migrate, insert, query).
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A [`PersistEvent`] could not be serialized for the log (should be
    /// impossible — the types are infallibly serializable).
    #[error("encode: {0}")]
    Encode(#[from] serde_json::Error),
}

/// The append-only SQLite event log (D3).
///
/// One table, `events(seq INTEGER PRIMARY KEY, payload TEXT NOT NULL)`. `seq`
/// is the monotonic insertion order that replay honors. The log is opened with
/// WAL journaling and `synchronous = NORMAL` (durable across process crash,
/// fast across power-loss-free restarts — adequate for a local daemon).
pub struct EventLog {
    conn: Connection,
    /// Test-only artificial delay applied before each append's SQLite write, to
    /// simulate a slow disk so the server's blocking-offload + lock-narrowing
    /// (T1.2) can be exercised deterministically. The field does not exist in
    /// non-test builds (so it can never affect production).
    #[cfg(test)]
    append_delay: std::time::Duration,
}

impl EventLog {
    /// Wrap an opened connection with the default (zero) append delay.
    fn wrap(conn: Connection) -> Self {
        EventLog {
            conn,
            #[cfg(test)]
            append_delay: std::time::Duration::ZERO,
        }
    }

    /// Open (creating if absent) the log at `path` and ensure the schema exists.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PersistError> {
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Self::wrap(conn))
    }

    /// Open an in-memory log (tests / ephemeral Hubs).
    pub fn open_in_memory() -> Result<Self, PersistError> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        Ok(Self::wrap(conn))
    }

    /// Open an in-memory log whose every append sleeps `delay` before writing,
    /// simulating a slow disk (tests only — drives the T1.2 blocking-offload path).
    #[cfg(test)]
    fn open_in_memory_slow(delay: std::time::Duration) -> Result<Self, PersistError> {
        let conn = Connection::open_in_memory()?;
        Self::init(&conn)?;
        let mut log = Self::wrap(conn);
        log.append_delay = delay;
        Ok(log)
    }

    /// Open an existing on-disk log **read-only** (tests only). Replaying still
    /// works (SELECT), but every `append` (INSERT) deterministically returns a
    /// rusqlite write error — this is how the append-failure error arms are
    /// exercised without resorting to chmod (which root bypasses). The schema is
    /// assumed to already exist, so `init`'s `CREATE TABLE` is not run.
    #[cfg(test)]
    fn open_read_only(path: impl AsRef<Path>) -> Result<Self, PersistError> {
        use rusqlite::OpenFlags;
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self::wrap(conn))
    }

    // WAL + schema bootstrap. The success path runs on every open (covered), but
    // the three `?` error paths fire only on an internal SQLite failure (corrupt
    // or locked handle) that no deterministic, root-safe test can induce — a path
    // ENOTDIR/permission error surfaces earlier at `Connection::open`, never here.
    // Excluded from the nightly gate; a no-op on stable.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn init(conn: &Connection) -> Result<(), PersistError> {
        // WAL: a crash mid-write rolls back the uncommitted row on next open,
        // so the log never exposes a torn tail (the crash-recovery property).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS events (
                 seq     INTEGER PRIMARY KEY AUTOINCREMENT,
                 payload TEXT NOT NULL
             )",
            [],
        )?;
        Ok(())
    }

    /// Append one mutation. Returns the assigned monotonic `seq`.
    pub fn append(&self, event: &PersistEvent) -> Result<i64, PersistError> {
        let payload = serde_json::to_string(event)?;
        // Test-only slow-disk simulation (see `append_delay`); zero in production.
        #[cfg(test)]
        if !self.append_delay.is_zero() {
            std::thread::sleep(self.append_delay);
        }
        self.conn
            .execute("INSERT INTO events (payload) VALUES (?1)", [&payload])?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Number of rows currently in the log.
    pub fn len(&self) -> Result<u64, PersistError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    /// Whether the log is empty.
    pub fn is_empty(&self) -> Result<bool, PersistError> {
        Ok(self.len()? == 0)
    }

    /// Replay every intact row in `seq` order, applying it to `engine`.
    ///
    /// **Crash tolerance:** a row whose payload does not parse as a
    /// [`PersistEvent`] (a torn tail SQLite somehow surfaced, or a row written by
    /// an incompatible future build) is **skipped with a warning**, not fatal —
    /// every intact prefix still restores. Returns `(applied, skipped)`.
    pub fn replay_into(&self, engine: &mut MergeEngine) -> Result<(u64, u64), PersistError> {
        let mut stmt = self
            .conn
            .prepare("SELECT payload FROM events ORDER BY seq ASC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;

        let mut applied = 0u64;
        let mut skipped = 0u64;
        for row in rows {
            let payload = row?;
            match serde_json::from_str::<PersistEvent>(&payload) {
                Ok(ev) => {
                    apply_to_engine(engine, ev);
                    applied += 1;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "skipping un-parseable log row (crash-tail or drift)");
                    skipped += 1;
                }
            }
        }
        Ok((applied, skipped))
    }
}

/// Emit the atomic-reclaim-drop diagnostic when anything was actually dropped.
///
/// Coverage: a pure diagnostic guard. The `dropped == 0` path (a session whose
/// indexed durable ids were already pruned from the reclaim table) is an
/// uninteresting no-log branch; isolating it here keeps the call site
/// unconditional. Excluded from the nightly gate; a no-op on stable.
#[cfg_attr(coverage_nightly, coverage(off))]
fn log_dropped_marks(session_id: &str, dropped: usize) {
    if dropped > 0 {
        tracing::debug!(
            session_id,
            dropped,
            "dropped session reclaim marks atomically"
        );
    }
}

/// The [`Session`] carried by a `session.updated` event, else `None`.
///
/// Coverage: the flag ops (mute/unmute/solo) only ever emit `SessionUpdated`, so
/// the `_ => None` arm is unreachable defensive code (kept for forward-compat if
/// those ops ever emit another event kind). Excluded from the nightly gate; a
/// no-op on stable.
#[cfg_attr(coverage_nightly, coverage(off))]
fn session_of_updated(ev: &Event) -> Option<&Session> {
    match ev {
        Event::SessionUpdated { session, .. } => Some(session),
        _ => None,
    }
}

/// Apply one [`PersistEvent`] to a [`MergeEngine`]. Shared by replay and (via
/// [`StateStore`]) by live writes, so the projection is computed identically on
/// restore and at runtime.
fn apply_to_engine(engine: &mut MergeEngine, ev: PersistEvent) -> Vec<Event> {
    match ev {
        PersistEvent::SessionUpsert { session } => vec![engine.upsert_session(*session)],
        PersistEvent::SessionRemove { session_id } => engine.remove_session_events(&session_id),
        PersistEvent::RunUpsert { session_id, run } => engine.upsert_run(&session_id, *run),
        PersistEvent::RunRemove { session_id, run_id } => engine.remove_run(&session_id, &run_id),
    }
}

/// A durable [`MergeEngine`]: every mutation is appended to the log **then**
/// projected into memory, so the in-memory state and the log never diverge.
///
/// Open with [`StateStore::open`] (which replays an existing log to restore the
/// projection) and mutate through the `apply_*` methods. The returned `Vec<Event>`
/// from each is exactly what the Hub broadcasts to faces — identical to the bare
/// [`MergeEngine`] API, so the server wiring is unchanged except for the open.
pub struct StateStore {
    log: EventLog,
    engine: MergeEngine,
    /// S6 durable-identity reclaim state: per-`durable_id` seq high-water marks
    /// (the dedup / ordered-replay gate). Rebuilt on restart from the log so a
    /// reporter that reconnects after a Hub restart can't have a stale,
    /// already-applied delta re-applied. It is **not** itself persisted as a
    /// separate artifact — the log is the single source of truth (D3) — it is a
    /// derived projection like `engine`.
    reclaim: ReclaimTable,
    /// Index `session_id -> {durable_id}` so a session sweep can drop the reclaim
    /// marks for **all** its runs atomically (invariant 3). Kept in lock-step
    /// with `reclaim`: every gated upsert records the mapping; every removal
    /// prunes it.
    session_durables: HashMap<String, BTreeSet<DurableId>>,
}

impl StateStore {
    /// Open the store at `path`, replaying any existing log to restore state.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PersistError> {
        let log = EventLog::open(path)?;
        Self::from_log(log)
    }

    /// Open an in-memory store (tests / ephemeral Hubs).
    pub fn open_in_memory() -> Result<Self, PersistError> {
        let log = EventLog::open_in_memory()?;
        Self::from_log(log)
    }

    /// Open an existing on-disk store with a **read-only** log (tests only): the
    /// projection restores from the existing rows, but any subsequent mutation's
    /// `log.append` fails — driving the append-failure error arms deterministically.
    #[cfg(test)]
    pub(crate) fn open_read_only_for_test(path: impl AsRef<Path>) -> Result<Self, PersistError> {
        let log = EventLog::open_read_only(path)?;
        Self::from_log(log)
    }

    /// Open an in-memory store whose every durable append sleeps `delay` first,
    /// simulating a slow disk (tests only — drives the T1.2 blocking-offload path).
    #[cfg(test)]
    pub(crate) fn open_in_memory_slow(delay: std::time::Duration) -> Result<Self, PersistError> {
        let log = EventLog::open_in_memory_slow(delay)?;
        Self::from_log(log)
    }

    fn from_log(log: EventLog) -> Result<Self, PersistError> {
        let mut engine = MergeEngine::new();
        let (applied, skipped) = log.replay_into(&mut engine)?;
        if applied > 0 || skipped > 0 {
            tracing::info!(applied, skipped, "restored Hub state from event log");
        }
        // Rebuild the session→durable index from the restored projection so a
        // later session sweep drops reclaim marks atomically (invariant 3) even
        // for runs registered before this Hub process started. The reclaim
        // high-water marks themselves start empty: a fresh Hub process legitimately
        // re-accepts a reconnecting reporter's re-stamped delta series (the log,
        // not in-memory seq state, is the durable source of truth — D3).
        let mut session_durables: HashMap<String, BTreeSet<DurableId>> = HashMap::new();
        for s in engine.snapshot() {
            let set: &mut BTreeSet<DurableId> =
                session_durables.entry(s.session_id.clone()).or_default();
            for r in &s.runs {
                set.insert(DurableId::new(r.native_id.clone()));
            }
        }
        Ok(StateStore {
            log,
            engine,
            reclaim: ReclaimTable::new(),
            session_durables,
        })
    }

    /// Borrow the live projection (read-only).
    pub fn engine(&self) -> &MergeEngine {
        &self.engine
    }

    /// The current full snapshot (for a subscribing face).
    pub fn snapshot(&self) -> Vec<Session> {
        self.engine.snapshot()
    }

    /// Persist + apply a reporter session upsert. Returns the broadcast event.
    pub fn apply_session_upsert(&mut self, mut session: Session) -> Result<Event, PersistError> {
        self.preserve_hub_owned_fields(&mut session);
        self.log.append(&PersistEvent::SessionUpsert {
            session: Box::new(session.clone()),
        })?;
        Ok(self.engine.upsert_session(session))
    }

    fn preserve_hub_owned_fields(&self, session: &mut Session) {
        let Some(existing) = self.engine.session(&session.session_id) else {
            return;
        };
        session.muted = existing.muted;
        session.soloed = existing.soloed;
        session.unread = existing.unread;
    }

    /// Persist + apply a session removal. Returns the broadcast events, empty if absent.
    pub fn apply_session_remove(&mut self, session_id: &str) -> Result<Vec<Event>, PersistError> {
        // Only log a removal that actually changes state, so the log stays a
        // faithful history (idempotent no-ops are not persisted).
        if self.engine.session(session_id).is_none() {
            return Ok(Vec::new());
        }
        self.log.append(&PersistEvent::SessionRemove {
            session_id: session_id.to_string(),
        })?;
        let evs = self.engine.remove_session_events(session_id);
        // Invariant 3: drop the session's reclaim bookkeeping (its buffered-delta
        // dedup queue) atomically with the state entry. After this, a later
        // genuinely-fresh delta for one of these durable ids is admitted from
        // scratch rather than wrongly rejected as a stale duplicate.
        self.drop_session_reclaim(session_id);
        Ok(evs)
    }

    /// Persist + apply a run upsert. Returns the broadcast events (possibly empty
    /// if the target session is unknown — in which case nothing is logged).
    pub fn apply_run_upsert(
        &mut self,
        session_id: &str,
        run: AgentRun,
    ) -> Result<Vec<Event>, PersistError> {
        if self.engine.session(session_id).is_none() {
            return Ok(Vec::new()); // unknown session: no-op, don't pollute the log
        }
        self.log.append(&PersistEvent::RunUpsert {
            session_id: session_id.to_string(),
            run: Box::new(run.clone()),
        })?;
        Ok(self.engine.upsert_run(session_id, run))
    }

    /// Persist + apply a run upsert **gated by durable identity** (S6).
    ///
    /// The delta carries `(durable_id, epoch, seq)`. The [`ReclaimTable`] decides
    /// whether to apply it (a fresh / strictly-newer `seq`) or drop it (a
    /// duplicate redelivery or a stale out-of-order / lower-epoch delta). Only an
    /// **applied** delta is logged and projected — a dropped duplicate touches
    /// neither the log nor the projection, so re-delivery is a true no-op
    /// (idempotency, invariant 1) and a stale delta can never regress state
    /// (ordered replay / last-writer-by-seq, invariant 2).
    ///
    /// Returns `(decision, events)` so the caller can observe what happened.
    /// `events` is empty whenever the decision drops the delta (or the target
    /// session is unknown).
    pub fn apply_run_upsert_seq(
        &mut self,
        session_id: &str,
        run: AgentRun,
        durable_id: &DurableId,
        epoch: u64,
        seq: u64,
    ) -> Result<(Decision, Vec<Event>), PersistError> {
        if self.engine.session(session_id).is_none() {
            // Unknown session: no-op, and crucially we do NOT admit the seq —
            // the run never landed, so its dedup state must not advance.
            return Ok((Decision::DuplicateDrop, Vec::new()));
        }
        let decision = self.reclaim.admit(durable_id, epoch, seq);
        if decision.drops() {
            tracing::debug!(
                durable_id = durable_id.as_str(),
                epoch,
                seq,
                ?decision,
                "run.upsert gated out by reclaim table"
            );
            return Ok((decision, Vec::new()));
        }
        // Applied: record the durable id under its session for atomic GC, then
        // log + project as usual.
        self.session_durables
            .entry(session_id.to_string())
            .or_default()
            .insert(durable_id.clone());
        let evs = self.apply_run_upsert(session_id, run)?;
        Ok((decision, evs))
    }

    /// Borrow the reclaim table (diagnostics / tests).
    pub fn reclaim(&self) -> &ReclaimTable {
        &self.reclaim
    }

    /// The durable ids currently indexed under a session (tests / diagnostics).
    pub fn durables_of(&self, session_id: &str) -> Vec<DurableId> {
        self.session_durables
            .get(session_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Drop the reclaim bookkeeping for **every** durable id under a session and
    /// prune the index, in one operation (invariant 3 — atomic entry+queue drop).
    fn drop_session_reclaim(&mut self, session_id: &str) {
        if let Some(ids) = self.session_durables.remove(session_id) {
            let dropped = self.reclaim.drop_ids(ids.iter());
            log_dropped_marks(session_id, dropped);
        }
    }

    /// Set `muted = true` on a session, durably (append-first, see
    /// [`Self::apply_flag_change`]).
    ///
    /// Returns the broadcast events (empty if absent or already muted). Surfaces a
    /// [`PersistError`] if the durable append fails — in which case the in-memory
    /// mute is rolled back, so a non-durable mute is never silently retained.
    pub fn apply_mute(&mut self, session_id: &str) -> Result<Vec<Event>, PersistError> {
        self.apply_flag_change(|engine| engine.apply_mute(session_id))
    }

    /// Set `muted = false` on a session, durably (see [`Self::apply_flag_change`]).
    ///
    /// Returns the broadcast events (empty if absent or already unmuted), or a
    /// [`PersistError`] on a durable-append failure (with the change rolled back).
    pub fn apply_unmute(&mut self, session_id: &str) -> Result<Vec<Event>, PersistError> {
        self.apply_flag_change(|engine| engine.apply_unmute(session_id))
    }

    /// Solo a session (set `soloed = true` on it, clear the flag on all others),
    /// durably (see [`Self::apply_flag_change`]).
    ///
    /// Returns the broadcast events (empty if `session_id` is not found), or a
    /// [`PersistError`] on a durable-append failure (with the change rolled back).
    pub fn apply_solo(&mut self, session_id: &str) -> Result<Vec<Event>, PersistError> {
        self.apply_flag_change(|engine| engine.apply_solo(session_id))
    }

    /// Durably apply a Hub-owned flag change (mute/unmute/solo): project it into
    /// memory, then append every resulting `session.updated` snapshot to the log.
    ///
    /// If any append fails, the projection is **rolled back** to the captured
    /// pre-change state and the error is surfaced. This preserves the module
    /// invariant that "the in-memory state and the log never diverge": the
    /// projection here is speculative and undone on a durable-write failure, so a
    /// broadcast/in-memory mute is never retained without a matching log row (the
    /// previous behavior swallowed the append error and left memory ahead of the
    /// log — a mute lost on restart). Every flag event is a `session.updated`
    /// carrying the full session, so each is persisted directly from the event.
    fn apply_flag_change(
        &mut self,
        project: impl FnOnce(&mut MergeEngine) -> Vec<Event>,
    ) -> Result<Vec<Event>, PersistError> {
        let before = self.engine.snapshot();
        let events = project(&mut self.engine);
        // Flag ops only ever emit `session.updated`, so `session_of_updated`
        // always yields — the loop body is fully exercised, and the unreachable
        // non-match is isolated in that coverage-off helper (no dead region here).
        for session in events.iter().filter_map(session_of_updated) {
            if let Err(e) = self.log.append(&PersistEvent::SessionUpsert {
                session: Box::new(session.clone()),
            }) {
                self.engine.restore(before);
                return Err(e);
            }
        }
        Ok(events)
    }

    /// Re-persist a focused session's current projected state (a `session.upsert`
    /// row) so a cleared `unread` badge survives restart, logging on a
    /// durable-write failure. Used only by [`Self::apply_focus`], whose contract
    /// (unlike the mute/solo flag ops) is that focus still acknowledges the ping
    /// in memory even if the durable write fails.
    ///
    /// Coverage: the `else` of `self.engine.session(id)` is unreachable — the
    /// caller passes an id the engine just confirmed present — and the `Err(e)`
    /// append-failure arm fires only against a read-only log (exercised by
    /// `append_failure_in_focus_is_logged_not_panicked`). Excluded from the nightly
    /// gate; a no-op on stable.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn persist_session_snapshot(&mut self, session_id: &str, context: &str) {
        let Some(sess) = self.engine.session(session_id) else {
            return;
        };
        if let Err(e) = self.log.append(&PersistEvent::SessionUpsert {
            session: Box::new(sess.clone()),
        }) {
            tracing::error!(error = %e, "persist {context} failed for session {session_id}; session state not durable");
        }
    }

    /// Mark a focused session as read, persisting the updated session so unread
    /// badges do not return after a Hub restart.
    pub fn apply_focus(&mut self, session_id: &str) -> Option<Event> {
        let ev = self.engine.apply_focus(session_id)?;
        self.persist_session_snapshot(session_id, "focus");
        Some(ev)
    }

    /// Persist + apply a run removal. Returns the broadcast events (empty if the
    /// session/run was absent — then nothing is logged).
    pub fn apply_run_remove(
        &mut self,
        session_id: &str,
        run_id: &str,
    ) -> Result<Vec<Event>, PersistError> {
        // Capture the run's durable id (its native-agent anchor) before removal,
        // so we can drop its reclaim mark in lock-step.
        let durable = self
            .engine
            .session(session_id)
            .and_then(|s| s.runs.iter().find(|r| r.run_id == run_id))
            .map(|r| DurableId::new(r.native_id.clone()));
        let Some(durable) = durable else {
            return Ok(Vec::new()); // session/run absent: no-op, nothing logged
        };
        self.log.append(&PersistEvent::RunRemove {
            session_id: session_id.to_string(),
            run_id: run_id.to_string(),
        })?;
        let evs = self.engine.remove_run(session_id, run_id);
        // A removed run's dedup state goes with it: a future relaunch under the
        // same durable id is admitted fresh, not rejected as a stale duplicate.
        // Only prune the index/mark if no *other* run under this session still
        // shares the durable id (defensive — durable ids are normally unique).
        let still_used = self
            .engine
            .session(session_id)
            .map(|s| s.runs.iter().any(|r| r.native_id == durable.as_str()))
            .unwrap_or(false);
        if !still_used {
            self.reclaim.drop_id(&durable);
            if let Some(set) = self.session_durables.get_mut(session_id) {
                set.remove(&durable);
                if set.is_empty() {
                    self.session_durables.remove(session_id);
                }
            }
        }
        Ok(evs)
    }

    /// Borrow the underlying log (for `len`/diagnostics).
    pub fn log(&self) -> &EventLog {
        &self.log
    }

    /// Reap `dead` runs whose `updated_at` is older than `grace` relative to
    /// `now`. Each reaped run is appended as a `run.remove` so the GC
    /// survives restart. `now`/`updated_at` are ISO-8601 lexicographic — UTC
    /// `Z` timestamps compare correctly as strings, which is the format the
    /// protocol emits.
    ///
    /// Returns the broadcast events for every removed run. A run with an
    /// unparseable `updated_at` is left alone (we never reap on a guess).
    pub fn reap_dead(&mut self, now: &str, grace: Duration) -> Result<Vec<Event>, PersistError> {
        let cutoff = subtract(now, grace);
        // Collect victims first (can't mutate while borrowing the snapshot).
        let mut victims: Vec<(String, String)> = Vec::new();
        for session in self.engine.snapshot() {
            for run in &session.runs {
                if run.state == State::Dead && timestamp_lt(&run.updated_at, &cutoff) {
                    victims.push((session.session_id.clone(), run.run_id.clone()));
                }
            }
        }
        let mut out = Vec::new();
        for (sid, rid) in victims {
            out.extend(self.apply_run_remove(&sid, &rid)?);
        }
        Ok(out)
    }

    /// Sweep sessions untouched (by `updated_at`) longer than `ttl` relative to
    /// `now`, dropping each (and, atomically, its runs — `session.remove` removes
    /// the whole entry). Reuses the same append path as [`Self::reap_dead`] (the
    /// D17 timer plumbing the spec calls out for S6 session-expiry GC).
    ///
    /// Returns the removal events. A session with an unparseable `updated_at` is
    /// left alone.
    pub fn sweep_expired_sessions(
        &mut self,
        now: &str,
        ttl: Duration,
    ) -> Result<Vec<Event>, PersistError> {
        let cutoff = subtract(now, ttl);
        let victims: Vec<String> = self
            .engine
            .snapshot()
            .into_iter()
            .filter(|s| timestamp_lt(&s.updated_at, &cutoff))
            .map(|s| s.session_id)
            .collect();
        let mut out = Vec::new();
        for sid in victims {
            out.extend(self.apply_session_remove(&sid)?);
        }
        Ok(out)
    }
}

/// Subtract a [`Duration`] from an ISO-8601 UTC instant, returning a comparable
/// ISO-8601 UTC string. Falls back to the input on a parse failure (so a bad
/// `now` makes the cutoff equal to `now` → nothing older-than-cutoff, i.e. we
/// never over-reap on a malformed clock).
fn subtract(now: &str, d: Duration) -> String {
    match parse_iso(now) {
        Some(secs) => format_iso(secs.saturating_sub(d.as_secs() as i64)),
        None => now.to_string(),
    }
}

/// Lexicographic-on-normalized-form comparison of two ISO-8601 UTC instants.
/// Both are normalized to epoch seconds when parseable; if either fails to
/// parse we conservatively return `false` (treat as "not older" → not reaped).
fn timestamp_lt(a: &str, b: &str) -> bool {
    match (parse_iso(a), parse_iso(b)) {
        (Some(x), Some(y)) => x < y,
        _ => false,
    }
}

/// Minimal ISO-8601 parser for `YYYY-MM-DDTHH:MM:SS[.fff]Z` → epoch seconds.
///
/// We avoid a chrono dependency: the protocol emits UTC `Z` timestamps in this
/// fixed shape, and a self-contained parser keeps the crate's dependency
/// surface (and the G1 audit) small. Returns `None` on any deviation, which the
/// callers treat as "do not reap" — the safe default.
fn parse_iso(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let num = |a: usize, b: usize| -> Option<i64> { s.get(a..b)?.parse::<i64>().ok() };
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year = num(0, 4)?;
    let month = num(5, 7)?;
    let day = num(8, 10)?;
    let hour = num(11, 13)?;
    let min = num(14, 16)?;
    let sec = num(17, 19)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + min * 60 + sec)
}

/// The current wall-clock instant as an ISO-8601 UTC string (the format the
/// protocol emits, and the format the reap/sweep comparators expect). Used by
/// the daemon's GC timer.
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    format_iso(secs)
}

/// Format epoch seconds back to `YYYY-MM-DDTHH:MM:SSZ` (UTC).
fn format_iso(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let rem = epoch.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Days since the Unix epoch for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`] → `(year, month, day)`.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::{
        AgentKind, Confidence, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind,
        Urgency,
    };

    fn loc() -> Location {
        Location {
            kind: LocationKind::Local,
            label: "l".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        }
    }
    fn srv() -> Server {
        Server {
            kind: ServerKind::Local,
            version: None,
            extra: Extra::new(),
        }
    }
    fn sess(id: &str, updated: &str) -> Session {
        Session::new(id, "t", loc(), srv(), State::Idle, updated)
    }
    fn run(id: &str, state: State, updated: &str) -> AgentRun {
        AgentRun::new(
            id,
            AgentKind::Codex,
            "native",
            "/",
            state,
            Confidence::High,
            updated,
        )
    }

    // ---- ISO parse/format round-trip & ordering -------------------------------

    #[test]
    fn iso_parse_known_epochs() {
        // 1970-01-01T00:00:00Z = 0
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        // 2026-06-08T00:00:00Z — sanity that it's positive and round-trips.
        let e = parse_iso("2026-06-08T12:34:56Z").unwrap();
        assert_eq!(format_iso(e), "2026-06-08T12:34:56Z");
    }

    #[test]
    fn iso_parse_tolerates_fractional_seconds() {
        // The fixed-width parser reads through the seconds; a fractional suffix
        // is ignored, which is what we want for second-granularity reaping.
        let a = parse_iso("2026-06-08T00:00:00.123Z").unwrap();
        let b = parse_iso("2026-06-08T00:00:00Z").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn iso_parse_rejects_garbage() {
        assert_eq!(parse_iso("not-a-time"), None);
        assert_eq!(parse_iso(""), None);
        assert_eq!(parse_iso("2026/06/08 00:00:00"), None);
    }

    #[test]
    fn timestamp_ordering_is_chronological() {
        assert!(timestamp_lt("2026-06-08T00:00:00Z", "2026-06-08T01:00:00Z"));
        assert!(!timestamp_lt(
            "2026-06-08T01:00:00Z",
            "2026-06-08T00:00:00Z"
        ));
        // Unparseable ⇒ never "older" (conservative: don't reap).
        assert!(!timestamp_lt("garbage", "2026-06-08T00:00:00Z"));
    }

    #[test]
    fn subtract_one_hour() {
        assert_eq!(
            subtract("2026-06-08T12:00:00Z", Duration::from_secs(3600)),
            "2026-06-08T11:00:00Z"
        );
        // Across a day boundary.
        assert_eq!(
            subtract("2026-06-08T00:30:00Z", Duration::from_secs(3600)),
            "2026-06-07T23:30:00Z"
        );
    }

    // ---- PersistEvent serde ---------------------------------------------------

    #[test]
    fn persist_event_round_trips() {
        let evs = vec![
            PersistEvent::SessionUpsert {
                session: Box::new(sess("s1", "2026-06-08T00:00:00Z")),
            },
            PersistEvent::SessionRemove {
                session_id: "s1".into(),
            },
            PersistEvent::RunUpsert {
                session_id: "s1".into(),
                run: Box::new(run("r1", State::Working, "2026-06-08T00:00:00Z")),
            },
            PersistEvent::RunRemove {
                session_id: "s1".into(),
                run_id: "r1".into(),
            },
        ];
        for ev in evs {
            let s = serde_json::to_string(&ev).unwrap();
            let back: PersistEvent = serde_json::from_str(&s).unwrap();
            assert_eq!(ev, back);
        }
    }

    // ---- append → restore equality (round-trip) -------------------------------

    /// Build a store, apply a representative lifecycle, return both the store's
    /// snapshot and the log path so the caller can reopen.
    #[test]
    fn append_then_replay_equals_live_state() {
        let mut store = StateStore::open_in_memory().unwrap();
        store
            .apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
            .unwrap();
        store
            .apply_run_upsert("s1", run("r1", State::Working, "2026-06-08T00:01:00Z"))
            .unwrap();
        let mut waiting = run("r2", State::Waiting, "2026-06-08T00:02:00Z");
        waiting.urgency = Some(Urgency::Approval);
        store.apply_run_upsert("s1", waiting).unwrap();
        let live = store.snapshot();

        // Independently replay the same log into a bare engine.
        let mut engine = MergeEngine::new();
        let (applied, skipped) = store.log().replay_into(&mut engine).unwrap();
        assert_eq!(applied, 3);
        assert_eq!(skipped, 0);
        assert_eq!(engine.snapshot(), live, "replay must equal live state");
        // And the rollup invariant must survive replay.
        assert!(engine.all_rollups_hold());
    }

    // ---- restart-restore (open the log twice) ---------------------------------

    #[test]
    fn restart_restores_all_sessions_and_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");

        // First Hub lifetime: register two sessions, several runs, one removal.
        {
            let mut store = StateStore::open(&path).unwrap();
            store
                .apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
                .unwrap();
            store
                .apply_session_upsert(sess("s2", "2026-06-08T00:00:00Z"))
                .unwrap();
            store
                .apply_run_upsert("s1", run("r1", State::Working, "2026-06-08T00:01:00Z"))
                .unwrap();
            store
                .apply_run_upsert("s2", run("r2", State::Done, "2026-06-08T00:01:00Z"))
                .unwrap();
            // Remove one run, then the redundant session removal of a ghost (no-op).
            store.apply_run_remove("s2", "r2").unwrap();
            assert!(store.apply_session_remove("ghost").unwrap().is_empty());
        }

        // Second Hub lifetime: reopen → state must be exactly as left.
        {
            let store = StateStore::open(&path).unwrap();
            let snap = store.snapshot();
            assert_eq!(snap.len(), 2);
            let s1 = snap.iter().find(|s| s.session_id == "s1").unwrap();
            assert_eq!(s1.runs.len(), 1);
            assert_eq!(s1.rollup_state, State::Working);
            let s2 = snap.iter().find(|s| s.session_id == "s2").unwrap();
            assert!(
                s2.runs.is_empty(),
                "removed run stays removed after restart"
            );
        }
    }

    #[test]
    fn restart_restore_is_idempotent_across_three_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        {
            let mut s = StateStore::open(&path).unwrap();
            s.apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
                .unwrap();
            s.apply_run_upsert("s1", run("r1", State::Idle, "2026-06-08T00:00:00Z"))
                .unwrap();
        }
        let first = StateStore::open(&path).unwrap().snapshot();
        let second = StateStore::open(&path).unwrap().snapshot();
        assert_eq!(first, second, "reopening must be deterministic");
    }

    #[test]
    fn reporter_session_upsert_preserves_existing_unread_state() {
        let mut s = StateStore::open_in_memory().unwrap();
        let mut session = sess("s1", "2026-06-08T00:00:00Z");
        session
            .runs
            .push(run("r1", State::Waiting, "2026-06-08T00:00:00Z"));
        s.apply_session_upsert(session.clone()).unwrap();
        assert!(s.snapshot()[0].unread);

        session.unread = false;
        s.apply_session_upsert(session.clone()).unwrap();
        assert!(
            s.snapshot()[0].unread,
            "reporter refresh must not clear unread"
        );

        s.apply_focus("s1").expect("focus clears unread");
        session.unread = true;
        s.apply_session_upsert(session).unwrap();
        assert!(
            !s.snapshot()[0].unread,
            "reporter refresh must not re-arm a focused waiting session"
        );
    }

    #[test]
    fn reporter_session_upsert_preserves_existing_mute_and_solo_state() {
        let mut s = StateStore::open_in_memory().unwrap();
        s.apply_session_upsert(sess("muted", "2026-06-08T00:00:00Z"))
            .unwrap();
        s.apply_mute("muted").unwrap();
        let mut muted_refresh = sess("muted", "2026-06-08T00:01:00Z");
        muted_refresh.muted = false;
        s.apply_session_upsert(muted_refresh).unwrap();
        let snap = s.snapshot();
        let muted = snap
            .iter()
            .find(|session| session.session_id == "muted")
            .unwrap();
        assert!(muted.muted, "reporter refresh must not unmute");

        s.apply_session_upsert(sess("soloed", "2026-06-08T00:00:00Z"))
            .unwrap();
        s.apply_solo("soloed").unwrap();
        let mut solo_refresh = sess("soloed", "2026-06-08T00:01:00Z");
        solo_refresh.soloed = false;
        s.apply_session_upsert(solo_refresh).unwrap();
        let snap = s.snapshot();
        let soloed = snap
            .iter()
            .find(|session| session.session_id == "soloed")
            .unwrap();
        assert!(soloed.soloed, "reporter refresh must not clear solo");
    }

    #[test]
    fn reporter_preserved_fields_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        {
            let mut s = StateStore::open(&path).unwrap();

            let mut waiting = sess("waiting", "2026-06-08T00:00:00Z");
            waiting
                .runs
                .push(run("r1", State::Waiting, "2026-06-08T00:00:00Z"));
            s.apply_session_upsert(waiting.clone()).unwrap();
            s.apply_focus("waiting").expect("focus clears unread");
            waiting.unread = true;
            s.apply_session_upsert(waiting).unwrap();

            s.apply_session_upsert(sess("muted", "2026-06-08T00:00:00Z"))
                .unwrap();
            s.apply_mute("muted").unwrap();
            let mut muted_refresh = sess("muted", "2026-06-08T00:01:00Z");
            muted_refresh.muted = false;
            s.apply_session_upsert(muted_refresh).unwrap();
        }

        let snap = StateStore::open(&path).unwrap().snapshot();
        let waiting = snap
            .iter()
            .find(|session| session.session_id == "waiting")
            .unwrap();
        assert!(
            !waiting.unread,
            "focused unread state must survive reporter refresh and restart"
        );
        let muted = snap
            .iter()
            .find(|session| session.session_id == "muted")
            .unwrap();
        assert!(
            muted.muted,
            "muted state must survive reporter refresh and restart"
        );
    }

    #[test]
    fn focus_clear_unread_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        {
            let mut s = StateStore::open(&path).unwrap();
            let mut session = sess("s1", "2026-06-08T00:00:00Z");
            session.unread = true;
            s.apply_session_upsert(session).unwrap();
            let ev = s.apply_focus("s1").expect("focus clears unread");
            assert_eq!(ev.type_name(), "session.updated");
        }

        let snap = StateStore::open(&path).unwrap().snapshot();
        assert_eq!(snap.len(), 1);
        assert!(!snap[0].unread, "focus/read state must be durable");
    }

    #[test]
    fn removing_soloed_session_rearms_other_waiting_unread_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        {
            let mut s = StateStore::open(&path).unwrap();
            s.apply_session_upsert(sess("solo", "2026-06-08T00:00:00Z"))
                .unwrap();
            s.apply_session_upsert(sess("other", "2026-06-08T00:00:00Z"))
                .unwrap();
            s.apply_run_upsert("solo", run("r1", State::Waiting, "2026-06-08T00:01:00Z"))
                .unwrap();
            s.apply_run_upsert("other", run("r2", State::Waiting, "2026-06-08T00:01:00Z"))
                .unwrap();
            s.apply_focus("solo").unwrap();
            s.apply_focus("other").unwrap();
            s.apply_solo("solo").unwrap();

            let evs = s.apply_session_remove("solo").unwrap();
            assert_eq!(
                evs.iter().map(Event::type_name).collect::<Vec<_>>(),
                vec!["session.removed", "session.updated"]
            );
        }

        let snap = StateStore::open(&path).unwrap().snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].session_id, "other");
        assert!(
            snap[0].unread,
            "removing a solo must re-arm remaining waiting sessions durably"
        );
    }

    // ---- crash-mid-write recovery (truncated / partial tail) ------------------

    #[test]
    fn crash_mid_write_partial_tail_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        {
            let mut s = StateStore::open(&path).unwrap();
            s.apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
                .unwrap();
            s.apply_run_upsert("s1", run("r1", State::Working, "2026-06-08T00:00:00Z"))
                .unwrap();
        }
        // Simulate a crash that left a garbage/truncated final row in the table
        // (e.g. a torn write a less-careful log might surface). We inject it
        // directly so replay's skip-on-parse-failure path is exercised.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "INSERT INTO events (payload) VALUES (?1)",
                ["{\"kind\":\"run.upsert\",\"session_id\":\"s1\",\"ru"], // truncated JSON
            )
            .unwrap();
        }
        // Reopen: the two intact rows restore; the torn tail is skipped, not fatal.
        let store = StateStore::open(&path).unwrap();
        let snap = store.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].runs.len(), 1, "intact prefix fully restored");

        // Verify the skip was counted (not silently lost).
        let mut engine = MergeEngine::new();
        let (applied, skipped) = store.log().replay_into(&mut engine).unwrap();
        assert_eq!(applied, 2);
        assert_eq!(skipped, 1);
    }

    #[test]
    fn unknown_future_kind_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        {
            let s = StateStore::open(&path).unwrap();
            // Append a well-formed row, then a row with an unknown future `kind`.
            s.log()
                .append(&PersistEvent::SessionUpsert {
                    session: Box::new(sess("s1", "2026-06-08T00:00:00Z")),
                })
                .unwrap();
        }
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "INSERT INTO events (payload) VALUES (?1)",
                ["{\"kind\":\"session.future_op\",\"session_id\":\"s9\"}"],
            )
            .unwrap();
        }
        let store = StateStore::open(&path).unwrap();
        assert_eq!(
            store.snapshot().len(),
            1,
            "unknown kind skipped, prefix kept"
        );
    }

    // ---- reap-after-grace timing (D17) ----------------------------------------

    #[test]
    fn reap_dead_only_after_grace() {
        let mut store = StateStore::open_in_memory().unwrap();
        store
            .apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
            .unwrap();
        // A run that went dead at 10:00.
        store
            .apply_run_upsert("s1", run("dead-old", State::Dead, "2026-06-08T10:00:00Z"))
            .unwrap();
        // A run that went dead at 11:30 (recent).
        store
            .apply_run_upsert("s1", run("dead-new", State::Dead, "2026-06-08T11:30:00Z"))
            .unwrap();
        // A live working run that must never be reaped.
        store
            .apply_run_upsert("s1", run("alive", State::Working, "2026-06-08T09:00:00Z"))
            .unwrap();

        // At 11:45 with a 1 h grace: cutoff = 10:45. Only `dead-old` (10:00) is
        // older than the cutoff; `dead-new` (11:30) and the live run survive.
        let evs = store
            .reap_dead("2026-06-08T11:45:00Z", DEFAULT_REAP_GRACE)
            .unwrap();
        // One run.removed + its session.updated.
        assert!(evs.iter().any(|e| e.type_name() == "run.removed"));
        let s1 = store.engine().session("s1").unwrap();
        let ids: Vec<_> = s1.runs.iter().map(|r| r.run_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["dead-new", "alive"],
            "only the over-grace dead run reaped"
        );
    }

    #[test]
    fn reap_just_before_and_after_boundary() {
        let mut store = StateStore::open_in_memory().unwrap();
        store
            .apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
            .unwrap();
        store
            .apply_run_upsert("s1", run("d", State::Dead, "2026-06-08T10:00:00Z"))
            .unwrap();
        let grace = Duration::from_secs(3600);

        // Exactly at grace boundary (now=11:00 → cutoff=10:00). updated==cutoff
        // is NOT strictly older, so it is NOT yet reaped.
        let evs = store.reap_dead("2026-06-08T11:00:00Z", grace).unwrap();
        assert!(evs.is_empty(), "not reaped at the exact boundary");
        assert_eq!(store.engine().session("s1").unwrap().runs.len(), 1);

        // One second past grace → reaped.
        let evs = store.reap_dead("2026-06-08T11:00:01Z", grace).unwrap();
        assert!(!evs.is_empty(), "reaped one second past grace");
        assert!(store.engine().session("s1").unwrap().runs.is_empty());
    }

    #[test]
    fn reaped_run_stays_reaped_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        {
            let mut store = StateStore::open(&path).unwrap();
            store
                .apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
                .unwrap();
            store
                .apply_run_upsert("s1", run("d", State::Dead, "2026-06-08T00:00:00Z"))
                .unwrap();
            // Reap well past grace.
            let evs = store
                .reap_dead("2026-06-08T12:00:00Z", DEFAULT_REAP_GRACE)
                .unwrap();
            assert!(!evs.is_empty());
        }
        // Restart: the reap was logged, so the run does not resurrect.
        let store = StateStore::open(&path).unwrap();
        assert!(store.engine().session("s1").unwrap().runs.is_empty());
    }

    // ---- session-expiry sweep (S6/S7 atomic entry+buffer drop) -----------------

    #[test]
    fn sweep_expires_stale_sessions_atomically() {
        let mut store = StateStore::open_in_memory().unwrap();
        // Stale session last touched at 00:00 with a run under it.
        store
            .apply_session_upsert(sess("stale", "2026-06-08T00:00:00Z"))
            .unwrap();
        store
            .apply_run_upsert("stale", run("r", State::Idle, "2026-06-08T00:00:00Z"))
            .unwrap();
        // Fresh session touched at 11:00.
        store
            .apply_session_upsert(sess("fresh", "2026-06-08T11:00:00Z"))
            .unwrap();

        // TTL 1h at now=12:00 → cutoff 11:00. `stale` (00:00) expires; `fresh`
        // (11:00 == cutoff, not strictly older) survives.
        let evs = store
            .sweep_expired_sessions("2026-06-08T12:00:00Z", Duration::from_secs(3600))
            .unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].type_name(), "session.removed");
        let snap = store.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].session_id, "fresh");
        // The whole entry (and its run) went together — nothing of `stale` left.
        assert!(store.engine().session("stale").is_none());
    }

    #[test]
    fn sweep_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        {
            let mut store = StateStore::open(&path).unwrap();
            store
                .apply_session_upsert(sess("stale", "2026-06-08T00:00:00Z"))
                .unwrap();
            store
                .sweep_expired_sessions("2026-06-08T12:00:00Z", Duration::from_secs(3600))
                .unwrap();
        }
        let store = StateStore::open(&path).unwrap();
        assert!(store.snapshot().is_empty(), "expired session stays gone");
    }

    #[test]
    fn malformed_updated_at_is_never_reaped() {
        let mut store = StateStore::open_in_memory().unwrap();
        store
            .apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
            .unwrap();
        // A dead run whose timestamp can't be parsed: conservative → never reaped.
        store
            .apply_run_upsert("s1", run("d", State::Dead, "whenever"))
            .unwrap();
        let evs = store
            .reap_dead("2026-06-08T23:59:59Z", DEFAULT_REAP_GRACE)
            .unwrap();
        assert!(evs.is_empty(), "unparseable updated_at must not be reaped");
        assert_eq!(store.engine().session("s1").unwrap().runs.len(), 1);
    }

    #[test]
    fn no_op_removes_do_not_grow_the_log() {
        let mut store = StateStore::open_in_memory().unwrap();
        store
            .apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
            .unwrap();
        let before = store.log().len().unwrap();
        // Removing an absent run / session / run on unknown session: all no-ops.
        assert!(store.apply_run_remove("s1", "ghost").unwrap().is_empty());
        assert!(store.apply_run_remove("ghost", "r").unwrap().is_empty());
        assert!(store.apply_session_remove("ghost").unwrap().is_empty());
        assert!(store
            .apply_run_upsert("ghost", run("r", State::Idle, "2026-06-08T00:00:00Z"))
            .unwrap()
            .is_empty());
        assert_eq!(
            store.log().len().unwrap(),
            before,
            "no-ops must not append rows"
        );
    }

    // ---- EventLog len / is_empty -----------------------------------------------

    #[test]
    fn log_len_and_is_empty_track_appends() {
        let log = EventLog::open_in_memory().unwrap();
        assert!(log.is_empty().unwrap(), "fresh log is empty");
        assert_eq!(log.len().unwrap(), 0);
        let seq = log
            .append(&PersistEvent::SessionRemove {
                session_id: "s1".into(),
            })
            .unwrap();
        assert_eq!(seq, 1, "first append gets seq 1");
        assert!(!log.is_empty().unwrap(), "log not empty after append");
        assert_eq!(log.len().unwrap(), 1);
    }

    // ---- durable-identity gating through the store (S6) ------------------------

    #[test]
    fn stamped_upsert_indexes_durable_then_drops_it_on_run_remove() {
        let mut store = StateStore::open_in_memory().unwrap();
        store
            .apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
            .unwrap();
        let did = DurableId::new("native"); // run() uses native_id "native"
        let (decision, evs) = store
            .apply_run_upsert_seq("s1", run("r1", State::Working, "t"), &did, 0, 1)
            .unwrap();
        assert_eq!(decision, Decision::ApplyFresh);
        assert!(!evs.is_empty(), "fresh stamped delta is applied");
        assert!(
            store.reclaim().contains(&did),
            "applied stamped delta records a reclaim mark"
        );
        assert_eq!(store.durables_of("s1"), vec![did.clone()]);

        // Removing the run drops its reclaim mark and prunes the index.
        store.apply_run_remove("s1", "r1").unwrap();
        assert!(
            !store.reclaim().contains(&did),
            "run remove drops the durable's reclaim mark"
        );
        assert!(
            store.durables_of("s1").is_empty(),
            "run remove prunes the session->durable index"
        );
    }

    /// Run `f` with a process-local TRACE-level tracing subscriber installed, so
    /// the `tracing::debug!` diagnostic-formatting regions on the gated-drop and
    /// atomic-reclaim-drop paths actually execute (they are compiled-in but
    /// level-gated; at the default level the format closure is skipped).
    fn with_trace<T>(f: impl FnOnce() -> T) -> T {
        use tracing::level_filters::LevelFilter;
        let sub = tracing_subscriber::fmt()
            .with_max_level(LevelFilter::TRACE)
            .with_test_writer()
            .finish();
        tracing::subscriber::with_default(sub, f)
    }

    /// Build an on-disk store, populate it via `setup`, then reopen it with a
    /// READ-ONLY log so every subsequent `log.append` deterministically fails —
    /// the root-safe way to drive the persist-failure error arms (no chmod). The
    /// projection is fully restored, so mutations still change in-memory state;
    /// only the durable append fails.
    fn read_only_store_after(
        setup: impl FnOnce(&mut StateStore),
    ) -> (tempfile::TempDir, StateStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.db");
        {
            let mut store = StateStore::open(&path).unwrap();
            setup(&mut store);
        }
        let store = StateStore::open_read_only_for_test(&path).unwrap();
        (dir, store)
    }

    #[test]
    fn flag_change_append_failure_rolls_back_and_surfaces_error() {
        // T1.3 regression: mute/unmute/solo must be append-FIRST — if the durable
        // log write fails, the change must NOT be retained in memory (no silent
        // divergence between memory and the log). Previously the append error was
        // swallowed with `tracing::error!` and the in-memory mute survived,
        // vanishing on the next restart.
        with_trace(|| {
            // mute: an un-muted session; the append fails → Err surfaced, mute
            // rolled back (memory stays un-muted, matching the empty log).
            let (_d1, mut store) = read_only_store_after(|s| {
                let mut sess = sess("s1", "2026-06-08T00:00:00Z");
                sess.runs
                    .push(run("r1", State::Waiting, "2026-06-08T00:00:00Z"));
                s.apply_session_upsert(sess).unwrap();
            });
            assert!(matches!(
                store.apply_mute("s1").unwrap_err(),
                PersistError::Sqlite(_)
            ));
            assert!(
                !store.engine().session("s1").unwrap().muted,
                "a mute whose durable append failed must be rolled back, not retained"
            );

            // unmute: a durably-muted session (restored muted); the unmute append
            // fails → Err surfaced, unmute rolled back (memory stays muted).
            let (_d2, mut store) = read_only_store_after(|s| {
                s.apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
                    .unwrap();
                s.apply_mute("s1").unwrap();
            });
            assert!(
                store.engine().session("s1").unwrap().muted,
                "restored muted"
            );
            assert!(matches!(
                store.apply_unmute("s1").unwrap_err(),
                PersistError::Sqlite(_)
            ));
            assert!(
                store.engine().session("s1").unwrap().muted,
                "an unmute whose durable append failed must be rolled back (stays muted)"
            );

            // solo: an un-soloed session; the append fails → Err surfaced, solo
            // rolled back (memory stays un-soloed).
            let (_d3, mut store) = read_only_store_after(|s| {
                s.apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
                    .unwrap();
            });
            assert!(matches!(
                store.apply_solo("s1").unwrap_err(),
                PersistError::Sqlite(_)
            ));
            assert!(
                !store.engine().session("s1").unwrap().soloed,
                "a solo whose durable append failed must be rolled back"
            );
        });
    }

    #[test]
    fn append_failure_in_focus_is_logged_not_panicked() {
        // Focus's contract (unlike the flag ops) is to still acknowledge the ping
        // in memory even if the durable append fails — it must log, not panic, and
        // return the event. Drives `persist_session_snapshot`'s error arm.
        with_trace(|| {
            let (_dir, mut store) = read_only_store_after(|s| {
                let mut sess = sess("s1", "2026-06-08T00:00:00Z");
                sess.runs
                    .push(run("r1", State::Waiting, "2026-06-08T00:00:00Z"));
                s.apply_session_upsert(sess).unwrap();
            });

            let focus = store
                .apply_focus("s1")
                .expect("focus still returns an event");
            assert_eq!(focus.type_name(), "session.updated");
            assert!(
                !store.engine().session("s1").unwrap().unread,
                "focus clears unread in memory even when the durable append fails"
            );
        });
    }

    #[test]
    fn append_failure_in_session_remove_returns_sqlite_error() {
        let (_dir, mut store) = read_only_store_after(|s| {
            s.apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
                .unwrap();
        });
        // The remove path uses `?` on append, so a read-only log surfaces the
        // error to the caller (the server then logs + drops it).
        let err = store.apply_session_remove("s1").unwrap_err();
        assert!(matches!(err, PersistError::Sqlite(_)));
    }

    #[test]
    fn append_failure_in_run_upsert_and_remove_returns_sqlite_error() {
        let (_dir, mut store) = read_only_store_after(|s| {
            let mut sess = sess("s1", "2026-06-08T00:00:00Z");
            sess.runs
                .push(run("r1", State::Working, "2026-06-08T00:00:00Z"));
            s.apply_session_upsert(sess).unwrap();
        });
        // Upsert of a NEW run on a known session appends → fails read-only.
        let err = store
            .apply_run_upsert("s1", run("r2", State::Idle, "2026-06-08T00:00:00Z"))
            .unwrap_err();
        assert!(matches!(err, PersistError::Sqlite(_)));
        // Remove of an existing run appends a run.remove → fails read-only.
        let err = store.apply_run_remove("s1", "r1").unwrap_err();
        assert!(matches!(err, PersistError::Sqlite(_)));
    }

    #[test]
    fn append_failure_in_session_upsert_returns_sqlite_error() {
        let (_dir, mut store) = read_only_store_after(|s| {
            s.apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
                .unwrap();
        });
        let err = store
            .apply_session_upsert(sess("s2", "2026-06-08T00:00:00Z"))
            .unwrap_err();
        assert!(matches!(err, PersistError::Sqlite(_)));
    }

    #[test]
    fn open_on_enotdir_path_is_sqlite_error() {
        // A path whose parent component is a regular file makes Connection::open
        // fail (ENOTDIR) — the open-time PersistError branch, root-safe.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        let bad = file.join("sub").join("events.db");
        assert!(
            matches!(StateStore::open(&bad), Err(PersistError::Sqlite(_))),
            "ENOTDIR open must surface a sqlite error"
        );
    }

    #[test]
    fn gated_drop_and_atomic_reclaim_drop_emit_diagnostics() {
        with_trace(|| {
            let mut store = StateStore::open_in_memory().unwrap();
            store
                .apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
                .unwrap();
            let did = DurableId::new("native");
            store
                .apply_run_upsert_seq("s1", run("r1", State::Working, "t"), &did, 0, 5)
                .unwrap();
            // A duplicate triggers the gated-out debug diagnostic (line ~351).
            let (decision, _) = store
                .apply_run_upsert_seq("s1", run("r1", State::Idle, "t"), &did, 0, 5)
                .unwrap();
            assert_eq!(decision, Decision::DuplicateDrop);
            // Removing the session triggers the atomic-reclaim-drop diagnostic.
            assert!(store.reclaim().contains(&did));
            store.apply_session_remove("s1").unwrap();
            assert!(!store.reclaim().contains(&did));
        });
    }

    #[test]
    fn stamped_duplicate_is_gated_out_and_not_logged() {
        let mut store = StateStore::open_in_memory().unwrap();
        store
            .apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
            .unwrap();
        let did = DurableId::new("native");
        store
            .apply_run_upsert_seq("s1", run("r1", State::Working, "t"), &did, 0, 5)
            .unwrap();
        let len_after_first = store.log().len().unwrap();

        // A duplicate (seq <= high-water) is dropped: no events, no new log row.
        let (decision, evs) = store
            .apply_run_upsert_seq("s1", run("r1", State::Idle, "t"), &did, 0, 5)
            .unwrap();
        assert_eq!(decision, Decision::DuplicateDrop);
        assert!(evs.is_empty(), "gated-out delta broadcasts nothing");
        assert_eq!(
            store.log().len().unwrap(),
            len_after_first,
            "gated-out delta must not grow the log"
        );
    }

    #[test]
    fn stamped_upsert_on_unknown_session_drops_without_admitting_seq() {
        let mut store = StateStore::open_in_memory().unwrap();
        let did = DurableId::new("native");
        let (decision, evs) = store
            .apply_run_upsert_seq("ghost", run("r1", State::Working, "t"), &did, 0, 1)
            .unwrap();
        assert_eq!(decision, Decision::DuplicateDrop);
        assert!(evs.is_empty());
        assert!(
            !store.reclaim().contains(&did),
            "unknown session must not advance the dedup seq"
        );
    }

    #[test]
    fn dropping_session_with_stamped_runs_drops_reclaim_marks_atomically() {
        let mut store = StateStore::open_in_memory().unwrap();
        store
            .apply_session_upsert(sess("s1", "2026-06-08T00:00:00Z"))
            .unwrap();
        let did = DurableId::new("native");
        store
            .apply_run_upsert_seq("s1", run("r1", State::Working, "t"), &did, 0, 1)
            .unwrap();
        assert!(store.reclaim().contains(&did));

        // Removing the session drops the reclaim bookkeeping for all its runs.
        let evs = store.apply_session_remove("s1").unwrap();
        assert!(!evs.is_empty(), "session removal broadcasts");
        assert!(
            !store.reclaim().contains(&did),
            "session removal drops its runs' reclaim marks (invariant 3)"
        );
        assert!(store.durables_of("s1").is_empty());
    }

    // ---- subtract / now_iso edge cases -----------------------------------------

    #[test]
    fn subtract_on_unparseable_now_returns_input_unchanged() {
        // A malformed clock makes the cutoff equal to `now` → never over-reaps.
        assert_eq!(
            subtract("not-a-time", Duration::from_secs(3600)),
            "not-a-time"
        );
    }

    #[test]
    fn now_iso_round_trips_through_parse() {
        let s = now_iso();
        let secs = parse_iso(&s).expect("now_iso emits a parseable ISO instant");
        assert_eq!(format_iso(secs), s, "now_iso output is canonical");
        assert!(secs > 0, "wall clock is after the epoch");
    }

    // ---- parse_iso structural rejections ---------------------------------------

    #[test]
    fn parse_iso_rejects_each_structural_deviation() {
        // Too short.
        assert_eq!(parse_iso("2026-06-08T00:00"), None);
        // Wrong date separators (positions 4/7/10).
        assert_eq!(parse_iso("2026.06-08T00:00:00Z"), None);
        assert_eq!(parse_iso("2026-06.08T00:00:00Z"), None);
        assert_eq!(parse_iso("2026-06-08X00:00:00Z"), None);
        // Wrong time separators (positions 13/16).
        assert_eq!(parse_iso("2026-06-08T00.00:00Z"), None);
        assert_eq!(parse_iso("2026-06-08T00:00.00Z"), None);
        // Out-of-range month / day.
        assert_eq!(parse_iso("2026-13-08T00:00:00Z"), None);
        assert_eq!(parse_iso("2026-06-32T00:00:00Z"), None);
        // Out-of-range hour / minute / second.
        assert_eq!(parse_iso("2026-06-08T24:00:00Z"), None);
        assert_eq!(parse_iso("2026-06-08T00:60:00Z"), None);
        assert_eq!(parse_iso("2026-06-08T00:00:61Z"), None);
        // A valid leap-second-ish second (60) is accepted; a normal one too.
        assert!(parse_iso("2026-06-08T00:00:60Z").is_some());
        assert!(parse_iso("2026-06-08T23:59:59Z").is_some());
    }
}
