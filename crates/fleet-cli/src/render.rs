//! Pure-function render/reducer for `fleet ls`.
//!
//! This module contains NO I/O, NO async, NO network — only deterministic
//! transformations of protocol objects into renderable rows. This makes it
//! straightforward to unit-test the full snapshot→delta→display pipeline without
//! a live Hub. The test criterion ("CLI render-snapshot") is exercised here.
//!
//! **Reducer contract:**
//! - [`State`] is the mutable in-memory view owned by the CLI face.
//! - [`State::apply`] accepts a single [`Event`] and mutates the view in-place.
//! - [`State::rows`] returns a stable-sorted list of [`Row`]s (the text the
//!   terminal displays). Sorting: muted sessions last, then by rollup urgency
//!   descending (approval > question > idle-done > none), then by rollup state
//!   (waiting > error > working > done > idle > dead), then by session_id
//!   lexicographically for determinism.
//!
//! **Events handled** (full protocol surface for S3):
//! - `fleet.snapshot` — replace entire view
//! - `session.added` / `session.updated` — upsert session (keyed by session_id)
//! - `session.removed` — drop session
//! - `run.added` / `run.updated` — upsert run within a session
//! - `run.removed` — drop run from a session
//!
//! After every run upsert/remove the session's `rollup_state` and
//! `rollup_urgency` are recomputed from the updated runs list using
//! [`fleet_protocol::rollup`]. The Hub also recomputes these on its side, but
//! the CLI recomputes locally so rows stay correct even if the Hub-computed
//! rollup in a `session.updated` event lags by one delta.

use fleet_protocol::{
    rollup::{rollup_state, rollup_urgency},
    AgentRun, Confidence, Event, Session, State as RunState, Urgency,
};
use std::collections::HashMap;

// ── Row (the rendered line the terminal shows) ────────────────────────────────

/// A single rendered line in `fleet ls`.
///
/// Each row corresponds to one [`Session`]. The `runs` sub-rows are embedded
/// below it (they share the session's row group but do not become their own top-
/// level rows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Stable identifier — same as [`Session::session_id`].
    pub session_id: String,
    /// Display title.
    pub title: String,
    /// Rolled-up state across runs (most urgent).
    pub rollup_state: RunState,
    /// Rolled-up urgency across runs (most urgent); `None` = no urgency.
    pub rollup_urgency: Option<Urgency>,
    /// Whether the session is muted.
    pub muted: bool,
    /// Whether the session has an unread notification.
    pub unread: bool,
    /// Inline summary per run.
    pub runs: Vec<RunRow>,
}

/// Recompute a session's rolled-up state + urgency from its current `runs` list.
/// `Urgency::None` normalizes to absent (`None`), matching the wire contract.
fn recompute_rollups(sess: &mut Session) {
    sess.rollup_state = rollup_state(&sess.runs).unwrap_or(RunState::Idle);
    sess.rollup_urgency = rollup_urgency(&sess.runs).and_then(|u| {
        if u == Urgency::None {
            None
        } else {
            Some(u)
        }
    });
}

/// A rendered per-run sub-row embedded within a [`Row`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub run_id: String,
    pub agent_kind: fleet_protocol::AgentKind,
    pub state: RunState,
    pub confidence: Confidence,
    pub cwd: String,
    pub last_message: Option<String>,
    pub urgency: Option<Urgency>,
    pub waiting_since: Option<String>,
}

impl RunRow {
    pub fn from_run(r: &AgentRun) -> Self {
        RunRow {
            run_id: r.run_id.clone(),
            agent_kind: r.agent_kind.clone(),
            state: r.state,
            confidence: r.confidence,
            cwd: r.cwd.clone(),
            last_message: r.last_message.clone(),
            urgency: r.urgency,
            waiting_since: r.waiting_since.clone(),
        }
    }
}

impl Row {
    /// One-line text representation used in unit tests and terminal output.
    ///
    /// Format: `[ROLLUP_STATE] title  (N run(s))  [urgency]  [muted]`
    pub fn render_line(&self) -> String {
        let state_str = state_label(self.rollup_state);
        let urgency_str = self.rollup_urgency.map(urgency_label).unwrap_or_default();
        let muted = if self.muted { "  [muted]" } else { "" };
        let unread = if self.unread { " *" } else { "" };
        let n = self.runs.len();
        let runs_str = match n {
            0 => "  (no runs)".to_string(),
            1 => "  (1 run)".to_string(),
            _ => format!("  ({n} runs)"),
        };
        format!(
            "[{state_str}]{unread} {title}{runs_str}{urgency_str}{muted}",
            title = self.title,
        )
    }
}

fn state_label(s: RunState) -> &'static str {
    match s {
        RunState::Working => "working",
        RunState::Waiting => "waiting",
        RunState::Idle => "idle",
        RunState::Done => "done",
        RunState::Error => "error",
        RunState::Dead => "dead",
    }
}

fn urgency_label(u: Urgency) -> &'static str {
    match u {
        Urgency::Approval => "  [approval]",
        Urgency::Question => "  [question]",
        Urgency::IdleDone => "  [idle-done]",
        Urgency::None => "",
    }
}

// ── Sort order ────────────────────────────────────────────────────────────────

/// Numeric sort key — lower value is displayed higher (more urgent first).
///
/// Primary: muted last. Secondary: urgency descending. Tertiary: state
/// descending. Quaternary: session_id ascending (deterministic tiebreak).
fn sort_key(row: &Row) -> (u8, u8, u8) {
    let muted_k: u8 = if row.muted { 1 } else { 0 };
    let urgency_k: u8 = 255
        - match row.rollup_urgency {
            Some(Urgency::Approval) => 3,
            Some(Urgency::Question) => 2,
            Some(Urgency::IdleDone) => 1,
            Some(Urgency::None) | None => 0,
        };
    let state_k: u8 = 255
        - match row.rollup_state {
            RunState::Waiting => 5,
            RunState::Error => 4,
            RunState::Working => 3,
            RunState::Done => 2,
            RunState::Idle => 1,
            RunState::Dead => 0,
        };
    (muted_k, urgency_k, state_k)
}

// ── Reducer state ─────────────────────────────────────────────────────────────

/// The CLI's in-memory view of Hub state. Apply events with [`Self::apply`],
/// read the rendered list with [`Self::rows`].
#[derive(Debug, Default, Clone)]
pub struct CliState {
    /// Keyed by `session_id`.
    sessions: HashMap<String, Session>,
}

impl CliState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one event from the Hub, mutating the local view.
    ///
    /// Unknown event types that survive the protocol layer are unreachable here
    /// because [`Event`] is an exhaustive enum — serde would have rejected them
    /// first. Any session-level event for a session not yet in the map is a
    /// harmless no-op for the `*updated`/`*removed` variants (the snapshot will
    /// provide the canonical truth on re-subscribe).
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Snapshot { sessions, .. } => {
                self.sessions.clear();
                for s in sessions {
                    self.sessions.insert(s.session_id.clone(), s);
                }
            }

            Event::SessionAdded { session, .. } | Event::SessionUpdated { session, .. } => {
                self.sessions.insert(session.session_id.clone(), session);
            }

            Event::SessionRemoved { session_id, .. } => {
                self.sessions.remove(&session_id);
            }

            Event::RunAdded {
                session_id, run, ..
            }
            | Event::RunUpdated {
                session_id, run, ..
            } => {
                if let Some(sess) = self.sessions.get_mut(&session_id) {
                    // Upsert the run (keyed by run_id).
                    if let Some(pos) = sess.runs.iter().position(|r| r.run_id == run.run_id) {
                        sess.runs[pos] = run;
                    } else {
                        sess.runs.push(run);
                    }
                    // Recompute rollups from the local runs list.
                    recompute_rollups(sess);
                }
                // If session_id is unknown we drop the delta (harmless — a
                // session.added event will arrive from a fresh snapshot).
            }

            Event::RunRemoved {
                session_id, run_id, ..
            } => {
                if let Some(sess) = self.sessions.get_mut(&session_id) {
                    sess.runs.retain(|r| r.run_id != run_id);
                    // Recompute rollups.
                    recompute_rollups(sess);
                }
            }
        }
    }

    /// Apply a sequence of events, returning `self` for chaining in tests.
    // Used in tests; main.rs drives events one-by-one through `apply`.
    #[allow(dead_code)]
    pub fn apply_all(&mut self, events: impl IntoIterator<Item = Event>) -> &mut Self {
        for ev in events {
            self.apply(ev);
        }
        self
    }

    /// Produce the sorted, renderable list of rows.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows: Vec<Row> = self.sessions.values().map(session_to_row).collect();
        // Stable sort: primary by (muted, urgency_desc, state_desc), tiebreak by
        // session_id ascending.
        rows.sort_by(|a, b| {
            sort_key(a)
                .cmp(&sort_key(b))
                .then_with(|| a.session_id.cmp(&b.session_id))
        });
        rows
    }

    /// Number of sessions currently tracked.
    // Used in tests; callers needing the count can use `rows().len()`.
    #[allow(dead_code)]
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Whether the state is empty (no sessions).
    // Used in tests and connection warm-up path.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

fn session_to_row(s: &Session) -> Row {
    Row {
        session_id: s.session_id.clone(),
        title: s.title.clone(),
        rollup_state: s.rollup_state,
        rollup_urgency: s.rollup_urgency,
        muted: s.muted,
        unread: s.unread,
        runs: s.runs.iter().map(RunRow::from_run).collect(),
    }
}

// ── Display helpers (used by main.rs) ─────────────────────────────────────────

/// Format one run sub-row for terminal output.
pub fn format_run_row(r: &RunRow, indent: &str) -> String {
    let kind = match r.agent_kind {
        fleet_protocol::AgentKind::ClaudeCode => "claude",
        fleet_protocol::AgentKind::Codex => "codex",
        fleet_protocol::AgentKind::Other => "other",
    };
    let conf = match r.confidence {
        Confidence::High => "high",
        Confidence::Inferred => "inferred",
    };
    let state = state_label(r.state);
    let waiting = r
        .waiting_since
        .as_deref()
        .map(|w| format!("  waiting since {w}"))
        .unwrap_or_default();
    let msg = r
        .last_message
        .as_deref()
        .map(|m| format!("  \"{m}\""))
        .unwrap_or_default();
    format!(
        "{indent}  [{kind}] {state} (conf: {conf})  {cwd}{waiting}{msg}",
        cwd = r.cwd
    )
}

// ── Unit tests (heavy, pure-function, no I/O) ─────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::{
        AgentKind, AgentRun, Confidence, Event, Extra, Location, LocationGlyph, LocationKind,
        Server, ServerKind, Session, State as RunState, Urgency, SCHEMA_VERSION,
    };

    // ── Helpers ────────────────────────────────────────────────────────────────

    fn loc() -> Location {
        Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
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

    fn session(id: &str, title: &str, state: RunState) -> Session {
        Session::new(id, title, loc(), srv(), state, "2026-06-08T00:00:00Z")
    }

    fn run(run_id: &str, state: RunState, urgency: Option<Urgency>) -> AgentRun {
        let mut r = AgentRun::new(
            run_id,
            AgentKind::ClaudeCode,
            "native-1",
            "/home/user/project",
            state,
            Confidence::High,
            "2026-06-08T00:00:00Z",
        );
        r.urgency = urgency;
        r
    }

    fn run_codex(run_id: &str, state: RunState) -> AgentRun {
        AgentRun::new(
            run_id,
            AgentKind::Codex,
            "thread-1",
            "/tmp",
            state,
            Confidence::Inferred,
            "2026-06-08T00:00:00Z",
        )
    }

    // ── snapshot tests ─────────────────────────────────────────────────────────

    #[test]
    fn empty_snapshot_gives_empty_rows() {
        let mut st = CliState::new();
        st.apply(Event::snapshot(vec![]));
        assert!(st.is_empty());
        assert_eq!(st.rows(), vec![]);
    }

    #[test]
    fn snapshot_with_sessions_populates_rows() {
        let mut st = CliState::new();
        let s1 = session("s1", "project-a", RunState::Idle);
        let s2 = session("s2", "project-b", RunState::Working);
        st.apply(Event::snapshot(vec![s1, s2]));
        assert_eq!(st.session_count(), 2);
        let rows = st.rows();
        // working sorts before idle
        assert_eq!(rows[0].session_id, "s2");
        assert_eq!(rows[1].session_id, "s1");
    }

    #[test]
    fn second_snapshot_replaces_first() {
        let mut st = CliState::new();
        st.apply(Event::snapshot(vec![session("s1", "old", RunState::Idle)]));
        assert_eq!(st.session_count(), 1);
        st.apply(Event::snapshot(vec![
            session("s2", "new-a", RunState::Idle),
            session("s3", "new-b", RunState::Idle),
        ]));
        assert_eq!(st.session_count(), 2);
        let ids: Vec<_> = st.rows().iter().map(|r| r.session_id.clone()).collect();
        assert!(!ids.contains(&"s1".to_string()));
        assert!(ids.contains(&"s2".to_string()));
        assert!(ids.contains(&"s3".to_string()));
    }

    // ── session.added / session.updated ───────────────────────────────────────

    #[test]
    fn session_added_appears_in_rows() {
        let mut st = CliState::new();
        st.apply(Event::snapshot(vec![]));
        st.apply(Event::session_added(session(
            "s1",
            "alpha",
            RunState::Working,
        )));
        assert_eq!(st.session_count(), 1);
        assert_eq!(st.rows()[0].title, "alpha");
    }

    #[test]
    fn session_updated_replaces_old_title() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session(
            "s1",
            "old-title",
            RunState::Idle,
        )));
        let mut updated = session("s1", "new-title", RunState::Done);
        updated.session_id = "s1".into();
        st.apply(Event::session_updated(updated));
        let rows = st.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "new-title");
        assert_eq!(rows[0].rollup_state, RunState::Done);
    }

    // ── session.removed ───────────────────────────────────────────────────────

    #[test]
    fn session_removed_drops_it_from_rows() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "a", RunState::Idle)));
        st.apply(Event::session_added(session("s2", "b", RunState::Idle)));
        st.apply(Event::session_removed("s1"));
        assert_eq!(st.session_count(), 1);
        assert_eq!(st.rows()[0].session_id, "s2");
    }

    #[test]
    fn removing_unknown_session_is_noop() {
        let mut st = CliState::new();
        st.apply(Event::snapshot(vec![]));
        // Should not panic.
        st.apply(Event::session_removed("nonexistent"));
        assert!(st.is_empty());
    }

    // ── run.added / run.updated ───────────────────────────────────────────────

    #[test]
    fn run_added_appears_in_sub_rows() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        st.apply(Event::run_added("s1", run("r1", RunState::Working, None)));
        let rows = st.rows();
        assert_eq!(rows[0].runs.len(), 1);
        assert_eq!(rows[0].runs[0].run_id, "r1");
    }

    #[test]
    fn run_added_updates_rollup_state() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        assert_eq!(st.rows()[0].rollup_state, RunState::Idle);

        st.apply(Event::run_added("s1", run("r1", RunState::Working, None)));
        assert_eq!(st.rows()[0].rollup_state, RunState::Working);

        // Adding a waiting run should promote rollup to waiting.
        st.apply(Event::run_added(
            "s1",
            run("r2", RunState::Waiting, Some(Urgency::Approval)),
        ));
        assert_eq!(st.rows()[0].rollup_state, RunState::Waiting);
        assert_eq!(st.rows()[0].rollup_urgency, Some(Urgency::Approval));
    }

    #[test]
    fn run_updated_replaces_old_run() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        st.apply(Event::run_added("s1", run("r1", RunState::Working, None)));

        let mut updated = run("r1", RunState::Done, None);
        updated.last_message = Some("task complete".into());
        st.apply(Event::run_updated("s1", updated));

        let rows = st.rows();
        assert_eq!(rows[0].runs.len(), 1, "upsert must not duplicate");
        assert_eq!(rows[0].runs[0].state, RunState::Done);
        assert_eq!(
            rows[0].runs[0].last_message.as_deref(),
            Some("task complete")
        );
        assert_eq!(rows[0].rollup_state, RunState::Done);
    }

    #[test]
    fn run_update_for_unknown_session_is_noop() {
        let mut st = CliState::new();
        // Should not panic.
        st.apply(Event::run_updated(
            "no-such-session",
            run("r1", RunState::Working, None),
        ));
        assert!(st.is_empty());
    }

    // ── run.removed ───────────────────────────────────────────────────────────

    #[test]
    fn run_removed_drops_sub_row_and_recomputes_rollup() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        st.apply(Event::run_added("s1", run("r1", RunState::Working, None)));
        st.apply(Event::run_added(
            "s1",
            run("r2", RunState::Waiting, Some(Urgency::Approval)),
        ));
        // Both runs present; rollup = waiting.
        assert_eq!(st.rows()[0].rollup_state, RunState::Waiting);

        // Remove the waiting run; rollup should fall back to working.
        st.apply(Event::run_removed("s1", "r2"));
        let rows = st.rows();
        assert_eq!(rows[0].runs.len(), 1);
        assert_eq!(rows[0].rollup_state, RunState::Working);
        assert_eq!(rows[0].rollup_urgency, None);
    }

    #[test]
    fn run_removed_last_run_gives_idle_rollup() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        st.apply(Event::run_added("s1", run("r1", RunState::Working, None)));
        st.apply(Event::run_removed("s1", "r1"));
        let rows = st.rows();
        assert!(rows[0].runs.is_empty());
        // Default when no runs: idle.
        assert_eq!(rows[0].rollup_state, RunState::Idle);
    }

    #[test]
    fn removing_unknown_run_is_noop() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        st.apply(Event::run_removed("s1", "no-such-run"));
        assert_eq!(st.rows()[0].runs.len(), 0);
    }

    #[test]
    fn run_removed_for_unknown_session_is_noop() {
        // run.removed for a session NOT in the map hits the `if let Some(sess)`
        // None branch of the RunRemoved arm (a harmless drop — the snapshot is
        // canonical). Distinct from removing an unknown run from a KNOWN session.
        let mut st = CliState::new();
        st.apply(Event::snapshot(vec![]));
        st.apply(Event::run_removed("no-such-session", "r1"));
        assert!(st.is_empty());
    }

    // ── Sequence / lifecycle tests ────────────────────────────────────────────

    #[test]
    fn working_to_waiting_to_working_to_dead_lifecycle() {
        let mut st = CliState::new();
        st.apply(Event::snapshot(vec![]));
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        st.apply(Event::run_added("s1", run("r1", RunState::Working, None)));

        assert_eq!(st.rows()[0].rollup_state, RunState::Working);

        // Transition → waiting with approval urgency.
        let mut r = run("r1", RunState::Waiting, Some(Urgency::Approval));
        r.waiting_since = Some("2026-06-08T10:00:00Z".into());
        st.apply(Event::run_updated("s1", r));
        assert_eq!(st.rows()[0].rollup_state, RunState::Waiting);
        assert_eq!(st.rows()[0].rollup_urgency, Some(Urgency::Approval));
        assert_eq!(
            st.rows()[0].runs[0].waiting_since.as_deref(),
            Some("2026-06-08T10:00:00Z")
        );

        // Transition → back to working (approval resolved).
        st.apply(Event::run_updated("s1", run("r1", RunState::Working, None)));
        assert_eq!(st.rows()[0].rollup_state, RunState::Working);
        assert_eq!(st.rows()[0].rollup_urgency, None);

        // Session removed (agent dead).
        st.apply(Event::session_removed("s1"));
        assert!(st.is_empty());
    }

    #[test]
    fn full_snapshot_then_delta_sequence() {
        // Simulate: get snapshot with one session, then receive two more deltas.
        let mut st = CliState::new();

        let mut s1 = session("s1", "web", RunState::Idle);
        s1.runs.push(run("r1", RunState::Working, None));
        // The Hub's snapshot already computes rollup_state = Working for s1.
        s1.rollup_state = RunState::Working;

        st.apply(Event::snapshot(vec![s1]));

        // A second session arrives via delta.
        st.apply(Event::session_added(session(
            "s2",
            "backend",
            RunState::Idle,
        )));

        // A run is added to s2.
        st.apply(Event::run_added(
            "s2",
            run("r2", RunState::Waiting, Some(Urgency::Question)),
        ));

        let rows = st.rows();
        // s2 is waiting (more urgent), sorts first.
        assert_eq!(rows[0].session_id, "s2");
        assert_eq!(rows[0].rollup_state, RunState::Waiting);
        assert_eq!(rows[0].rollup_urgency, Some(Urgency::Question));

        assert_eq!(rows[1].session_id, "s1");
        assert_eq!(rows[1].rollup_state, RunState::Working);
    }

    // ── Sort order tests ──────────────────────────────────────────────────────

    #[test]
    fn waiting_sorts_before_working() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session(
            "s-working",
            "W",
            RunState::Working,
        )));
        st.apply(Event::session_added(session(
            "s-waiting",
            "Wt",
            RunState::Waiting,
        )));
        let rows = st.rows();
        assert_eq!(rows[0].session_id, "s-waiting");
        assert_eq!(rows[1].session_id, "s-working");
    }

    #[test]
    fn approval_urgency_sorts_before_question() {
        let mut st = CliState::new();
        let mut s_q = session("s-q", "question", RunState::Waiting);
        s_q.rollup_urgency = Some(Urgency::Question);
        let mut s_a = session("s-a", "approval", RunState::Waiting);
        s_a.rollup_urgency = Some(Urgency::Approval);
        st.apply(Event::session_added(s_q));
        st.apply(Event::session_added(s_a));
        let rows = st.rows();
        // approval is most urgent → first row.
        assert_eq!(rows[0].rollup_urgency, Some(Urgency::Approval));
        assert_eq!(rows[1].rollup_urgency, Some(Urgency::Question));
    }

    #[test]
    fn muted_session_sorts_last() {
        let mut st = CliState::new();
        let mut s_muted = session("s-muted", "muted", RunState::Waiting);
        s_muted.muted = true;
        let s_live = session("s-live", "live", RunState::Idle);
        st.apply(Event::session_added(s_muted));
        st.apply(Event::session_added(s_live));
        let rows = st.rows();
        assert_eq!(rows[0].session_id, "s-live");
        assert_eq!(rows[1].session_id, "s-muted");
        assert!(rows[1].muted);
    }

    #[test]
    fn tiebreak_by_session_id_is_deterministic() {
        let mut st = CliState::new();
        // All sessions in identical state — must sort by session_id.
        for id in ["s-c", "s-a", "s-b"] {
            st.apply(Event::session_added(session(id, id, RunState::Idle)));
        }
        let rows = st.rows();
        let ids: Vec<_> = rows.iter().map(|r| r.session_id.as_str()).collect();
        assert_eq!(ids, vec!["s-a", "s-b", "s-c"]);
    }

    #[test]
    fn done_distinct_from_idle_d9_honored() {
        let mut st = CliState::new();
        let s_idle = session("s-idle", "idle-sess", RunState::Idle);
        let s_done = session("s-done", "done-sess", RunState::Done);
        st.apply(Event::session_added(s_idle));
        st.apply(Event::session_added(s_done));
        let rows = st.rows();
        // Done ranks above idle.
        assert_eq!(rows[0].rollup_state, RunState::Done);
        assert_eq!(rows[1].rollup_state, RunState::Idle);
        // Wire tokens must differ (D9).
        let done_token = serde_json::to_value(RunState::Done).unwrap();
        let idle_token = serde_json::to_value(RunState::Idle).unwrap();
        assert_ne!(done_token, idle_token);
    }

    // ── render_line ───────────────────────────────────────────────────────────

    #[test]
    fn render_line_idle_no_runs() {
        let row = Row {
            session_id: "s1".into(),
            title: "my project".into(),
            rollup_state: RunState::Idle,
            rollup_urgency: None,
            muted: false,
            unread: false,
            runs: vec![],
        };
        assert_eq!(row.render_line(), "[idle] my project  (no runs)");
    }

    #[test]
    fn render_line_waiting_with_approval_muted() {
        let row = Row {
            session_id: "s1".into(),
            title: "repo".into(),
            rollup_state: RunState::Waiting,
            rollup_urgency: Some(Urgency::Approval),
            muted: true,
            unread: true,
            runs: vec![RunRow {
                run_id: "r1".into(),
                agent_kind: AgentKind::ClaudeCode,
                state: RunState::Waiting,
                confidence: Confidence::High,
                cwd: "/home".into(),
                last_message: None,
                urgency: Some(Urgency::Approval),
                waiting_since: None,
            }],
        };
        let line = row.render_line();
        assert!(line.contains("[waiting]"));
        assert!(line.contains("[approval]"));
        assert!(line.contains("[muted]"));
        assert!(line.contains('*'), "unread marker should be present");
    }

    #[test]
    fn render_line_two_runs_plural() {
        let row = Row {
            session_id: "s1".into(),
            title: "proj".into(),
            rollup_state: RunState::Working,
            rollup_urgency: None,
            muted: false,
            unread: false,
            runs: vec![
                RunRow {
                    run_id: "r1".into(),
                    agent_kind: AgentKind::Codex,
                    state: RunState::Working,
                    confidence: Confidence::Inferred,
                    cwd: "/a".into(),
                    last_message: None,
                    urgency: None,
                    waiting_since: None,
                },
                RunRow {
                    run_id: "r2".into(),
                    agent_kind: AgentKind::ClaudeCode,
                    state: RunState::Idle,
                    confidence: Confidence::High,
                    cwd: "/b".into(),
                    last_message: None,
                    urgency: None,
                    waiting_since: None,
                },
            ],
        };
        assert!(row.render_line().contains("(2 runs)"));
    }

    // ── Multiple runs rollup property: highest-state wins ─────────────────────

    #[test]
    fn rollup_reflects_most_urgent_of_multiple_runs() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "p", RunState::Idle)));
        st.apply(Event::run_added("s1", run("r1", RunState::Idle, None)));
        st.apply(Event::run_added("s1", run("r2", RunState::Done, None)));
        st.apply(Event::run_added("s1", run("r3", RunState::Working, None)));
        st.apply(Event::run_added("s1", run("r4", RunState::Error, None)));
        st.apply(Event::run_added(
            "s1",
            run("r5", RunState::Waiting, Some(Urgency::Approval)),
        ));

        let rows = st.rows();
        assert_eq!(rows[0].rollup_state, RunState::Waiting);
        assert_eq!(rows[0].rollup_urgency, Some(Urgency::Approval));
        assert_eq!(rows[0].runs.len(), 5);
    }

    #[test]
    fn urgency_none_normalized_to_none_in_rollup() {
        // A run with Urgency::None should NOT leak as Some(Urgency::None) in the row.
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "p", RunState::Idle)));
        let mut r = run("r1", RunState::Waiting, None);
        r.urgency = Some(Urgency::None);
        st.apply(Event::run_added("s1", r));
        let rows = st.rows();
        // rollup_urgency should be None, not Some(Urgency::None).
        assert_eq!(rows[0].rollup_urgency, None);
    }

    // ── apply_all convenience ─────────────────────────────────────────────────

    #[test]
    fn apply_all_reduces_sequence() {
        let mut st = CliState::new();
        st.apply_all([
            Event::snapshot(vec![]),
            Event::session_added(session("s1", "a", RunState::Idle)),
            Event::run_added("s1", run("r1", RunState::Working, None)),
            Event::run_updated("s1", run("r1", RunState::Done, None)),
        ]);
        let rows = st.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rollup_state, RunState::Done);
    }

    // ── Codex/Claude mixed agents ─────────────────────────────────────────────

    #[test]
    fn mixed_agent_kinds_in_same_session() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "mixed", RunState::Idle)));
        st.apply(Event::run_added(
            "s1",
            run("r-claude", RunState::Working, None),
        ));
        st.apply(Event::run_added("s1", run_codex("r-codex", RunState::Idle)));

        let rows = st.rows();
        assert_eq!(rows[0].runs.len(), 2);
        // One Claude, one Codex.
        let kinds: Vec<_> = rows[0].runs.iter().map(|r| &r.agent_kind).collect();
        assert!(kinds.iter().any(|k| matches!(k, AgentKind::ClaudeCode)));
        assert!(kinds.iter().any(|k| matches!(k, AgentKind::Codex)));
        // Rollup = working (most urgent of working + idle).
        assert_eq!(rows[0].rollup_state, RunState::Working);
    }

    // ── Forward-compat: schema_version on events is captured ─────────────────

    #[test]
    fn snapshot_with_future_schema_version_tolerated() {
        // Build a snapshot JSON with schema_version = 99 (a future version the
        // current CLI cannot know about). The Event must still deserialize.
        let raw = serde_json::json!({
            "type": "fleet.snapshot",
            "schema_version": 99,
            "sessions": [],
            "future_key": "ignored"
        });
        // This checks that the protocol types do not `deny_unknown_fields`.
        let ev: Event = serde_json::from_value(raw).unwrap();
        let mut st = CliState::new();
        st.apply(ev);
        assert!(st.is_empty());
    }

    #[test]
    fn run_added_event_with_unknown_fields_tolerated() {
        let raw = serde_json::json!({
            "type": "run.added",
            "schema_version": SCHEMA_VERSION,
            "session_id": "s1",
            "run": {
                "schema_version": SCHEMA_VERSION,
                "run_id": "r1",
                "agent_kind": "codex",
                "native_id": "thread-99",
                "cwd": "/tmp",
                "state": "working",
                "confidence": "high",
                "updated_at": "2026-06-08T00:00:00Z",
                "future_field_x": [1, 2, 3]
            }
        });
        let ev: Event = serde_json::from_value(raw).unwrap();
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        st.apply(ev);
        assert_eq!(st.rows()[0].runs.len(), 1);
    }

    // ── Idempotent apply (same event twice) ───────────────────────────────────

    #[test]
    fn applying_session_added_twice_is_idempotent() {
        let mut st = CliState::new();
        let s = session("s1", "proj", RunState::Idle);
        st.apply(Event::session_added(s.clone()));
        st.apply(Event::session_added(s));
        // session.added is an upsert — second apply replaces the first entry.
        assert_eq!(st.session_count(), 1);
    }

    #[test]
    fn applying_run_added_twice_is_idempotent() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        let r = run("r1", RunState::Working, None);
        st.apply(Event::run_added("s1", r.clone()));
        st.apply(Event::run_added("s1", r));
        // run.added is also an upsert.
        assert_eq!(st.rows()[0].runs.len(), 1);
    }

    // ── Error and Dead states ─────────────────────────────────────────────────

    #[test]
    fn error_state_ranks_above_working() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session(
            "s-working",
            "W",
            RunState::Working,
        )));
        st.apply(Event::session_added(session(
            "s-error",
            "E",
            RunState::Error,
        )));
        let rows = st.rows();
        assert_eq!(rows[0].session_id, "s-error");
    }

    #[test]
    fn dead_state_ranks_below_all() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s-dead", "D", RunState::Dead)));
        st.apply(Event::session_added(session("s-idle", "I", RunState::Idle)));
        let rows = st.rows();
        assert_eq!(rows[0].session_id, "s-idle");
        assert_eq!(rows[1].session_id, "s-dead");
    }

    // ── run.updated for run not yet in the session ─────────────────────────────

    #[test]
    fn run_updated_for_unknown_run_is_added() {
        // run.updated behaves as an upsert — if the run isn't known yet, add it.
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        st.apply(Event::run_updated(
            "s1",
            run("new-run", RunState::Working, None),
        ));
        assert_eq!(st.rows()[0].runs.len(), 1);
        assert_eq!(st.rows()[0].rollup_state, RunState::Working);
    }

    // ── Confidence-honesty: inferred vs high ─────────────────────────────────

    #[test]
    fn confidence_is_preserved_in_run_row() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));

        let mut r = run("r1", RunState::Waiting, Some(Urgency::Approval));
        r.confidence = Confidence::Inferred;
        st.apply(Event::run_added("s1", r));

        let rows = st.rows();
        assert_eq!(rows[0].runs[0].confidence, Confidence::Inferred);
    }

    #[test]
    fn high_confidence_is_preserved_in_run_row() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));

        let mut r = run("r1", RunState::Waiting, Some(Urgency::Approval));
        r.confidence = Confidence::High;
        st.apply(Event::run_added("s1", r));

        let rows = st.rows();
        assert_eq!(rows[0].runs[0].confidence, Confidence::High);
    }

    #[test]
    fn run_row_renders_confidence_label_readably() {
        // Regression: the row used to render "(inferredconf)" — confidence label
        // must read "(conf: inferred)" / "(conf: high)", not be glued to "conf".
        for (c, want) in [
            (Confidence::Inferred, "(conf: inferred)"),
            (Confidence::High, "(conf: high)"),
        ] {
            let mut st = CliState::new();
            st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
            let mut r = run("r1", RunState::Waiting, Some(Urgency::Approval));
            r.confidence = c;
            st.apply(Event::run_added("s1", r));
            let line = format_run_row(&st.rows()[0].runs[0], "");
            assert!(line.contains(want), "want {want:?} in: {line}");
            assert!(!line.contains("inferredconf") && !line.contains("highconf"));
        }
    }

    // ── state_label: every state renders its own distinct token ────────────────

    #[test]
    fn state_label_covers_all_states() {
        // render_line() routes the rollup_state through state_label; assert each
        // of the six states yields its own word (covers the Done/Error/Dead arms).
        let cases = [
            (RunState::Working, "[working]"),
            (RunState::Waiting, "[waiting]"),
            (RunState::Idle, "[idle]"),
            (RunState::Done, "[done]"),
            (RunState::Error, "[error]"),
            (RunState::Dead, "[dead]"),
        ];
        for (state, want) in cases {
            let row = Row {
                session_id: "s".into(),
                title: "t".into(),
                rollup_state: state,
                rollup_urgency: None,
                muted: false,
                unread: false,
                runs: vec![],
            };
            let line = row.render_line();
            assert!(
                line.contains(want),
                "state {state:?} → want {want:?} in: {line}"
            );
        }
    }

    // ── urgency_label: every urgency renders its own token (or empty) ──────────

    #[test]
    fn urgency_label_covers_all_variants() {
        // render_line() routes rollup_urgency through urgency_label; assert each
        // variant yields its own marker (covers the Question/IdleDone arms) and
        // that Urgency::None renders no marker (the empty-string arm).
        let cases = [
            (Some(Urgency::Approval), "[approval]"),
            (Some(Urgency::Question), "[question]"),
            (Some(Urgency::IdleDone), "[idle-done]"),
        ];
        for (urgency, want) in cases {
            let row = Row {
                session_id: "s".into(),
                title: "t".into(),
                rollup_state: RunState::Waiting,
                rollup_urgency: urgency,
                muted: false,
                unread: false,
                runs: vec![],
            };
            let line = row.render_line();
            assert!(
                line.contains(want),
                "urgency {urgency:?} → want {want:?} in: {line}"
            );
        }
        // Some(Urgency::None) renders no urgency marker (the `=> ""` arm).
        let none_row = Row {
            session_id: "s".into(),
            title: "t".into(),
            rollup_state: RunState::Waiting,
            rollup_urgency: Some(Urgency::None),
            muted: false,
            unread: false,
            runs: vec![],
        };
        let line = none_row.render_line();
        assert!(
            !line.contains("[approval]")
                && !line.contains("[question]")
                && !line.contains("[idle-done]"),
            "Urgency::None must render no urgency marker, got: {line}"
        );
    }

    // ── sort_key: the IdleDone urgency arm participates in ordering ────────────

    #[test]
    fn idle_done_urgency_sorts_between_question_and_none() {
        // Exercises the `Some(Urgency::IdleDone) => 1` arm of sort_key: an
        // idle-done session must rank below a question one but above a no-urgency
        // one (all in the same state so urgency is the deciding key).
        let mut st = CliState::new();
        let mut s_q = session("s-q", "q", RunState::Waiting);
        s_q.rollup_urgency = Some(Urgency::Question);
        let mut s_id = session("s-id", "id", RunState::Waiting);
        s_id.rollup_urgency = Some(Urgency::IdleDone);
        let s_none = session("s-none", "none", RunState::Waiting);
        st.apply(Event::session_added(s_none));
        st.apply(Event::session_added(s_id));
        st.apply(Event::session_added(s_q));
        let rows = st.rows();
        assert_eq!(rows[0].rollup_urgency, Some(Urgency::Question));
        assert_eq!(rows[1].rollup_urgency, Some(Urgency::IdleDone));
        assert_eq!(rows[2].rollup_urgency, None);
    }

    // ── run.removed leaves an urgent run → rollup_urgency stays Some ───────────

    #[test]
    fn run_removed_leaving_urgent_run_keeps_some_urgency() {
        // Remove a NON-urgent run, leaving an urgent one behind: the recomputed
        // rollup_urgency after removal must be `Some(_)` (covers the `Some(u)`
        // arm of the RunRemoved rollup recompute, not just the `None` arm).
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        st.apply(Event::run_added(
            "s1",
            run("r-plain", RunState::Working, None),
        ));
        st.apply(Event::run_added(
            "s1",
            run("r-urgent", RunState::Waiting, Some(Urgency::Approval)),
        ));
        // Remove the plain (non-urgent) run — the urgent one survives.
        st.apply(Event::run_removed("s1", "r-plain"));
        let rows = st.rows();
        assert_eq!(rows[0].runs.len(), 1);
        assert_eq!(rows[0].rollup_state, RunState::Waiting);
        assert_eq!(
            rows[0].rollup_urgency,
            Some(Urgency::Approval),
            "an urgent run surviving a removal keeps the rollup urgency"
        );
    }

    // ── format_run_row: every agent kind renders its own label ─────────────────

    #[test]
    fn format_run_row_covers_all_agent_kinds() {
        // Exercises the codex + other arms of format_run_row's `kind` match (the
        // claude arm is already covered by other tests).
        for (kind, want) in [
            (AgentKind::ClaudeCode, "[claude]"),
            (AgentKind::Codex, "[codex]"),
            (AgentKind::Other, "[other]"),
        ] {
            let rr = RunRow {
                run_id: "r1".into(),
                agent_kind: kind.clone(),
                state: RunState::Working,
                confidence: Confidence::High,
                cwd: "/x".into(),
                last_message: None,
                urgency: None,
                waiting_since: None,
            };
            let line = format_run_row(&rr, "");
            assert!(
                line.contains(want),
                "kind {kind:?} → want {want:?} in: {line}"
            );
        }
    }

    // ── format_run_row: waiting_since + last_message branches both render ───────

    #[test]
    fn format_run_row_renders_waiting_since_and_message() {
        let rr = RunRow {
            run_id: "r1".into(),
            agent_kind: AgentKind::ClaudeCode,
            state: RunState::Waiting,
            confidence: Confidence::High,
            cwd: "/proj".into(),
            last_message: Some("needs approval".into()),
            urgency: Some(Urgency::Approval),
            waiting_since: Some("2026-06-08T10:00:00Z".into()),
        };
        let line = format_run_row(&rr, "");
        assert!(
            line.contains("waiting since 2026-06-08T10:00:00Z"),
            "{line}"
        );
        assert!(line.contains("\"needs approval\""), "{line}");
    }
}
