//! The canonical merge engine (README §4.3, §7.4; PLAN S2).
//!
//! The Hub holds exactly one authoritative copy of fleet state — a map of
//! [`Session`]s, each owning its [`AgentRun`]s — and every face is a pure
//! projection of it (the spec's "all faces see the same thing" invariant,
//! §4.3). Reporter deltas flow in; the engine applies them, recomputes each
//! touched session's rollups, and returns the outbound [`Event`]s that describe
//! exactly what changed, which the server fans out to every subscriber.
//!
//! ## Rollup invariant (the G0 property test target)
//!
//! `rollup_state` is the **most-urgent state across a session's runs** and
//! `rollup_urgency` the **most-urgent urgency** likewise, using the shared
//! ordering in [`fleet_protocol::rollup`]. This is the one invariant the G0 gate
//! property-tests: after *any* sequence of deltas, every session's stored rollup
//! equals the recomputed max over its current runs.
//!
//! ## Delta semantics
//!
//! - **upsert** (session or run): insert if absent, replace in place if present
//!   (keyed by `session_id` / `run_id`). Run upserts preserve run order:
//!   existing runs keep their slot, new runs append.
//! - **remove**: drop by id; removing the last run of a session leaves an empty
//!   session (the session itself is removed only by an explicit session.remove).
//! - A run delta targeting an unknown session is a no-op that returns no events
//!   (the reporter must register the session first).

use fleet_protocol::{rollup, AgentRun, Event, Session};
use std::collections::HashMap;

/// The Hub's authoritative state and the engine that mutates it.
///
/// Sessions are stored in a map for O(1) id lookup; a parallel `order` vec
/// preserves insertion order so snapshots are deterministic (important for the
/// two-face-consistency test and for stable CLI rendering).
#[derive(Debug, Default)]
pub struct MergeEngine {
    sessions: HashMap<String, Session>,
    /// Insertion order of session ids, for deterministic snapshots.
    order: Vec<String>,
}

impl MergeEngine {
    /// A fresh, empty engine.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of sessions currently held.
    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    /// Whether the engine holds no sessions.
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Borrow a session by id, if present.
    pub fn session(&self, session_id: &str) -> Option<&Session> {
        self.sessions.get(session_id)
    }

    /// The current full snapshot, in deterministic insertion order. This is the
    /// payload of the `fleet.snapshot` sent on subscribe.
    pub fn snapshot(&self) -> Vec<Session> {
        self.order
            .iter()
            .filter_map(|id| self.sessions.get(id).cloned())
            .collect()
    }

    /// Recompute a session's rollups from its current runs (README §7.1).
    ///
    /// An empty session (no runs) keeps its existing `rollup_state` and clears
    /// `rollup_urgency` — there is nothing to roll up, and we must not invent a
    /// state. Callers that add the first run will recompute immediately after.
    fn recompute_rollups(session: &mut Session) {
        if let Some(state) = rollup::rollup_state(&session.runs) {
            session.rollup_state = state;
        }
        // rollup_urgency is None for an empty run set, and the most-urgent
        // (possibly Urgency::None) otherwise. We normalize Urgency::None → None
        // on the optional field so the wire shows absence rather than "null".
        session.rollup_urgency = match rollup::rollup_urgency(&session.runs) {
            Some(fleet_protocol::Urgency::None) | None => None,
            Some(u) => Some(u),
        };
    }

    /// Insert or replace a whole session (reporter `session.upsert`).
    ///
    /// On upsert the session's rollups are recomputed from whatever runs it
    /// carries, so a reporter that ships a session + runs in one object still
    /// gets a correct rollup. Returns the outbound event (added vs updated).
    pub fn upsert_session(&mut self, mut session: Session) -> Event {
        Self::recompute_rollups(&mut session);
        let id = session.session_id.clone();
        if self.sessions.contains_key(&id) {
            self.sessions.insert(id.clone(), session.clone());
            Event::session_updated(session)
        } else {
            self.order.push(id.clone());
            self.sessions.insert(id, session.clone());
            Event::session_added(session)
        }
    }

    /// Remove a session by id (reporter `session.remove`). Returns the removal
    /// event, or `None` if the session was not present (idempotent).
    pub fn remove_session(&mut self, session_id: &str) -> Option<Event> {
        if self.sessions.remove(session_id).is_some() {
            self.order.retain(|id| id != session_id);
            Some(Event::session_removed(session_id.to_string()))
        } else {
            None
        }
    }

    /// Insert or replace a run within a session (reporter `run.upsert`).
    ///
    /// Recomputes the session rollup and returns **both** the run event
    /// (added/updated) and a `session.updated` reflecting the new rollup, so
    /// faces that track session-level rollups stay correct. Returns an empty
    /// vec if the target session is unknown (no-op).
    pub fn upsert_run(&mut self, session_id: &str, run: AgentRun) -> Vec<Event> {
        let Some(session) = self.sessions.get_mut(session_id) else {
            return Vec::new();
        };
        let run_event =
            if let Some(existing) = session.runs.iter_mut().find(|r| r.run_id == run.run_id) {
                *existing = run.clone();
                Event::run_updated(session_id.to_string(), run)
            } else {
                session.runs.push(run.clone());
                Event::run_added(session_id.to_string(), run)
            };
        Self::recompute_rollups(session);
        let session_clone = session.clone();
        vec![run_event, Event::session_updated(session_clone)]
    }

    /// Remove a run from a session (reporter `run.remove`).
    ///
    /// Recomputes the rollup and returns the run-removal event plus a
    /// `session.updated`. Returns an empty vec if the session or run was absent.
    pub fn remove_run(&mut self, session_id: &str, run_id: &str) -> Vec<Event> {
        let Some(session) = self.sessions.get_mut(session_id) else {
            return Vec::new();
        };
        let before = session.runs.len();
        session.runs.retain(|r| r.run_id != run_id);
        if session.runs.len() == before {
            return Vec::new(); // run wasn't there
        }
        Self::recompute_rollups(session);
        let session_clone = session.clone();
        vec![
            Event::run_removed(session_id.to_string(), run_id.to_string()),
            Event::session_updated(session_clone),
        ]
    }

    /// Validate the rollup invariant for a single session: its stored rollups
    /// equal the recomputed max over its current runs. Used by the property
    /// test; cheap enough to expose for debug assertions too.
    pub fn rollup_holds(session: &Session) -> bool {
        let expected_state = rollup::rollup_state(&session.runs);
        let state_ok = match expected_state {
            // Empty run set: rollup_state is whatever it was last set to; we
            // only assert the non-empty case, where it must equal the max.
            None => true,
            Some(s) => session.rollup_state == s,
        };
        let expected_urgency = match rollup::rollup_urgency(&session.runs) {
            Some(fleet_protocol::Urgency::None) | None => None,
            Some(u) => Some(u),
        };
        state_ok && session.rollup_urgency == expected_urgency
    }

    /// Validate the rollup invariant across all held sessions.
    pub fn all_rollups_hold(&self) -> bool {
        self.sessions.values().all(Self::rollup_holds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::{
        AgentKind, Confidence, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind,
        State, Urgency,
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
    fn sess(id: &str) -> Session {
        Session::new(id, "t", loc(), srv(), State::Idle, "2026-06-08T00:00:00Z")
    }
    fn run_with(id: &str, state: State, urgency: Option<Urgency>) -> AgentRun {
        let mut r = AgentRun::new(
            id,
            AgentKind::Codex,
            "native",
            "/",
            state,
            Confidence::High,
            "2026-06-08T00:00:00Z",
        );
        r.urgency = urgency;
        r
    }

    #[test]
    fn empty_engine_empty_snapshot() {
        let e = MergeEngine::new();
        assert!(e.is_empty());
        assert!(e.snapshot().is_empty());
    }

    #[test]
    fn session_add_then_update() {
        let mut e = MergeEngine::new();
        let ev = e.upsert_session(sess("s1"));
        assert_eq!(ev.type_name(), "session.added");
        let ev = e.upsert_session(sess("s1"));
        assert_eq!(ev.type_name(), "session.updated");
        assert_eq!(e.len(), 1);
    }

    #[test]
    fn session_remove_is_idempotent() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        assert!(e.remove_session("s1").is_some());
        assert!(e.remove_session("s1").is_none());
        assert!(e.is_empty());
    }

    #[test]
    fn run_upsert_recomputes_rollup() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        // Add a working run → rollup_state becomes working.
        let evs = e.upsert_run("s1", run_with("r1", State::Working, None));
        assert_eq!(evs[0].type_name(), "run.added");
        assert_eq!(e.session("s1").unwrap().rollup_state, State::Working);
        // Add a waiting+approval run → rollup escalates.
        e.upsert_run(
            "s1",
            run_with("r2", State::Waiting, Some(Urgency::Approval)),
        );
        let s = e.session("s1").unwrap();
        assert_eq!(s.rollup_state, State::Waiting);
        assert_eq!(s.rollup_urgency, Some(Urgency::Approval));
        assert!(MergeEngine::rollup_holds(s));
    }

    #[test]
    fn run_update_in_place_changes_rollup() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.upsert_run(
            "s1",
            run_with("r1", State::Waiting, Some(Urgency::Approval)),
        );
        // Same run id resolves → working, no urgency.
        let evs = e.upsert_run("s1", run_with("r1", State::Working, None));
        assert_eq!(evs[0].type_name(), "run.updated");
        let s = e.session("s1").unwrap();
        assert_eq!(s.runs.len(), 1, "update is in place, not append");
        assert_eq!(s.rollup_state, State::Working);
        assert_eq!(s.rollup_urgency, None);
    }

    #[test]
    fn run_remove_recomputes() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.upsert_run("s1", run_with("r1", State::Working, None));
        e.upsert_run(
            "s1",
            run_with("r2", State::Waiting, Some(Urgency::Question)),
        );
        // Remove the waiting one → rollup falls back to working.
        let evs = e.remove_run("s1", "r2");
        assert_eq!(evs[0].type_name(), "run.removed");
        let s = e.session("s1").unwrap();
        assert_eq!(s.rollup_state, State::Working);
        assert_eq!(s.rollup_urgency, None);
    }

    #[test]
    fn run_delta_on_unknown_session_is_noop() {
        let mut e = MergeEngine::new();
        assert!(e
            .upsert_run("ghost", run_with("r1", State::Working, None))
            .is_empty());
        assert!(e.remove_run("ghost", "r1").is_empty());
        assert!(e.remove_run("also-ghost", "r2").is_empty());
    }

    #[test]
    fn remove_nonexistent_run_is_noop() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        assert!(e.remove_run("s1", "nope").is_empty());
    }

    #[test]
    fn snapshot_is_insertion_ordered() {
        let mut e = MergeEngine::new();
        for id in ["a", "b", "c"] {
            e.upsert_session(sess(id));
        }
        let ids: Vec<_> = e.snapshot().into_iter().map(|s| s.session_id).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
        // Updating b in place must not reorder.
        e.upsert_session(sess("b"));
        let ids: Vec<_> = e.snapshot().into_iter().map(|s| s.session_id).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
        // Removing b drops it from order.
        e.remove_session("b");
        let ids: Vec<_> = e.snapshot().into_iter().map(|s| s.session_id).collect();
        assert_eq!(ids, vec!["a", "c"]);
    }

    #[test]
    fn upsert_session_with_runs_rolls_up_immediately() {
        let mut e = MergeEngine::new();
        let mut s = sess("s1");
        s.runs
            .push(run_with("r1", State::Waiting, Some(Urgency::Approval)));
        s.runs.push(run_with("r2", State::Working, None));
        e.upsert_session(s);
        let s = e.session("s1").unwrap();
        assert_eq!(s.rollup_state, State::Waiting);
        assert_eq!(s.rollup_urgency, Some(Urgency::Approval));
        assert!(MergeEngine::rollup_holds(s));
    }
}
