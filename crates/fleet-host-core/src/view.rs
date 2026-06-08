//! The inbox view-model and its reducer (PLAN S19).
//!
//! [`InboxModel`] holds the host face's authoritative copy of Hub state — a map
//! of [`Session`]s keyed by `session_id`, plus their insertion order — and folds
//! protocol [`Event`]s into it. [`InboxModel::view`] projects that state into an
//! [`InboxView`]: the **vertical list of session tabs** the window draws.
//!
//! Everything here is pure: no I/O, no async, no window. The reduce is
//! deterministic — `apply`ing the same event sequence always yields the same
//! [`InboxView`] — which is exactly the `◆G3` gate criterion ("UI reducer
//! determinism (snapshot+delta→view)").

use std::collections::HashMap;

use fleet_protocol::{
    rollup::{rollup_state, rollup_urgency},
    AgentKind, AgentRun, Confidence, Event, LocationGlyph, Session, State, Urgency,
};

/// The reduced per-run lifecycle state shown on a tab.
///
/// This mirrors [`fleet_protocol::State`] one-to-one (so **D9** is honored —
/// [`TabState::Done`] is never folded into [`TabState::Idle`]) but is its own
/// type so the host view-model is decoupled from any wire-token churn and can
/// carry view-only helpers (`glyph`, `is_attention`) the GUI needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabState {
    Working,
    Waiting,
    Idle,
    Done,
    Error,
    Dead,
}

impl TabState {
    /// All variants, for exhaustive testing.
    pub const ALL: [TabState; 6] = [
        TabState::Working,
        TabState::Waiting,
        TabState::Idle,
        TabState::Done,
        TabState::Error,
        TabState::Dead,
    ];

    /// Project a protocol [`State`] into a [`TabState`]. Total and lossless.
    pub fn from_state(s: State) -> Self {
        match s {
            State::Working => TabState::Working,
            State::Waiting => TabState::Waiting,
            State::Idle => TabState::Idle,
            State::Done => TabState::Done,
            State::Error => TabState::Error,
            State::Dead => TabState::Dead,
        }
    }

    /// A short status glyph for the tab (the leading column the window draws).
    /// Stable text the GUI maps to an icon; kept here so the CLI and GUI can
    /// share the vocabulary if desired.
    pub fn glyph(self) -> &'static str {
        match self {
            TabState::Working => "▶", // running
            TabState::Waiting => "⏸", // blocked on the user (the only ping)
            TabState::Idle => "·",    // alive, awaiting prompt
            TabState::Done => "✓",    // task reported complete (distinct from idle, D9)
            TabState::Error => "✕",   // errored
            TabState::Dead => "☠",    // process gone / timed out
        }
    }

    /// `true` only for [`TabState::Waiting`] — the single attention-demanding
    /// state (§7.3, "the only state that pings"). The [`crate::notify`] and
    /// [`crate::sort`] seams build on this.
    pub fn is_attention(self) -> bool {
        matches!(self, TabState::Waiting)
    }
}

/// The agent flavor icon shown next to a tab (README §7.2 `agent_kind`).
///
/// One tab can host several runs of mixed kinds; [`SessionTab::agent_icon`] is
/// the icon for the *rolled-up* (most-urgent) run so the window has a single
/// glyph to draw per tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentIcon {
    Claude,
    Codex,
    Other,
    /// No runs in the session yet (e.g. a freshly-registered session).
    None,
}

impl AgentIcon {
    pub fn from_kind(k: &AgentKind) -> Self {
        match k {
            AgentKind::ClaudeCode => AgentIcon::Claude,
            AgentKind::Codex => AgentIcon::Codex,
            AgentKind::Other => AgentIcon::Other,
        }
    }

    /// A short label the GUI maps to its icon asset.
    pub fn label(self) -> &'static str {
        match self {
            AgentIcon::Claude => "claude",
            AgentIcon::Codex => "codex",
            AgentIcon::Other => "agent",
            AgentIcon::None => "",
        }
    }
}

/// One vertical session tab in the inbox (PLAN S19: "glyph, agent icon, title,
/// state").
///
/// This is the **stable view-model row** the host window renders and later
/// slices extend. Fields may be *added* (S20 age, S22 confidence rendering,
/// S25 mute affordances already present), but the reduce that produces them
/// stays the contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTab {
    /// Stable id — same as [`Session::session_id`]. The window's selection key.
    pub session_id: String,
    /// The location glyph (laptop / docker / remote) — the leftmost column.
    pub glyph: LocationGlyph,
    /// The rolled-up agent flavor icon for this tab.
    pub agent_icon: AgentIcon,
    /// Display title (the session title).
    pub title: String,
    /// Rolled-up lifecycle state across the session's runs (most urgent).
    pub state: TabState,
    /// Rolled-up urgency across runs; `None` ⇒ no urgency. Drives notify (S21).
    pub urgency: Option<Urgency>,
    /// Worst confidence among the session's *waiting* runs (the ones whose
    /// confidence the GUI surfaces, S22). `None` when nothing is waiting.
    /// Invariant 5: this is reported truthfully, never upgraded here.
    pub confidence: Option<Confidence>,
    /// ISO-8601 timestamp the rolled-up run entered `waiting`; `None` otherwise.
    /// The S20 waiting-age timer reads this (kept as the raw stamp so the view
    /// stays a pure function — age is computed against an injected "now").
    pub waiting_since: Option<String>,
    /// Whether the session is muted (S25). Surfaced now; the [`crate::mute`]
    /// seam adds the command plumbing.
    pub muted: bool,
    /// Whether the session is soloed (S25).
    pub soloed: bool,
    /// Whether the session has an unread notification (drives the badge, S21).
    pub unread: bool,
    /// Number of runs hosted under this tab (the window shows a count badge).
    pub run_count: usize,
    /// The rolled-up run's last message — the inbox preview line (e.g. the
    /// assistant's final words on `idle`/`done`, or "Approve Bash?" on `waiting`).
    /// `None` when the rolled-up run has no message. Taken from the run that owns
    /// the rolled-up state (same run as [`SessionTab::agent_icon`]).
    pub last_message: Option<String>,
}

/// The whole reduced inbox: the ordered list of session tabs the window draws.
///
/// **Default ordering is insertion order** — deterministic and window-agnostic.
/// The S20 [`crate::sort`] slice will layer the `(unread, urgency, age)` sort on
/// top; until then the view exposes raw Hub order so the reducer-determinism
/// tests assert a single, stable shape.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InboxView {
    /// The vertical session tabs, in insertion order.
    pub tabs: Vec<SessionTab>,
}

impl InboxView {
    /// Number of tabs.
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// Whether the inbox is empty.
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// Borrow a tab by session id, if present.
    pub fn tab(&self, session_id: &str) -> Option<&SessionTab> {
        self.tabs.iter().find(|t| t.session_id == session_id)
    }
}

/// The host face's reducer: folds protocol [`Event`]s into [`InboxView`]s.
///
/// Mirrors the Hub merge engine and the CLI reducer (it reuses
/// [`fleet_protocol::rollup`] for the rollup ordering) so all three faces agree.
/// Sessions are stored in a map for O(1) id lookup with a parallel `order` vec
/// so [`view`](Self::view) is deterministic.
#[derive(Debug, Default, Clone)]
pub struct InboxModel {
    sessions: HashMap<String, Session>,
    /// Insertion order of session ids, for a deterministic view.
    order: Vec<String>,
}

impl InboxModel {
    /// A fresh, empty model.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of sessions currently tracked.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether the model holds no sessions.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Apply one [`Event`] from the Hub, mutating the model in place.
    ///
    /// Semantics match the Hub merge engine and the CLI reducer:
    /// - `fleet.snapshot` replaces the whole model (and resets order).
    /// - `session.added`/`session.updated` upsert the session (keyed by id);
    ///   a newly-seen id appends to `order`, an existing id keeps its slot.
    /// - `session.removed` drops the session and its order slot.
    /// - `run.added`/`run.updated` upsert the run within its session (keyed by
    ///   `run_id`) and recompute that session's rollups locally; a delta for an
    ///   unknown session is a harmless no-op (the next snapshot is canonical).
    /// - `run.removed` drops the run and recomputes rollups.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Snapshot { sessions, .. } => {
                self.sessions.clear();
                self.order.clear();
                for s in sessions {
                    let id = s.session_id.clone();
                    if !self.sessions.contains_key(&id) {
                        self.order.push(id.clone());
                    }
                    self.sessions.insert(id, s);
                }
            }

            Event::SessionAdded { session, .. } | Event::SessionUpdated { session, .. } => {
                let id = session.session_id.clone();
                if !self.sessions.contains_key(&id) {
                    self.order.push(id.clone());
                }
                self.sessions.insert(id, session);
            }

            Event::SessionRemoved { session_id, .. } => {
                if self.sessions.remove(&session_id).is_some() {
                    self.order.retain(|id| id != &session_id);
                }
            }

            Event::RunAdded {
                session_id, run, ..
            }
            | Event::RunUpdated {
                session_id, run, ..
            } => {
                if let Some(sess) = self.sessions.get_mut(&session_id) {
                    upsert_run(sess, run);
                    recompute_rollups(sess);
                }
            }

            Event::RunRemoved {
                session_id, run_id, ..
            } => {
                if let Some(sess) = self.sessions.get_mut(&session_id) {
                    sess.runs.retain(|r| r.run_id != run_id);
                    recompute_rollups(sess);
                }
            }
        }
    }

    /// Apply a sequence of events, returning `&mut self` for chaining in tests.
    pub fn apply_all(&mut self, events: impl IntoIterator<Item = Event>) -> &mut Self {
        for ev in events {
            self.apply(ev);
        }
        self
    }

    /// Project the current state into an [`InboxView`].
    ///
    /// Pure and deterministic: the tabs come out in insertion order, each a
    /// projection of its session via [`session_to_tab`]. This is the function
    /// the `◆G3` reducer-determinism tests pin.
    pub fn view(&self) -> InboxView {
        let tabs = self
            .order
            .iter()
            .filter_map(|id| self.sessions.get(id))
            .map(session_to_tab)
            .collect();
        InboxView { tabs }
    }
}

/// Upsert a run into a session (keyed by `run_id`): replace in place if present,
/// otherwise append (preserving run order).
fn upsert_run(sess: &mut Session, run: AgentRun) {
    if let Some(pos) = sess.runs.iter().position(|r| r.run_id == run.run_id) {
        sess.runs[pos] = run;
    } else {
        sess.runs.push(run);
    }
}

/// Recompute a session's `rollup_state`/`rollup_urgency` from its current runs,
/// using the shared protocol ordering. An empty run set keeps the existing
/// `rollup_state` (we never invent one) and clears `rollup_urgency`. Normalizes
/// [`Urgency::None`] to `None` on the optional field, matching the Hub.
fn recompute_rollups(sess: &mut Session) {
    if let Some(state) = rollup_state(&sess.runs) {
        sess.rollup_state = state;
    }
    sess.rollup_urgency = match rollup_urgency(&sess.runs) {
        Some(Urgency::None) | None => None,
        Some(u) => Some(u),
    };
}

/// The run that owns the rolled-up state (state-match, else the first run) — the
/// single run the tab's icon/preview represent. `None` when there are no runs.
fn rolled_up_run(sess: &Session) -> Option<&AgentRun> {
    sess.runs
        .iter()
        .find(|r| r.state == sess.rollup_state)
        .or_else(|| sess.runs.first())
}

/// The agent icon for a session: the kind of its rolled-up run, or
/// [`AgentIcon::None`] when there are no runs.
fn rolled_up_agent_icon(sess: &Session) -> AgentIcon {
    rolled_up_run(sess)
        .map(|r| AgentIcon::from_kind(&r.agent_kind))
        .unwrap_or(AgentIcon::None)
}

/// The rolled-up run's `last_message` — the inbox preview line.
fn rolled_up_last_message(sess: &Session) -> Option<String> {
    rolled_up_run(sess).and_then(|r| r.last_message.clone())
}

/// The worst confidence among a session's *waiting* runs, plus the earliest
/// `waiting_since` stamp — the pair the GUI surfaces for the attention tier.
///
/// "Worst" = [`Confidence::Inferred`] beats [`Confidence::High`] (we report the
/// *least* trustworthy waiting signal so the badge never overstates — invariant
/// 5). `None` when nothing is waiting.
fn waiting_confidence_and_since(sess: &Session) -> (Option<Confidence>, Option<String>) {
    let mut conf: Option<Confidence> = None;
    let mut since: Option<String> = None;
    for r in sess.runs.iter().filter(|r| r.state == State::Waiting) {
        conf = Some(match (conf, r.confidence) {
            // Inferred is the weaker signal → it wins the "worst" reduction.
            (Some(Confidence::Inferred), _) | (_, Confidence::Inferred) => Confidence::Inferred,
            _ => Confidence::High,
        });
        if let Some(w) = &r.waiting_since {
            since = match since {
                Some(prev) if prev <= *w => Some(prev),
                _ => Some(w.clone()),
            };
        }
    }
    (conf, since)
}

/// Project a [`Session`] into its [`SessionTab`].
fn session_to_tab(sess: &Session) -> SessionTab {
    let (confidence, waiting_since) = waiting_confidence_and_since(sess);
    SessionTab {
        session_id: sess.session_id.clone(),
        glyph: sess.location.glyph.clone(),
        agent_icon: rolled_up_agent_icon(sess),
        title: sess.title.clone(),
        state: TabState::from_state(sess.rollup_state),
        urgency: sess.rollup_urgency,
        confidence,
        waiting_since,
        muted: sess.muted,
        soloed: sess.soloed,
        unread: sess.unread,
        run_count: sess.runs.len(),
        last_message: rolled_up_last_message(sess),
    }
}

// ── Unit tests (heavy, pure-function, no window) ──────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::{
        AgentKind, AgentRun, Confidence, Event, Extra, Location, LocationGlyph, LocationKind,
        Server, ServerKind, Session, State, Urgency,
    };

    // ── Helpers ────────────────────────────────────────────────────────────────

    fn loc(glyph: LocationGlyph) -> Location {
        Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
            glyph,
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

    fn session(id: &str, title: &str, state: State) -> Session {
        Session::new(
            id,
            title,
            loc(LocationGlyph::Laptop),
            srv(),
            state,
            "2026-06-08T00:00:00Z",
        )
    }

    fn run(id: &str, kind: AgentKind, state: State, urgency: Option<Urgency>) -> AgentRun {
        let mut r = AgentRun::new(
            id,
            kind,
            "native-1",
            "/home/user/project",
            state,
            Confidence::High,
            "2026-06-08T00:00:00Z",
        );
        r.urgency = urgency;
        r
    }

    // ── empty / snapshot ─────────────────────────────────────────────────────

    #[test]
    fn fresh_model_is_empty() {
        let m = InboxModel::new();
        assert!(m.is_empty());
        assert!(m.view().is_empty());
        assert_eq!(m.view().len(), 0);
    }

    #[test]
    fn empty_snapshot_gives_empty_view() {
        let mut m = InboxModel::new();
        m.apply(Event::snapshot(vec![]));
        assert!(m.view().is_empty());
    }

    #[test]
    fn snapshot_populates_tabs_in_insertion_order() {
        let mut m = InboxModel::new();
        m.apply(Event::snapshot(vec![
            session("s1", "alpha", State::Idle),
            session("s2", "beta", State::Working),
            session("s3", "gamma", State::Done),
        ]));
        let v = m.view();
        let ids: Vec<_> = v.tabs.iter().map(|t| t.session_id.as_str()).collect();
        // Insertion order preserved (NOT urgency-sorted — that's S20).
        assert_eq!(ids, vec!["s1", "s2", "s3"]);
        assert_eq!(v.tabs[1].title, "beta");
        assert_eq!(v.tabs[1].state, TabState::Working);
    }

    #[test]
    fn second_snapshot_replaces_first_and_resets_order() {
        let mut m = InboxModel::new();
        m.apply(Event::snapshot(vec![session("old", "o", State::Idle)]));
        m.apply(Event::snapshot(vec![
            session("s2", "a", State::Idle),
            session("s1", "b", State::Idle),
        ]));
        let v = m.view();
        let ids: Vec<_> = v.tabs.iter().map(|t| t.session_id.as_str()).collect();
        assert_eq!(ids, vec!["s2", "s1"]);
        assert!(v.tab("old").is_none());
    }

    // ── session.added / updated / removed ────────────────────────────────────

    #[test]
    fn session_added_appends_tab() {
        let mut m = InboxModel::new();
        m.apply(Event::snapshot(vec![]));
        m.apply(Event::session_added(session("s1", "alpha", State::Working)));
        let v = m.view();
        assert_eq!(v.len(), 1);
        assert_eq!(v.tabs[0].title, "alpha");
        assert_eq!(v.tabs[0].state, TabState::Working);
    }

    #[test]
    fn session_updated_replaces_in_place_keeping_order() {
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "one", State::Idle)));
        m.apply(Event::session_added(session("s2", "two", State::Idle)));
        // Update s1's title and state; order must NOT change.
        m.apply(Event::session_updated(session(
            "s1",
            "one-prime",
            State::Done,
        )));
        let v = m.view();
        let ids: Vec<_> = v.tabs.iter().map(|t| t.session_id.as_str()).collect();
        assert_eq!(ids, vec!["s1", "s2"]);
        assert_eq!(v.tab("s1").unwrap().title, "one-prime");
        assert_eq!(v.tab("s1").unwrap().state, TabState::Done);
    }

    #[test]
    fn session_removed_drops_tab() {
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "a", State::Idle)));
        m.apply(Event::session_added(session("s2", "b", State::Idle)));
        m.apply(Event::session_removed("s1"));
        let v = m.view();
        assert_eq!(v.len(), 1);
        assert_eq!(v.tabs[0].session_id, "s2");
    }

    #[test]
    fn removing_unknown_session_is_noop() {
        let mut m = InboxModel::new();
        m.apply(Event::snapshot(vec![]));
        m.apply(Event::session_removed("ghost")); // must not panic
        assert!(m.view().is_empty());
    }

    // ── run.added / updated / removed → rollups ──────────────────────────────

    #[test]
    fn run_added_updates_rollup_state_and_icon() {
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "p", State::Idle)));
        assert_eq!(m.view().tabs[0].state, TabState::Idle);
        assert_eq!(m.view().tabs[0].agent_icon, AgentIcon::None);
        assert_eq!(m.view().tabs[0].run_count, 0);

        m.apply(Event::run_added(
            "s1",
            run("r1", AgentKind::Codex, State::Working, None),
        ));
        let t = &m.view().tabs[0];
        assert_eq!(t.state, TabState::Working);
        assert_eq!(t.agent_icon, AgentIcon::Codex);
        assert_eq!(t.run_count, 1);
    }

    #[test]
    fn run_added_promotes_rollup_to_waiting_with_urgency() {
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "p", State::Idle)));
        m.apply(Event::run_added(
            "s1",
            run("r1", AgentKind::ClaudeCode, State::Working, None),
        ));
        m.apply(Event::run_added(
            "s1",
            run(
                "r2",
                AgentKind::ClaudeCode,
                State::Waiting,
                Some(Urgency::Approval),
            ),
        ));
        let t = &m.view().tabs[0];
        assert_eq!(t.state, TabState::Waiting);
        assert_eq!(t.urgency, Some(Urgency::Approval));
        assert!(t.state.is_attention());
    }

    #[test]
    fn run_updated_in_place_does_not_duplicate() {
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "p", State::Idle)));
        m.apply(Event::run_added(
            "s1",
            run("r1", AgentKind::Codex, State::Working, None),
        ));
        m.apply(Event::run_updated(
            "s1",
            run("r1", AgentKind::Codex, State::Done, None),
        ));
        let t = &m.view().tabs[0];
        assert_eq!(t.run_count, 1, "upsert must not append");
        assert_eq!(t.state, TabState::Done);
    }

    #[test]
    fn run_update_for_unknown_session_is_noop() {
        let mut m = InboxModel::new();
        m.apply(Event::run_updated(
            "no-such",
            run("r1", AgentKind::Codex, State::Working, None),
        ));
        assert!(m.view().is_empty());
    }

    #[test]
    fn run_removed_recomputes_rollup() {
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "p", State::Idle)));
        m.apply(Event::run_added(
            "s1",
            run("r1", AgentKind::Codex, State::Working, None),
        ));
        m.apply(Event::run_added(
            "s1",
            run(
                "r2",
                AgentKind::Codex,
                State::Waiting,
                Some(Urgency::Question),
            ),
        ));
        assert_eq!(m.view().tabs[0].state, TabState::Waiting);
        m.apply(Event::run_removed("s1", "r2"));
        let t = &m.view().tabs[0];
        assert_eq!(t.state, TabState::Working);
        assert_eq!(t.urgency, None);
        assert_eq!(t.run_count, 1);
    }

    // ── D9: done distinct from idle ──────────────────────────────────────────

    #[test]
    fn done_is_distinct_from_idle_d9() {
        assert_ne!(TabState::Done, TabState::Idle);
        assert_eq!(TabState::from_state(State::Done), TabState::Done);
        assert_eq!(TabState::from_state(State::Idle), TabState::Idle);
        // Glyphs differ so the window can render them distinctly.
        assert_ne!(TabState::Done.glyph(), TabState::Idle.glyph());
    }

    #[test]
    fn tabstate_projection_is_total_and_lossless() {
        for s in State::ALL {
            let t = TabState::from_state(s);
            // Round-trip the discriminant by name to prove no two collapse.
            let expected = match s {
                State::Working => TabState::Working,
                State::Waiting => TabState::Waiting,
                State::Idle => TabState::Idle,
                State::Done => TabState::Done,
                State::Error => TabState::Error,
                State::Dead => TabState::Dead,
            };
            assert_eq!(t, expected);
        }
        // The six states map to six distinct tab states.
        let mut seen = std::collections::HashSet::new();
        for s in State::ALL {
            assert!(seen.insert(TabState::from_state(s)));
        }
        assert_eq!(seen.len(), 6);
    }

    // ── confidence honesty (invariant 5) ─────────────────────────────────────

    #[test]
    fn waiting_confidence_reports_worst_signal() {
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "p", State::Idle)));
        // One high-confidence waiting run, one inferred waiting run.
        let mut high = run(
            "r-high",
            AgentKind::Codex,
            State::Waiting,
            Some(Urgency::Approval),
        );
        high.confidence = Confidence::High;
        let mut inf = run(
            "r-inf",
            AgentKind::ClaudeCode,
            State::Waiting,
            Some(Urgency::Approval),
        );
        inf.confidence = Confidence::Inferred;
        m.apply(Event::run_added("s1", high));
        m.apply(Event::run_added("s1", inf));
        // The tab must report the WEAKER (inferred) signal — never overstate.
        assert_eq!(m.view().tabs[0].confidence, Some(Confidence::Inferred));
    }

    #[test]
    fn confidence_is_none_when_nothing_waiting() {
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "p", State::Idle)));
        m.apply(Event::run_added(
            "s1",
            run("r1", AgentKind::Codex, State::Working, None),
        ));
        assert_eq!(m.view().tabs[0].confidence, None);
    }

    #[test]
    fn high_confidence_surfaced_when_only_high_waiting() {
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "p", State::Idle)));
        let mut r = run(
            "r1",
            AgentKind::Codex,
            State::Waiting,
            Some(Urgency::Approval),
        );
        r.confidence = Confidence::High;
        r.waiting_since = Some("2026-06-08T10:00:00Z".into());
        m.apply(Event::run_added("s1", r));
        let t = &m.view().tabs[0];
        assert_eq!(t.confidence, Some(Confidence::High));
        assert_eq!(t.waiting_since.as_deref(), Some("2026-06-08T10:00:00Z"));
    }

    #[test]
    fn waiting_since_is_earliest_across_waiting_runs() {
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "p", State::Idle)));
        let mut r1 = run(
            "r1",
            AgentKind::Codex,
            State::Waiting,
            Some(Urgency::Approval),
        );
        r1.waiting_since = Some("2026-06-08T11:00:00Z".into());
        let mut r2 = run(
            "r2",
            AgentKind::Codex,
            State::Waiting,
            Some(Urgency::Approval),
        );
        r2.waiting_since = Some("2026-06-08T10:00:00Z".into());
        m.apply(Event::run_added("s1", r1));
        m.apply(Event::run_added("s1", r2));
        // Earliest wins (the longest-waiting run drives the age timer, S20).
        assert_eq!(
            m.view().tabs[0].waiting_since.as_deref(),
            Some("2026-06-08T10:00:00Z")
        );
    }

    // ── glyph / agent icon / muted-soloed-unread surfacing ───────────────────

    #[test]
    fn tab_carries_location_glyph() {
        let mut m = InboxModel::new();
        let mut s = session("s1", "p", State::Idle);
        s.location.glyph = LocationGlyph::Docker;
        m.apply(Event::session_added(s));
        assert_eq!(m.view().tabs[0].glyph, LocationGlyph::Docker);
    }

    #[test]
    fn muted_soloed_unread_surface_on_tab() {
        let mut m = InboxModel::new();
        let mut s = session("s1", "p", State::Waiting);
        s.muted = true;
        s.soloed = true;
        s.unread = true;
        m.apply(Event::session_added(s));
        let t = &m.view().tabs[0];
        assert!(t.muted && t.soloed && t.unread);
    }

    #[test]
    fn agent_icon_is_for_the_rolled_up_run() {
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "mixed", State::Idle)));
        // Codex idle + Claude working → rollup state is working → Claude icon.
        m.apply(Event::run_added(
            "s1",
            run("r-codex", AgentKind::Codex, State::Idle, None),
        ));
        m.apply(Event::run_added(
            "s1",
            run("r-claude", AgentKind::ClaudeCode, State::Working, None),
        ));
        let t = &m.view().tabs[0];
        assert_eq!(t.state, TabState::Working);
        assert_eq!(t.agent_icon, AgentIcon::Claude);
    }

    // ── reducer determinism (the ◆G3 gate criterion) ─────────────────────────

    #[test]
    fn reducer_is_deterministic_for_a_fixed_sequence() {
        // The same event sequence applied twice yields byte-identical views,
        // independent of any window. This is the G3 determinism pin.
        let seq = || {
            vec![
                Event::snapshot(vec![]),
                Event::session_added(session("s1", "web", State::Idle)),
                Event::run_added("s1", run("r1", AgentKind::Codex, State::Working, None)),
                Event::session_added(session("s2", "api", State::Idle)),
                Event::run_added(
                    "s2",
                    run(
                        "r2",
                        AgentKind::ClaudeCode,
                        State::Waiting,
                        Some(Urgency::Approval),
                    ),
                ),
                Event::run_updated("s1", run("r1", AgentKind::Codex, State::Done, None)),
                Event::session_removed("s2"),
            ]
        };

        let mut a = InboxModel::new();
        a.apply_all(seq());
        let mut b = InboxModel::new();
        b.apply_all(seq());

        assert_eq!(a.view(), b.view());
        // And the concrete expected shape:
        let v = a.view();
        assert_eq!(v.len(), 1);
        assert_eq!(v.tabs[0].session_id, "s1");
        assert_eq!(v.tabs[0].state, TabState::Done);
        assert_eq!(v.tabs[0].agent_icon, AgentIcon::Codex);
    }

    #[test]
    fn snapshot_then_deltas_match_expected_inbox_view() {
        // Build the exact expected InboxView and assert structural equality.
        let mut m = InboxModel::new();
        let mut s1 = session("s1", "web", State::Idle);
        s1.runs
            .push(run("r1", AgentKind::Codex, State::Working, None));
        s1.rollup_state = State::Working; // Hub-computed
        m.apply(Event::snapshot(vec![s1]));
        m.apply(Event::session_added(session("s2", "backend", State::Idle)));
        m.apply(Event::run_added(
            "s2",
            run(
                "r2",
                AgentKind::ClaudeCode,
                State::Waiting,
                Some(Urgency::Question),
            ),
        ));

        let expected = InboxView {
            tabs: vec![
                SessionTab {
                    session_id: "s1".into(),
                    glyph: LocationGlyph::Laptop,
                    agent_icon: AgentIcon::Codex,
                    title: "web".into(),
                    state: TabState::Working,
                    urgency: None,
                    confidence: None,
                    waiting_since: None,
                    muted: false,
                    soloed: false,
                    unread: false,
                    run_count: 1,
                    last_message: None,
                },
                SessionTab {
                    session_id: "s2".into(),
                    glyph: LocationGlyph::Laptop,
                    agent_icon: AgentIcon::Claude,
                    title: "backend".into(),
                    state: TabState::Waiting,
                    urgency: Some(Urgency::Question),
                    confidence: Some(Confidence::High),
                    waiting_since: None,
                    muted: false,
                    soloed: false,
                    unread: false,
                    run_count: 1,
                    last_message: None,
                },
            ],
        };
        assert_eq!(m.view(), expected);
    }

    #[test]
    fn full_lifecycle_working_waiting_working_dead() {
        let mut m = InboxModel::new();
        m.apply(Event::snapshot(vec![]));
        m.apply(Event::session_added(session("s1", "proj", State::Idle)));
        m.apply(Event::run_added(
            "s1",
            run("r1", AgentKind::Codex, State::Working, None),
        ));
        assert_eq!(m.view().tabs[0].state, TabState::Working);

        let mut waiting = run(
            "r1",
            AgentKind::Codex,
            State::Waiting,
            Some(Urgency::Approval),
        );
        waiting.waiting_since = Some("2026-06-08T10:00:00Z".into());
        m.apply(Event::run_updated("s1", waiting));
        assert_eq!(m.view().tabs[0].state, TabState::Waiting);
        assert_eq!(m.view().tabs[0].urgency, Some(Urgency::Approval));

        m.apply(Event::run_updated(
            "s1",
            run("r1", AgentKind::Codex, State::Working, None),
        ));
        assert_eq!(m.view().tabs[0].state, TabState::Working);
        assert_eq!(m.view().tabs[0].urgency, None);

        m.apply(Event::session_removed("s1"));
        assert!(m.view().is_empty());
    }

    #[test]
    fn forward_compat_event_with_unknown_fields_reduces() {
        // A newer Hub emits an event with a field this build doesn't know; the
        // reducer must still fold it (protocol types don't deny_unknown_fields).
        let raw = serde_json::json!({
            "type": "run.added",
            "schema_version": 1,
            "session_id": "s1",
            "run": {
                "schema_version": 1,
                "run_id": "r1",
                "agent_kind": "codex",
                "native_id": "thread-9",
                "cwd": "/tmp",
                "state": "working",
                "confidence": "high",
                "updated_at": "2026-06-08T00:00:00Z",
                "future_field": [1, 2, 3]
            }
        });
        let ev: Event = serde_json::from_value(raw).unwrap();
        let mut m = InboxModel::new();
        m.apply(Event::session_added(session("s1", "p", State::Idle)));
        m.apply(ev);
        assert_eq!(m.view().tabs[0].run_count, 1);
        assert_eq!(m.view().tabs[0].state, TabState::Working);
    }
}
