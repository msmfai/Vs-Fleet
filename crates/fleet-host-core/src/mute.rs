//! Mute / solo command derivation — slice S25 (node `MUTE`).
//!
//! S25 lets the inbox **mute** a session (state still shown, pings silenced) or
//! **solo** it (mute all others), consistent across CLI + GUI (PLAN S25,
//! WORK_GRAPH §3 gate `◆G3`: "mute/solo command round-trips"). The Hub owns the
//! authoritative `muted`/`soloed` flags (they live on [`fleet_protocol::Session`])
//! and broadcasts a `session.updated` whenever they change; faces only issue the
//! command and reflect the echoed state.
//!
//! ## Design
//!
//! ### What mute silences
//!
//! "Mute silences a session" means its pings (notifications, badge, attention
//! signal) are suppressed. **State is still visible** — a muted waiting session
//! still shows `⏸` in the list; it just never fires a desktop notification or
//! raises the inbox badge (PLAN S25, README §15.4). This is a *display*
//! decision, not a protocol decision: the Hub stores the flag; the face decides
//! whether to fire a ping.
//!
//! ### Solo
//!
//! Solo is *inverse* mute: the soloed session is un-muted and all others are
//! treated as muted. Exactly one session can be soloed at a time; soloing a
//! second session replaces the first (the Hub keeps a single `soloed` bit per
//! session and clears the others when a new solo arrives). Un-soloing (sending
//! a `mute` or `unmute` to the current soloed session) restores normal state.
//!
//! ### The ping-suppression predicate
//!
//! [`ping_suppressed`] is the pure function a face calls to decide whether to
//! fire a ping for a given tab. It encodes the full contract:
//! - If **any** session is soloed, only the soloed session pings; all others are
//!   suppressed.
//! - Otherwise a session whose own `muted` flag is set is suppressed.
//! - Non-waiting tabs never ping (state is not `waiting`).
//!
//! [`should_notify`] is the higher-level entry point that combines
//! `is_attention` (state is waiting) with `ping_suppressed`.
//!
//! ### Command helpers
//!
//! [`mute_command`], [`unmute_command`], and [`solo_command`] are thin wrappers
//! that construct the correct [`fleet_protocol::Command`] from a tab click,
//! factored here (not in the window code) so they are unit-testable and
//! consistent across CLI + GUI.
//!
//! ## Unit tests (mandatory per the node brief and `◆G3`)
//!
//! - Mute hides pings but not state.
//! - Solo inverts: only the soloed session pings; all others are suppressed.
//! - Round-trip through the command model: `Command::mute` / `unmute` / `solo`
//!   serialize correctly and carry the right `session_id`.

use fleet_protocol::{Command, SCHEMA_VERSION};

use crate::SessionTab;

// ── Ping-suppression predicate ────────────────────────────────────────────────

/// Return `true` if the ping for `tab` should be **suppressed** given the full
/// current tab list.
///
/// The rules (in order):
///
/// 1. The tab is not in the `waiting` state — only waiting tabs ever ping.
/// 2. Some other session in the inbox is soloed — when there is a solo, all
///    non-soloed sessions are silenced regardless of their own `muted` flag.
/// 3. The tab's own `muted` flag is set (and no solo is active).
///
/// `tabs` must be the **full** current inbox (not a subset), because the solo
/// check needs to know whether *any* session is soloed.
pub fn ping_suppressed(tab: &SessionTab, tabs: &[SessionTab]) -> bool {
    // Rule 1: non-waiting tabs never ping.
    if !tab.state.is_attention() {
        return true;
    }

    // Check if any session in the inbox is currently soloed.
    let any_soloed = tabs.iter().any(|t| t.soloed);

    if any_soloed {
        // Rule 2: a solo is active → only the soloed session pings.
        // The tab pings iff it is the soloed one; everyone else is suppressed.
        !tab.soloed
    } else {
        // Rule 3: no solo → each session's own muted flag governs.
        tab.muted
    }
}

/// Return `true` if a face should fire a ping (notification / badge) for `tab`.
///
/// This is the **affirmative** version of [`ping_suppressed`]: the tab must be
/// in the waiting state (the only attention-demanding state, README §7.3) *and*
/// its ping must not be suppressed.
pub fn should_notify(tab: &SessionTab, tabs: &[SessionTab]) -> bool {
    tab.state.is_attention() && !ping_suppressed(tab, tabs)
}

// ── Command constructors (view-model → protocol command) ─────────────────────

/// Construct the [`Command::Mute`] a face sends to the Hub when the user mutes
/// `session_id`.
pub fn mute_command(session_id: impl Into<String>) -> Command {
    Command::Mute {
        schema_version: SCHEMA_VERSION,
        session_id: session_id.into(),
    }
}

/// Construct the [`Command::Unmute`] a face sends when the user un-mutes
/// `session_id`.
pub fn unmute_command(session_id: impl Into<String>) -> Command {
    Command::Unmute {
        schema_version: SCHEMA_VERSION,
        session_id: session_id.into(),
    }
}

/// Construct the [`Command::Solo`] a face sends when the user solos
/// `session_id` (muting all others).
pub fn solo_command(session_id: impl Into<String>) -> Command {
    Command::Solo {
        schema_version: SCHEMA_VERSION,
        session_id: session_id.into(),
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentIcon, SessionTab, TabState};
    use fleet_protocol::{Confidence, LocationGlyph, Urgency};

    // ── Helpers ────────────────────────────────────────────────────────────────

    fn waiting_tab(id: &str, muted: bool, soloed: bool) -> SessionTab {
        SessionTab {
            session_id: id.into(),
            glyph: LocationGlyph::Laptop,
            agent_icon: AgentIcon::None,
            title: id.into(),
            state: TabState::Waiting,
            urgency: Some(Urgency::Approval),
            confidence: Some(Confidence::High),
            waiting_since: None,
            muted,
            soloed,
            unread: false,
            run_count: 1,
        }
    }

    fn idle_tab(id: &str, muted: bool) -> SessionTab {
        SessionTab {
            session_id: id.into(),
            glyph: LocationGlyph::Laptop,
            agent_icon: AgentIcon::None,
            title: id.into(),
            state: TabState::Idle,
            urgency: None,
            confidence: None,
            waiting_since: None,
            muted,
            soloed: false,
            unread: false,
            run_count: 0,
        }
    }

    fn working_tab(id: &str) -> SessionTab {
        SessionTab {
            session_id: id.into(),
            glyph: LocationGlyph::Laptop,
            agent_icon: AgentIcon::None,
            title: id.into(),
            state: TabState::Working,
            urgency: None,
            confidence: None,
            waiting_since: None,
            muted: false,
            soloed: false,
            unread: false,
            run_count: 1,
        }
    }

    // ── ping_suppressed: basic mute ───────────────────────────────────────────

    /// A waiting session with muted=false must NOT be suppressed (should notify).
    #[test]
    fn unmuted_waiting_session_is_not_suppressed() {
        let tab = waiting_tab("s1", false, false);
        let tabs = vec![tab.clone()];
        assert!(!ping_suppressed(&tab, &tabs));
        assert!(should_notify(&tab, &tabs));
    }

    /// Mute silences a waiting session's ping (state remains visible, but pings
    /// are suppressed).
    #[test]
    fn muted_waiting_session_is_suppressed() {
        let tab = waiting_tab("s1", /*muted=*/ true, false);
        let tabs = vec![tab.clone()];
        assert!(ping_suppressed(&tab, &tabs));
        assert!(!should_notify(&tab, &tabs));
    }

    /// Mute only hides pings, not state — the tab still reports waiting state.
    #[test]
    fn mute_hides_pings_not_state() {
        let tab = waiting_tab("s1", /*muted=*/ true, false);
        // State is still visible (TabState::Waiting) even though ping is suppressed.
        assert_eq!(tab.state, TabState::Waiting);
        assert!(tab.state.is_attention());
        // But the ping is suppressed.
        let tabs = vec![tab.clone()];
        assert!(ping_suppressed(&tab, &tabs));
        assert!(!should_notify(&tab, &tabs));
    }

    /// A non-waiting tab is never notifiable regardless of mute flag.
    #[test]
    fn non_waiting_tabs_never_ping_regardless_of_mute() {
        // Idle, working, done, dead — none should ping.
        for state in [
            TabState::Idle,
            TabState::Working,
            TabState::Done,
            TabState::Dead,
            TabState::Error,
        ] {
            let tab = SessionTab {
                session_id: "s".into(),
                glyph: LocationGlyph::Laptop,
                agent_icon: AgentIcon::None,
                title: "s".into(),
                state,
                urgency: None,
                confidence: None,
                waiting_since: None,
                muted: false, // even unmuted
                soloed: false,
                unread: false,
                run_count: 0,
            };
            let tabs = vec![tab.clone()];
            assert!(
                ping_suppressed(&tab, &tabs),
                "state={state:?} must always be suppressed (never pings)"
            );
            assert!(
                !should_notify(&tab, &tabs),
                "should_notify must be false for non-waiting state={state:?}"
            );
        }
    }

    // ── ping_suppressed: solo inverts ─────────────────────────────────────────

    /// Solo: when one session is soloed, all others are suppressed and the
    /// soloed session's own ping fires (even if it is also marked muted — the
    /// solo wins).
    #[test]
    fn solo_inverts_mute_only_soloed_session_pings() {
        // s1 is soloed (and waiting), s2 is not (also waiting).
        let s1 = waiting_tab("s1", /*muted=*/ false, /*soloed=*/ true);
        let s2 = waiting_tab("s2", /*muted=*/ false, /*soloed=*/ false);
        let tabs = vec![s1.clone(), s2.clone()];

        // s1 (soloed) should NOT be suppressed.
        assert!(!ping_suppressed(&s1, &tabs));
        assert!(should_notify(&s1, &tabs));

        // s2 (non-soloed) should be suppressed by the solo.
        assert!(ping_suppressed(&s2, &tabs));
        assert!(!should_notify(&s2, &tabs));
    }

    /// When a solo is active, even an un-muted session is suppressed if it is
    /// not the soloed one.
    #[test]
    fn solo_suppresses_unmuted_sessions() {
        let soloed = waiting_tab("solo", false, /*soloed=*/ true);
        let unmuted_other = waiting_tab("other", /*muted=*/ false, false);
        let tabs = vec![soloed.clone(), unmuted_other.clone()];

        assert!(!ping_suppressed(&soloed, &tabs), "soloed tab pings");
        assert!(
            ping_suppressed(&unmuted_other, &tabs),
            "unmuted-but-non-solo is suppressed"
        );
    }

    /// Multiple non-soloed waiting sessions in the inbox — all are suppressed
    /// when exactly one is soloed.
    #[test]
    fn solo_suppresses_all_non_soloed_even_multiple() {
        let s1 = waiting_tab("s1", false, /*soloed=*/ true);
        let s2 = waiting_tab("s2", false, false);
        let s3 = waiting_tab("s3", false, false);
        let s4 = waiting_tab("s4", false, false);
        let tabs = vec![s1.clone(), s2.clone(), s3.clone(), s4.clone()];

        // Only s1 should notify.
        assert!(should_notify(&s1, &tabs));
        assert!(!should_notify(&s2, &tabs));
        assert!(!should_notify(&s3, &tabs));
        assert!(!should_notify(&s4, &tabs));
    }

    /// No session is soloed → normal mute rules apply; unmuted sessions ping,
    /// muted ones are suppressed.
    #[test]
    fn no_solo_falls_through_to_mute_rules() {
        let unmuted = waiting_tab("a", /*muted=*/ false, false);
        let muted = waiting_tab("b", /*muted=*/ true, false);
        let tabs = vec![unmuted.clone(), muted.clone()];

        assert!(should_notify(&unmuted, &tabs));
        assert!(!should_notify(&muted, &tabs));
    }

    /// Solo with a muted soloed session: the solo overrides the mute. The soloed
    /// session pings even if it was also explicitly muted.
    #[test]
    fn solo_overrides_mute_on_soloed_session() {
        // s1 is both muted AND soloed — solo wins, it still pings.
        let s1 = waiting_tab("s1", /*muted=*/ true, /*soloed=*/ true);
        let s2 = waiting_tab("s2", false, false);
        let tabs = vec![s1.clone(), s2.clone()];

        // The solo overrides the mute on s1.
        assert!(
            should_notify(&s1, &tabs),
            "solo overrides mute on soloed tab"
        );
        // s2 is still suppressed.
        assert!(!should_notify(&s2, &tabs));
    }

    // ── edge cases ────────────────────────────────────────────────────────────

    /// An inbox with only non-waiting sessions never fires any ping.
    #[test]
    fn all_idle_inbox_no_pings() {
        let tabs = vec![idle_tab("a", false), idle_tab("b", false), working_tab("c")];
        for tab in &tabs {
            assert!(!should_notify(tab, &tabs));
        }
    }

    /// Empty inbox: suppression gracefully returns true (no tab should notify).
    #[test]
    fn empty_inbox_suppressed() {
        let tab = waiting_tab("s1", false, false);
        // Pass an empty slice as the "full inbox" — no solo anywhere, but the
        // muted=false/soloed=false tab is still the one being asked about.
        // Note: in practice the full inbox always contains the tab being queried.
        // With an empty inbox, no solo is active, so mute=false → not suppressed
        // by rule 3; ping_suppressed should return false.
        let empty: Vec<SessionTab> = vec![];
        assert!(!ping_suppressed(&tab, &empty));
    }

    // ── command round-trip through the protocol model ─────────────────────────

    /// `mute_command` produces a wire-correct `Command::Mute` for the right
    /// session id and the current schema version.
    #[test]
    fn mute_command_round_trip() {
        let cmd = mute_command("my-session");
        // It is the right variant.
        assert!(matches!(cmd, Command::Mute { .. }));
        // The command name matches the wire tag.
        assert_eq!(cmd.command_name(), "mute");
        // Wire serialization is correct.
        let v = serde_json::to_value(&cmd).unwrap();
        assert_eq!(v["command"], "mute");
        assert_eq!(v["session_id"], "my-session");
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
        // And it round-trips back.
        let back: Command = serde_json::from_value(v).unwrap();
        assert_eq!(back, cmd);
    }

    /// `unmute_command` round-trip.
    #[test]
    fn unmute_command_round_trip() {
        let cmd = unmute_command("sess-42");
        assert!(matches!(cmd, Command::Unmute { .. }));
        assert_eq!(cmd.command_name(), "unmute");
        let v = serde_json::to_value(&cmd).unwrap();
        assert_eq!(v["command"], "unmute");
        assert_eq!(v["session_id"], "sess-42");
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
        let back: Command = serde_json::from_value(v).unwrap();
        assert_eq!(back, cmd);
    }

    /// `solo_command` round-trip.
    #[test]
    fn solo_command_round_trip() {
        let cmd = solo_command("focus-this");
        assert!(matches!(cmd, Command::Solo { .. }));
        assert_eq!(cmd.command_name(), "solo");
        let v = serde_json::to_value(&cmd).unwrap();
        assert_eq!(v["command"], "solo");
        assert_eq!(v["session_id"], "focus-this");
        assert_eq!(v["schema_version"], SCHEMA_VERSION);
        let back: Command = serde_json::from_value(v).unwrap();
        assert_eq!(back, cmd);
    }

    /// All three command constructors agree with the protocol crate's own
    /// constructors (`Command::mute` / `unmute` / `solo`).
    #[test]
    fn command_constructors_agree_with_protocol_crate() {
        assert_eq!(mute_command("s1"), Command::mute("s1"));
        assert_eq!(unmute_command("s1"), Command::unmute("s1"));
        assert_eq!(solo_command("s1"), Command::solo("s1"));
    }

    /// Command names for mute/unmute/solo are distinct (no accidental collision).
    #[test]
    fn mute_unmute_solo_command_names_are_distinct() {
        let names = [
            mute_command("s").command_name(),
            unmute_command("s").command_name(),
            solo_command("s").command_name(),
        ];
        let mut unique = names.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            3,
            "mute/unmute/solo must have distinct command names"
        );
    }

    // ── combined model+mute round-trip ────────────────────────────────────────

    /// After the view-model reflects a muted session (via a `session.updated`
    /// with `muted=true`), the mute module correctly suppresses its ping.
    #[test]
    fn inbox_model_muted_session_ping_is_suppressed() {
        use crate::InboxModel;
        use fleet_protocol::{
            Event, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind, Session, State,
        };

        let loc = Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        };
        let srv = Server {
            kind: ServerKind::Local,
            version: None,
            extra: Extra::new(),
        };

        let mut m = InboxModel::new();
        // Start with an unmuted waiting session.
        let mut s1 = Session::new(
            "s1",
            "proj",
            loc.clone(),
            srv.clone(),
            State::Waiting,
            "2026-06-08T00:00:00Z",
        );
        s1.muted = false;
        m.apply(Event::session_added(s1));

        let view = m.view();
        let tab = view.tab("s1").unwrap();
        assert!(
            should_notify(tab, &view.tabs),
            "unmuted waiting should notify"
        );

        // Hub sends back session.updated with muted=true.
        let mut s1_muted = Session::new(
            "s1",
            "proj",
            loc,
            srv,
            State::Waiting,
            "2026-06-08T00:00:01Z",
        );
        s1_muted.muted = true;
        m.apply(Event::session_updated(s1_muted));

        let view = m.view();
        let tab = view.tab("s1").unwrap();
        assert!(tab.muted, "tab must reflect muted=true from hub");
        assert!(
            !should_notify(tab, &view.tabs),
            "muted waiting must not notify"
        );
        // But state is still visible.
        assert_eq!(tab.state, TabState::Waiting, "mute does not hide state");
    }

    /// After the view-model reflects a solo (one session has `soloed=true`),
    /// the mute module suppresses all others and lets the soloed one through.
    #[test]
    fn inbox_model_solo_inverts_via_hub_echo() {
        use crate::InboxModel;
        use fleet_protocol::{
            Event, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind, Session, State,
        };

        let loc = Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        };
        let srv = Server {
            kind: ServerKind::Local,
            version: None,
            extra: Extra::new(),
        };

        let mut m = InboxModel::new();
        let s1 = Session::new(
            "s1",
            "a",
            loc.clone(),
            srv.clone(),
            State::Waiting,
            "2026-06-08T00:00:00Z",
        );
        let s2 = Session::new(
            "s2",
            "b",
            loc.clone(),
            srv.clone(),
            State::Waiting,
            "2026-06-08T00:00:00Z",
        );
        let s3 = Session::new("s3", "c", loc, srv, State::Waiting, "2026-06-08T00:00:00Z");
        m.apply(Event::session_added(s1));
        m.apply(Event::session_added(s2));
        m.apply(Event::session_added(s3));

        {
            let view = m.view();
            // All three waiting + no solo → all notify.
            for tab in &view.tabs {
                assert!(
                    should_notify(tab, &view.tabs),
                    "{} should notify before solo",
                    tab.session_id
                );
            }
        }

        // Hub echoes s2 as soloed.
        let mut s2_solo = fleet_protocol::Session::new(
            "s2",
            "b",
            fleet_protocol::Location {
                kind: fleet_protocol::LocationKind::Local,
                label: "laptop".into(),
                glyph: fleet_protocol::LocationGlyph::Laptop,
                attach_hint: None,
                extra: fleet_protocol::Extra::new(),
            },
            fleet_protocol::Server {
                kind: fleet_protocol::ServerKind::Local,
                version: None,
                extra: fleet_protocol::Extra::new(),
            },
            State::Waiting,
            "2026-06-08T00:00:01Z",
        );
        s2_solo.soloed = true;
        m.apply(Event::session_updated(s2_solo));

        let view = m.view();
        let t1 = view.tab("s1").unwrap();
        let t2 = view.tab("s2").unwrap();
        let t3 = view.tab("s3").unwrap();

        assert!(!should_notify(t1, &view.tabs), "s1 suppressed by solo");
        assert!(should_notify(t2, &view.tabs), "s2 (soloed) still notifies");
        assert!(!should_notify(t3, &view.tabs), "s3 suppressed by solo");
    }
}
