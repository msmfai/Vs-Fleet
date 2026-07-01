//! The canonical merge engine (README §4.3, §7.4; the engineering spec).
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
//! ordering in [`fleet_protocol::rollup`]. This is the one invariant the test
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

use fleet_protocol::{rollup, AgentRun, Event, Session, State};
use std::collections::{HashMap, HashSet};

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
    /// An empty session (no runs) is reset to the **idle sentinel** ([`State::Idle`])
    /// and cleared of `rollup_urgency` — there is nothing to roll up. Resetting
    /// (rather than leaving the last computed rollup in place) is a correctness
    /// requirement: removing the last run of a `Waiting` session must not leave a
    /// stale `Waiting` rollup, which would keep [`Self::should_notify_session`]
    /// arming an `unread` badge for a session with zero runs. The notify-transition
    /// bookkeeping in the callers then clears that now-stale `unread`.
    fn recompute_rollups(session: &mut Session) {
        // Empty run set → idle sentinel; otherwise the most-urgent run state.
        session.rollup_state = rollup::rollup_state(&session.runs).unwrap_or(State::Idle);
        // rollup_urgency is None for an empty run set, and the most-urgent
        // (possibly Urgency::None) otherwise. We normalize Urgency::None → None
        // on the optional field so the wire shows absence rather than "null".
        session.rollup_urgency = match rollup::rollup_urgency(&session.runs) {
            Some(fleet_protocol::Urgency::None) | None => None,
            Some(u) => Some(u),
        };
    }

    fn any_soloed(&self) -> bool {
        self.sessions.values().any(|session| session.soloed)
    }

    fn should_notify_session(session: &Session, any_soloed: bool) -> bool {
        session.rollup_state == State::Waiting && !session.muted && (!any_soloed || session.soloed)
    }

    fn update_unread_for_notify_transition(
        session: &mut Session,
        old_notify: bool,
        new_notify: bool,
    ) -> bool {
        if !old_notify && new_notify && !session.unread {
            session.unread = true;
            true
        } else if old_notify && !new_notify && session.unread {
            session.unread = false;
            true
        } else {
            false
        }
    }

    fn notify_map(&self) -> HashMap<String, bool> {
        let any_soloed = self.any_soloed();
        self.sessions
            .iter()
            .map(|(id, session)| (id.clone(), Self::should_notify_session(session, any_soloed)))
            .collect()
    }

    fn reconcile_unread_from(&mut self, old_notify: &HashMap<String, bool>) -> HashSet<String> {
        let any_soloed = self.any_soloed();
        let mut changed = HashSet::new();
        for id in self.order.clone() {
            let old = old_notify.get(&id).copied().unwrap_or(false);
            if self.reconcile_one_unread(&id, old, any_soloed) {
                changed.insert(id);
            }
        }
        changed
    }

    /// Recompute one session's unread flag for a notify transition; returns
    /// whether it changed.
    ///
    /// Coverage: `id` always comes from `self.order`, which is kept in lock-step
    /// with `self.sessions`, so the `let-else` (a missing session) is unreachable
    /// defensive code. Excluded from the nightly gate; a no-op on stable.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn reconcile_one_unread(&mut self, id: &str, old: bool, any_soloed: bool) -> bool {
        let Some(session) = self.sessions.get_mut(id) else {
            return false;
        };
        let new = Self::should_notify_session(session, any_soloed);
        Self::update_unread_for_notify_transition(session, old, new)
    }

    fn updated_events_for(&self, changed_ids: HashSet<String>) -> Vec<Event> {
        self.order
            .iter()
            .filter(|id| changed_ids.contains(*id))
            .filter_map(|id| self.sessions.get(id).cloned())
            .map(Event::session_updated)
            .collect()
    }

    /// Insert or replace a whole session (reporter `session.upsert`).
    ///
    /// On upsert the session's rollups are recomputed from whatever runs it
    /// carries, so a reporter that ships a session + runs in one object still
    /// gets a correct rollup. Returns the outbound event (added vs updated).
    pub fn upsert_session(&mut self, mut session: Session) -> Event {
        let any_soloed_before = self.any_soloed();
        let old_notify = self
            .sessions
            .get(&session.session_id)
            .map(|existing| Self::should_notify_session(existing, any_soloed_before))
            .unwrap_or(false);
        Self::recompute_rollups(&mut session);
        let new_notify = Self::should_notify_session(&session, any_soloed_before);
        Self::update_unread_for_notify_transition(&mut session, old_notify, new_notify);
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
    /// event plus any remaining sessions whose unread state changed because the
    /// removal changed the mute/solo notification set. Empty if absent.
    pub fn remove_session_events(&mut self, session_id: &str) -> Vec<Event> {
        if !self.sessions.contains_key(session_id) {
            return Vec::new();
        }

        let old_notify = self.notify_map();
        self.sessions.remove(session_id);
        self.order.retain(|id| id != session_id);

        let mut events = vec![Event::session_removed(session_id.to_string())];
        let changed_ids = self.reconcile_unread_from(&old_notify);
        events.extend(self.updated_events_for(changed_ids));
        events
    }

    /// Backward-compatible convenience for callers that only care whether the
    /// target existed. State reconciliation still happens; additional update
    /// events are intentionally discarded by this narrow helper.
    pub fn remove_session(&mut self, session_id: &str) -> Option<Event> {
        self.remove_session_events(session_id).into_iter().next()
    }

    /// Insert or replace a run within a session (reporter `run.upsert`).
    ///
    /// Recomputes the session rollup and returns **both** the run event
    /// (added/updated) and a `session.updated` reflecting the new rollup, so
    /// faces that track session-level rollups stay correct. Returns an empty
    /// vec if the target session is unknown (no-op).
    pub fn upsert_run(&mut self, session_id: &str, run: AgentRun) -> Vec<Event> {
        let any_soloed = self.any_soloed();
        let Some(session) = self.sessions.get_mut(session_id) else {
            return Vec::new();
        };
        let old_notify = Self::should_notify_session(session, any_soloed);
        let run_event =
            if let Some(existing) = session.runs.iter_mut().find(|r| r.run_id == run.run_id) {
                *existing = run.clone();
                Event::run_updated(session_id.to_string(), run)
            } else {
                session.runs.push(run.clone());
                Event::run_added(session_id.to_string(), run)
            };
        Self::recompute_rollups(session);
        let new_notify = Self::should_notify_session(session, any_soloed);
        Self::update_unread_for_notify_transition(session, old_notify, new_notify);
        let session_clone = session.clone();
        vec![run_event, Event::session_updated(session_clone)]
    }

    /// Remove a run from a session (reporter `run.remove`).
    ///
    /// Recomputes the rollup and returns the run-removal event plus a
    /// `session.updated`. Returns an empty vec if the session or run was absent.
    pub fn remove_run(&mut self, session_id: &str, run_id: &str) -> Vec<Event> {
        let any_soloed = self.any_soloed();
        let Some(session) = self.sessions.get_mut(session_id) else {
            return Vec::new();
        };
        let old_notify = Self::should_notify_session(session, any_soloed);
        let before = session.runs.len();
        session.runs.retain(|r| r.run_id != run_id);
        if session.runs.len() == before {
            return Vec::new(); // run wasn't there
        }
        Self::recompute_rollups(session);
        let new_notify = Self::should_notify_session(session, any_soloed);
        Self::update_unread_for_notify_transition(session, old_notify, new_notify);
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

    /// Set `muted = true` on a session. If the session was soloed, muting it
    /// also clears `soloed` so the inbox leaves solo mode. Returns every
    /// `session.updated` caused by flag or unread changes. Empty if absent or
    /// already muted and not soloed.
    ///
    /// Muting silences pings for the session without removing it from the inbox
    /// (README §15.4 / the engineering spec). State is still visible; only notifications are
    /// suppressed.
    pub fn apply_mute(&mut self, session_id: &str) -> Vec<Event> {
        // `notify_map` borrows `self` immutably and must run before the mutable
        // `get_mut`; the `let-else` handles an absent session (the no-op return),
        // so no separate presence pre-check is needed.
        let old_notify = self.notify_map();
        let mut changed_ids = HashSet::new();

        let Some(sess) = self.sessions.get_mut(session_id) else {
            return Vec::new();
        };
        if sess.muted && !sess.soloed {
            return Vec::new(); // already muted, no change
        }
        sess.muted = true;
        sess.soloed = false;
        changed_ids.insert(session_id.to_string());

        changed_ids.extend(self.reconcile_unread_from(&old_notify));
        self.updated_events_for(changed_ids)
    }

    /// Set `muted = false` on a session. Returns every `session.updated` caused
    /// by flag or unread changes. Empty if absent or already unmuted and not
    /// soloed.
    pub fn apply_unmute(&mut self, session_id: &str) -> Vec<Event> {
        // See `apply_mute`: the `let-else` is the absent-session no-op path, so no
        // separate `contains_key` pre-check is needed.
        let old_notify = self.notify_map();
        let mut changed_ids = HashSet::new();

        let Some(sess) = self.sessions.get_mut(session_id) else {
            return Vec::new();
        };
        if !sess.muted && !sess.soloed {
            return Vec::new(); // already unmuted, no change
        }
        sess.muted = false;
        sess.soloed = false;
        changed_ids.insert(session_id.to_string());

        changed_ids.extend(self.reconcile_unread_from(&old_notify));
        self.updated_events_for(changed_ids)
    }

    /// Solo a session: set `soloed = true` on `session_id` and `soloed = false`
    /// on every other session in the inbox. Returns the broadcast events — one
    /// `session.updated` for the soloed session plus one for each session whose
    /// `soloed` flag had to be cleared.
    ///
    /// If `session_id` is not found, returns an empty vec (no-op).
    ///
    /// Semantics: exactly one session is soloed at a time. Sending a `solo` for
    /// a second session atomically moves the solo. The soloed session is also
    /// unmuted so it can ping. Sending a `solo` for the already-soloed unmuted
    /// session is a no-op (idempotent).
    pub fn apply_solo(&mut self, session_id: &str) -> Vec<Event> {
        if !self.sessions.contains_key(session_id) {
            return Vec::new();
        }

        let old_notify = self.notify_map();
        let mut changed_ids = HashSet::new();

        // First pass: clear the solo flag on every other session.
        for id in self.order.clone() {
            if id != session_id && self.clear_solo_flag(&id) {
                changed_ids.insert(id);
            }
        }

        // Second pass: set solo on the target.
        if self.set_solo_flag(session_id) {
            changed_ids.insert(session_id.to_string());
        }

        changed_ids.extend(self.reconcile_unread_from(&old_notify));
        self.updated_events_for(changed_ids)
    }

    /// Clear the `soloed` flag on `id`, returning whether it changed.
    ///
    /// Coverage: `id` comes from `self.order` (lock-step with `self.sessions`), so
    /// the `get_mut` `None` arm is unreachable defensive code. Excluded from the
    /// nightly gate; a no-op on stable.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn clear_solo_flag(&mut self, id: &str) -> bool {
        match self.sessions.get_mut(id) {
            Some(s) if s.soloed => {
                s.soloed = false;
                true
            }
            _ => false,
        }
    }

    /// Set `soloed` (and clear `muted`) on the target, returning whether it
    /// changed.
    ///
    /// Coverage: the caller's `contains_key` guard guarantees the session is
    /// present, so the `get_mut` `None` arm is unreachable. Excluded from the
    /// nightly gate; a no-op on stable.
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn set_solo_flag(&mut self, session_id: &str) -> bool {
        match self.sessions.get_mut(session_id) {
            Some(s) if !s.soloed || s.muted => {
                s.soloed = true;
                s.muted = false;
                true
            }
            _ => false,
        }
    }

    /// Mark a focused session as read. This is the Hub-side effect of
    /// `Command::focus`: focusing is an acknowledgement of the unread ping, but
    /// it must not fabricate progress by changing the session state.
    pub fn apply_focus(&mut self, session_id: &str) -> Option<Event> {
        let sess = self.sessions.get_mut(session_id)?;
        if !sess.unread {
            return None;
        }
        sess.unread = false;
        Some(Event::session_updated(sess.clone()))
    }

    /// Replace the entire in-memory session set with `sessions` verbatim (no
    /// rollup/unread recomputation), preserving their given order.
    ///
    /// Used by the durable [`StateStore`](crate::persist::StateStore) to **roll a
    /// projection back** to a captured pre-mutation snapshot when the durable log
    /// append for that mutation fails, so the in-memory state can never diverge
    /// from the log (the persistence invariant). Because it restores exact prior
    /// `Session` objects, no derived field is re-derived or perturbed.
    pub(crate) fn restore(&mut self, sessions: Vec<Session>) {
        self.sessions.clear();
        self.order.clear();
        for session in sessions {
            self.order.push(session.session_id.clone());
            self.sessions.insert(session.session_id.clone(), session);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::{
        AgentKind, Confidence, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind,
        State, Urgency,
    };

    /// The session id of a `session.updated` event (panicking on any other kind).
    /// The mismatch / `_ => None` arm is unreachable in the passing tests;
    /// excluded from the nightly gate so it never shows as uncovered (no-op on
    /// stable).
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn updated_session_id(ev: &Event) -> &str {
        match ev {
            Event::SessionUpdated { session, .. } => session.session_id.as_str(),
            other => panic!("expected session.updated, got {other:?}"),
        }
    }

    /// The session ids of all `session.updated` events in order (others skipped).
    /// Excluded from the nightly gate — see [`updated_session_id`].
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn updated_session_ids(evs: &[Event]) -> Vec<&str> {
        evs.iter()
            .filter_map(|ev| match ev {
                Event::SessionUpdated { session, .. } => Some(session.session_id.as_str()),
                _ => None,
            })
            .collect()
    }

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
    fn removing_soloed_session_rearms_other_waiting_unread() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("solo"));
        e.upsert_session(sess("other"));
        e.upsert_run(
            "solo",
            run_with("r1", State::Waiting, Some(Urgency::Approval)),
        );
        e.upsert_run(
            "other",
            run_with("r2", State::Waiting, Some(Urgency::Question)),
        );
        e.apply_focus("solo");
        e.apply_focus("other");
        e.apply_solo("solo");

        let evs = e.remove_session_events("solo");

        assert!(e.session("solo").is_none());
        assert!(e.session("other").unwrap().unread);
        let names: Vec<_> = evs.iter().map(Event::type_name).collect();
        assert_eq!(names, vec!["session.removed", "session.updated"]);
        assert_eq!(updated_session_id(&evs[1]), "other");
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
    fn waiting_transition_marks_session_unread() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));

        e.upsert_run(
            "s1",
            run_with("r1", State::Waiting, Some(Urgency::Approval)),
        );

        assert!(e.session("s1").unwrap().unread);
    }

    #[test]
    fn waiting_resolution_clears_session_unread() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.upsert_run(
            "s1",
            run_with("r1", State::Waiting, Some(Urgency::Approval)),
        );
        assert!(e.session("s1").unwrap().unread);

        e.upsert_run("s1", run_with("r1", State::Working, None));

        assert!(!e.session("s1").unwrap().unread);
    }

    #[test]
    fn focused_waiting_session_is_not_rearmed_by_same_waiting_update() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.upsert_run(
            "s1",
            run_with("r1", State::Waiting, Some(Urgency::Approval)),
        );
        e.apply_focus("s1");
        assert!(!e.session("s1").unwrap().unread);

        e.upsert_run(
            "s1",
            run_with("r1", State::Waiting, Some(Urgency::Approval)),
        );

        assert!(!e.session("s1").unwrap().unread);
    }

    #[test]
    fn mute_clears_waiting_unread_and_unmute_rearms_it() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.upsert_run(
            "s1",
            run_with("r1", State::Waiting, Some(Urgency::Question)),
        );
        assert!(e.session("s1").unwrap().unread);

        e.apply_mute("s1");
        assert!(!e.session("s1").unwrap().unread);

        e.apply_unmute("s1");
        assert!(e.session("s1").unwrap().unread);
    }

    #[test]
    fn removing_last_run_resets_rollup_to_idle_and_clears_unread() {
        // T1.6 regression: a session with a single Waiting run is `unread`; removing
        // that last run must reset the rollup to the idle sentinel AND clear the now
        // stale `unread` badge (a zero-run session must not keep pinging).
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.upsert_run(
            "s1",
            run_with("r1", State::Waiting, Some(Urgency::Approval)),
        );
        assert!(e.session("s1").unwrap().unread, "waiting run arms unread");
        assert_eq!(e.session("s1").unwrap().rollup_state, State::Waiting);

        e.remove_run("s1", "r1");

        let s = e.session("s1").unwrap();
        assert!(s.runs.is_empty(), "last run removed");
        assert_eq!(
            s.rollup_state,
            State::Idle,
            "empty session resets to the idle sentinel, not a stale Waiting"
        );
        assert_eq!(s.rollup_urgency, None, "empty session has no rollup urgency");
        assert!(
            !s.unread,
            "unread must clear once the last waiting run is gone"
        );
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

    // ── mute / unmute / solo (the engineering spec) ─────────────────────────────────────

    #[test]
    fn mute_sets_flag_and_emits_updated() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        assert!(!e.session("s1").unwrap().muted);
        let evs = e.apply_mute("s1");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].type_name(), "session.updated");
        assert!(e.session("s1").unwrap().muted);
    }

    #[test]
    fn mute_is_idempotent() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.apply_mute("s1");
        // Second mute is a no-op.
        assert!(e.apply_mute("s1").is_empty(), "second mute must be a no-op");
        assert!(e.session("s1").unwrap().muted);
    }

    #[test]
    fn mute_on_absent_session_is_none() {
        let mut e = MergeEngine::new();
        assert!(e.apply_mute("ghost").is_empty());
    }

    #[test]
    fn unmute_on_absent_session_is_none() {
        // Exercises the `let-else` absent-session no-op path of apply_unmute.
        let mut e = MergeEngine::new();
        assert!(e.apply_unmute("ghost").is_empty());
    }

    #[test]
    fn focus_clears_unread_and_keeps_state() {
        let mut e = MergeEngine::new();
        // A genuine waiting run gives the session a legitimate Waiting rollup and
        // arms `unread` (a run-less session is normalized to the idle sentinel).
        let mut s = sess("s1");
        s.runs
            .push(run_with("r1", State::Waiting, Some(Urgency::Approval)));
        e.upsert_session(s);
        assert!(e.session("s1").unwrap().unread);

        let ev = e.apply_focus("s1").expect("focus on unread session");
        assert_eq!(ev.type_name(), "session.updated");
        let s = e.session("s1").unwrap();
        assert!(!s.unread);
        assert_eq!(
            s.rollup_state,
            State::Waiting,
            "focus acknowledges the ping without changing run-derived state"
        );
    }

    #[test]
    fn focus_is_idempotent_for_read_or_absent_session() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        assert!(e.apply_focus("s1").is_none());
        assert!(e.apply_focus("ghost").is_none());
    }

    #[test]
    fn unmute_clears_flag_and_emits_updated() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.apply_mute("s1");
        let evs = e.apply_unmute("s1");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].type_name(), "session.updated");
        assert!(!e.session("s1").unwrap().muted);
    }

    #[test]
    fn unmute_is_idempotent() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        // Already unmuted; second unmute is a no-op.
        assert!(
            e.apply_unmute("s1").is_empty(),
            "unmute of already-unmuted must be no-op"
        );
    }

    #[test]
    fn mute_does_not_change_state_or_rollup() {
        // Muting must only flip the flag — rollup and state are unaffected.
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.upsert_run("s1", run_with("r1", State::Working, None));
        let before = e.session("s1").unwrap().rollup_state;
        e.apply_mute("s1");
        let after = e.session("s1").unwrap().rollup_state;
        assert_eq!(before, after, "mute must not change rollup_state");
        assert!(e.session("s1").unwrap().muted);
    }

    #[test]
    fn mute_clears_soloed_flag() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.apply_solo("s1");
        assert!(e.session("s1").unwrap().soloed);

        let evs = e.apply_mute("s1");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].type_name(), "session.updated");
        let s = e.session("s1").unwrap();
        assert!(s.muted);
        assert!(!s.soloed);
    }

    #[test]
    fn unmute_clears_soloed_even_when_not_muted() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.apply_solo("s1");
        assert!(e.session("s1").unwrap().soloed);
        assert!(!e.session("s1").unwrap().muted);

        let evs = e.apply_unmute("s1");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].type_name(), "session.updated");
        let s = e.session("s1").unwrap();
        assert!(!s.muted);
        assert!(!s.soloed);
    }

    #[test]
    fn muting_soloed_waiting_session_rearms_other_waiting_unread() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.upsert_session(sess("s2"));
        e.upsert_run(
            "s1",
            run_with("r1", State::Waiting, Some(Urgency::Approval)),
        );
        e.upsert_run(
            "s2",
            run_with("r2", State::Waiting, Some(Urgency::Question)),
        );
        e.apply_focus("s1");
        e.apply_focus("s2");
        assert!(!e.session("s1").unwrap().unread);
        assert!(!e.session("s2").unwrap().unread);

        e.apply_solo("s1");
        assert!(!e.session("s1").unwrap().unread);
        assert!(!e.session("s2").unwrap().unread);

        let evs = e.apply_mute("s1");

        assert!(e.session("s1").unwrap().muted);
        assert!(!e.session("s1").unwrap().soloed);
        assert!(!e.session("s1").unwrap().unread);
        assert!(e.session("s2").unwrap().unread);
        assert_eq!(updated_session_ids(&evs), vec!["s1", "s2"]);
    }

    #[test]
    fn solo_sets_flag_on_target_and_clears_others() {
        let mut e = MergeEngine::new();
        for id in ["s1", "s2", "s3"] {
            e.upsert_session(sess(id));
        }
        let evs = e.apply_solo("s2");
        // s2 gets a session.updated (soloed=true).
        assert!(!evs.is_empty());
        assert!(e.session("s2").unwrap().soloed);
        assert!(!e.session("s1").unwrap().soloed);
        assert!(!e.session("s3").unwrap().soloed);
        // The events must all be session.updated.
        for ev in &evs {
            assert_eq!(ev.type_name(), "session.updated");
        }
    }

    #[test]
    fn solo_on_absent_session_is_empty() {
        let mut e = MergeEngine::new();
        assert!(e.apply_solo("ghost").is_empty());
    }

    #[test]
    fn solo_is_idempotent() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        let evs1 = e.apply_solo("s1");
        assert!(!evs1.is_empty()); // first solo emits an event
        let evs2 = e.apply_solo("s1");
        assert!(
            evs2.is_empty(),
            "second solo must be a no-op (already soloed)"
        );
        assert!(e.session("s1").unwrap().soloed);
    }

    #[test]
    fn solo_unmutes_target() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("s1"));
        e.apply_mute("s1");
        assert!(e.session("s1").unwrap().muted);

        let evs = e.apply_solo("s1");
        assert_eq!(evs.len(), 1);
        let s = e.session("s1").unwrap();
        assert!(s.soloed);
        assert!(!s.muted);
    }

    #[test]
    fn solo_moves_from_one_session_to_another() {
        let mut e = MergeEngine::new();
        for id in ["a", "b"] {
            e.upsert_session(sess(id));
        }
        // Solo a, then solo b.
        e.apply_solo("a");
        assert!(e.session("a").unwrap().soloed);
        let evs = e.apply_solo("b");
        // a should have soloed cleared; b should have it set.
        assert!(!e.session("a").unwrap().soloed);
        assert!(e.session("b").unwrap().soloed);
        // Events contain both a (cleared) and b (set).
        assert_eq!(evs.len(), 2);
    }

    #[test]
    fn solo_single_session_inbox() {
        let mut e = MergeEngine::new();
        e.upsert_session(sess("solo"));
        let evs = e.apply_solo("solo");
        assert_eq!(evs.len(), 1);
        assert!(e.session("solo").unwrap().soloed);
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
