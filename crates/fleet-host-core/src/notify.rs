//! Notification derivation — slice S21 (node `UINOTIFY`).
//!
//! Maps [`crate::SessionTab`] state transitions to [`NotificationIntent`]s for
//! the Tauri host shell's notification plugin (PLAN S21 / D16: "OS-native via
//! Tauri, urgency-tiered"). The trigger is the pure transition
//! `old_tab → new_tab`; the Tauri notification-plugin *call* lives in the host
//! shell, not here — this module is a pure function, unit-testable with no OS or
//! window dependency.
//!
//! ## Urgency→sound mapping (PLAN S21)
//!
//! Urgency tiers drive **distinct desktop sound names** (not iOS `silent` /
//! Android Importance channels — those are per-platform APIs not available in the
//! Tauri v2 notification plugin's cross-platform interface). The tier→sound table:
//!
//! | Tier | Urgency | Sound name | Notes |
//! |---|---|---|---|
//! | Approval | `Urgency::Approval` | `"Fleet.Approval"` | Loudest — action required |
//! | Question | `Urgency::Question` | `"Fleet.Question"` | Mid — informational ask |
//! | IdleDone | `Urgency::IdleDone` | *(omitted)* | Silent tier — no sound |
//! | Working  | none | *(omitted)* | Activity, no alert |
//!
//! The `sound` field on [`NotificationIntent`] is `None` for the `IdleDone` tier
//! and for all non-pinging states. The host shell passes the `sound` string as-is
//! to the Tauri notification plugin's `sound` parameter; `None` means the plugin
//! is called with no `sound` argument (platform default silence for that
//! notification).
//!
//! ## Auto-resolve (PLAN S21, §21.4)
//!
//! When a tab transitions *out* of `Waiting` (the user answered in the terminal),
//! [`tab_transition`] returns [`NotificationOutcome::AutoResolve`] — a signal for
//! the host to clear the badge and dismiss any pending notification for that
//! session. The host clears on any terminal answer: it does not need to know
//! *which* answer; it only needs the `session_id`.
//!
//! ## No iOS/Android APIs
//!
//! This module deliberately does NOT use:
//! - the Tauri notification `silent` field (iOS-scoped; unavailable on the
//!   target desktop platforms macOS + Linux + Windows).
//! - Android notification Importance/channel APIs (Android-only).
//!
//! ## Design constraint: observer-not-owner (invariant 3)
//!
//! This module derives *intent* from Hub-broadcast state. It never writes back to
//! the Hub, never intercepts keystrokes, and never fires a notification unless a
//! state transition warrants it.

use crate::{SessionTab, TabState};
use fleet_protocol::Urgency;

// ── Public types ──────────────────────────────────────────────────────────────

/// The desktop sound name to pass to the Tauri notification plugin.
///
/// Each variant maps to a distinct string constant so the host shell can pass the
/// right name to the OS sound subsystem. The mapping is intentionally *named*
/// (not numeric) so product iteration can remap tiers without renaming call
/// sites.
///
/// Sound names are designed to be registered in the app bundle's sound resources
/// (e.g. `Fleet.Approval.caf` on macOS). The host passes the bare name (without
/// extension) to the OS; the OS resolves the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NotificationSound {
    /// Approval-tier alert — the loudest, most attention-demanding sound.
    /// Desktop sound name: `"Fleet.Approval"`.
    Approval,
    /// Question-tier alert — a mid-level informational sound.
    /// Desktop sound name: `"Fleet.Question"`.
    Question,
}

impl NotificationSound {
    /// The desktop sound name string passed to the Tauri notification plugin's
    /// `sound` parameter (no extension; the OS resolves the file).
    ///
    /// These are **distinct** names, not the same system sound, so the OS can play
    /// different audio for each urgency tier. Using the same name for two tiers
    /// would violate the PLAN S21 requirement for DISTINCT sounds.
    pub fn as_str(self) -> &'static str {
        match self {
            NotificationSound::Approval => "Fleet.Approval",
            NotificationSound::Question => "Fleet.Question",
        }
    }

    /// All sound variants, for exhaustive mapping tests.
    pub const ALL: [NotificationSound; 2] =
        [NotificationSound::Approval, NotificationSound::Question];
}

/// A fully-resolved notification intent: the title, body, and optional sound the
/// host shell should pass to the Tauri notification plugin for one session tab.
///
/// `sound` is `None` for the silent `IdleDone` tier and for non-pinging states.
/// The host passes `sound.as_deref()` to the plugin's `sound` parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationIntent {
    /// The session id this notification is for. The host uses this to clear the
    /// badge / dismiss the notification on auto-resolve.
    pub session_id: String,
    /// Short notification title shown in the OS notification centre.
    pub title: String,
    /// Notification body — the agent's last message or a generic prompt.
    pub body: String,
    /// Desktop sound name. `None` ⇒ silent (no `sound` argument to the plugin).
    /// Present only for the `Approval` and `Question` urgency tiers.
    pub sound: Option<NotificationSound>,
}

/// The outcome of a tab transition for the notification subsystem.
///
/// The host shell calls [`tab_transition`] on every view update, then acts on the
/// returned outcome:
/// - [`Fire`](NotificationOutcome::Fire) → send a new OS notification.
/// - [`AutoResolve`](NotificationOutcome::AutoResolve) → dismiss any pending
///   notification for the session and clear its badge.
/// - [`Noop`](NotificationOutcome::Noop) → no notification action needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationOutcome {
    /// A new notification should be fired. Contains the intent.
    Fire(NotificationIntent),
    /// The previous `waiting` notification has been answered in the terminal.
    /// The host should clear the badge and dismiss the pending notification for
    /// `session_id`. Corresponds to §21.4 auto-resolve.
    AutoResolve {
        /// The session whose notification should be cleared.
        session_id: String,
    },
    /// No notification action required for this transition.
    Noop,
}

// ── Core mapping ──────────────────────────────────────────────────────────────

/// Map a [`Urgency`] to the appropriate [`NotificationSound`], if any.
///
/// The `IdleDone` tier is intentionally **silent** (returns `None`). The
/// `Approval` and `Question` tiers return their respective distinct sound names.
/// This is the tier→sound mapping table required by PLAN S21 / WORK_GRAPH G3.
///
/// # Sound distinctness invariant
///
/// Every tier that has a sound MUST map to a **different** `NotificationSound`
/// variant. This is enforced by the unit tests.
pub fn urgency_to_sound(urgency: Urgency) -> Option<NotificationSound> {
    match urgency {
        Urgency::Approval => Some(NotificationSound::Approval),
        Urgency::Question => Some(NotificationSound::Question),
        // IdleDone is the silent tier — no sound, no iOS `silent` flag.
        Urgency::IdleDone => None,
        // Urgency::None ⇒ not waiting / not pinging.
        Urgency::None => None,
    }
}

/// Map a tab's urgency (the `Option<Urgency>` on [`SessionTab`]) to a sound,
/// normalizing the `Option` wrapper: `None` ⇒ `None` (silent).
pub fn tab_urgency_to_sound(urgency: Option<Urgency>) -> Option<NotificationSound> {
    urgency.and_then(urgency_to_sound)
}

/// Derive a notification title and body for a waiting session tab.
///
/// The title names the agent (from the session title); the body is generic for
/// v1 (a future slice can enrich it with `last_message`). Both are returned as
/// owned `String`s so the intent is self-contained.
fn notification_text(tab: &SessionTab) -> (String, String) {
    let urgency_label = match tab.urgency {
        Some(Urgency::Approval) => "Approval needed",
        Some(Urgency::Question) => "Question asked",
        Some(Urgency::IdleDone) => "Task complete",
        Some(Urgency::None) | None => "Waiting",
    };
    let title = format!("{urgency_label} — {}", tab.title);
    let body = String::from("Agent is waiting for your response.");
    (title, body)
}

/// Compute the [`NotificationOutcome`] for a single tab transition.
///
/// - **`None` → `Some(new_tab)`**: first appearance of the tab. If the new tab
///   is `Waiting`, fire a notification.
/// - **`Some(old)` → `Some(new)`**: delta update. Fire a notification when the
///   tab *enters* `Waiting` (old was not waiting, new is waiting). Auto-resolve
///   when the tab *leaves* `Waiting` (old was waiting, new is not).
/// - **`Some(old)` → `None`**: tab removed. If the old tab was `Waiting`,
///   auto-resolve (the session is gone; clear its badge).
///
/// In all other cases (non-pinging states, no transition across the `Waiting`
/// boundary): `Noop`.
///
/// The `muted` flag on the tab suppresses `Fire` outcomes but NOT `AutoResolve`
/// outcomes — a muted session whose pending notification is resolved should still
/// clear its badge.
pub fn tab_transition(old: Option<&SessionTab>, new: Option<&SessionTab>) -> NotificationOutcome {
    match (old, new) {
        // Tab added: fire if it arrives in Waiting and is not muted.
        (None, Some(new_tab)) => {
            if new_tab.state == TabState::Waiting && !new_tab.muted {
                let (title, body) = notification_text(new_tab);
                NotificationOutcome::Fire(NotificationIntent {
                    session_id: new_tab.session_id.clone(),
                    title,
                    body,
                    sound: tab_urgency_to_sound(new_tab.urgency),
                })
            } else {
                NotificationOutcome::Noop
            }
        }

        // Tab transition: handle the Waiting boundary crossings.
        (Some(old_tab), Some(new_tab)) => {
            let was_waiting = old_tab.state == TabState::Waiting;
            let now_waiting = new_tab.state == TabState::Waiting;

            if !was_waiting && now_waiting {
                // Entered Waiting — fire, unless muted.
                if new_tab.muted {
                    return NotificationOutcome::Noop;
                }
                let (title, body) = notification_text(new_tab);
                NotificationOutcome::Fire(NotificationIntent {
                    session_id: new_tab.session_id.clone(),
                    title,
                    body,
                    sound: tab_urgency_to_sound(new_tab.urgency),
                })
            } else if was_waiting && !now_waiting {
                // Left Waiting — auto-resolve (clear badge regardless of mute).
                NotificationOutcome::AutoResolve {
                    session_id: new_tab.session_id.clone(),
                }
            } else if was_waiting && now_waiting {
                // Still waiting: re-fire only if urgency changed (escalation).
                // In v1 we do not re-fire on same-urgency; changes in urgency
                // while already waiting fire a new notification.
                if old_tab.urgency != new_tab.urgency {
                    if new_tab.muted {
                        return NotificationOutcome::Noop;
                    }
                    let (title, body) = notification_text(new_tab);
                    NotificationOutcome::Fire(NotificationIntent {
                        session_id: new_tab.session_id.clone(),
                        title,
                        body,
                        sound: tab_urgency_to_sound(new_tab.urgency),
                    })
                } else {
                    NotificationOutcome::Noop
                }
            } else {
                // Neither was waiting nor is now waiting.
                NotificationOutcome::Noop
            }
        }

        // Tab removed while waiting — auto-resolve.
        (Some(old_tab), None) => {
            if old_tab.state == TabState::Waiting {
                NotificationOutcome::AutoResolve {
                    session_id: old_tab.session_id.clone(),
                }
            } else {
                NotificationOutcome::Noop
            }
        }

        // No old, no new: no-op (degenerate call, should not occur in practice).
        (None, None) => NotificationOutcome::Noop,
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentIcon, SessionTab, TabState};
    use fleet_protocol::{Confidence, LocationGlyph, Urgency};

    // ── Helpers ────────────────────────────────────────────────────────────────

    fn make_tab(id: &str, state: TabState, urgency: Option<Urgency>, muted: bool) -> SessionTab {
        SessionTab {
            session_id: id.into(),
            glyph: LocationGlyph::Laptop,
            agent_icon: AgentIcon::Claude,
            title: format!("Session {id}"),
            state,
            urgency,
            confidence: if state == TabState::Waiting {
                Some(Confidence::High)
            } else {
                None
            },
            waiting_since: if state == TabState::Waiting {
                Some("2026-06-08T10:00:00Z".into())
            } else {
                None
            },
            muted,
            soloed: false,
            unread: false,
            run_count: 1,
        }
    }

    fn waiting(id: &str, urgency: Urgency) -> SessionTab {
        make_tab(id, TabState::Waiting, Some(urgency), false)
    }

    fn waiting_muted(id: &str, urgency: Urgency) -> SessionTab {
        make_tab(id, TabState::Waiting, Some(urgency), true)
    }

    fn working(id: &str) -> SessionTab {
        make_tab(id, TabState::Working, None, false)
    }

    fn idle(id: &str) -> SessionTab {
        make_tab(id, TabState::Idle, None, false)
    }

    fn done(id: &str) -> SessionTab {
        make_tab(id, TabState::Done, None, false)
    }

    // ── urgency_to_sound mapping table ────────────────────────────────────────

    /// PLAN S21 and WORK_GRAPH G3: approval tier maps to a distinct sound name.
    #[test]
    fn approval_maps_to_approval_sound() {
        assert_eq!(
            urgency_to_sound(Urgency::Approval),
            Some(NotificationSound::Approval)
        );
    }

    /// Question tier maps to a distinct sound name.
    #[test]
    fn question_maps_to_question_sound() {
        assert_eq!(
            urgency_to_sound(Urgency::Question),
            Some(NotificationSound::Question)
        );
    }

    /// PLAN S21: IdleDone is the **silent tier** — no sound.
    #[test]
    fn idle_done_maps_to_no_sound() {
        assert_eq!(urgency_to_sound(Urgency::IdleDone), None);
    }

    /// Urgency::None (non-pinging) also maps to no sound.
    #[test]
    fn urgency_none_maps_to_no_sound() {
        assert_eq!(urgency_to_sound(Urgency::None), None);
    }

    /// No sound for absent urgency (Option::None wrapper).
    #[test]
    fn tab_urgency_none_maps_to_no_sound() {
        assert_eq!(tab_urgency_to_sound(None), None);
    }

    /// Approval sound string is "Fleet.Approval" (PLAN S21 distinct sound name).
    #[test]
    fn approval_sound_name_is_distinct() {
        assert_eq!(NotificationSound::Approval.as_str(), "Fleet.Approval");
    }

    /// Question sound string is "Fleet.Question" (PLAN S21 distinct sound name).
    #[test]
    fn question_sound_name_is_distinct() {
        assert_eq!(NotificationSound::Question.as_str(), "Fleet.Question");
    }

    /// All sound names are DISTINCT strings (PLAN S21 "distinct desktop sound names").
    #[test]
    fn all_sound_names_are_distinct() {
        let names: Vec<&str> = NotificationSound::ALL.iter().map(|s| s.as_str()).collect();
        let mut deduped = names.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(
            deduped.len(),
            names.len(),
            "every urgency tier must map to a distinct sound name: {names:?}"
        );
    }

    /// All tiers that have a sound each map to a DIFFERENT NotificationSound variant.
    #[test]
    fn tiers_with_sounds_map_to_different_variants() {
        let approval = urgency_to_sound(Urgency::Approval);
        let question = urgency_to_sound(Urgency::Question);
        assert_ne!(
            approval, question,
            "Approval and Question must use distinct NotificationSound variants"
        );
    }

    /// Pinging tiers (Approval, Question) always produce Some(sound).
    #[test]
    fn pinging_tiers_have_sounds() {
        for u in [Urgency::Approval, Urgency::Question] {
            assert!(
                urgency_to_sound(u).is_some(),
                "urgency {u:?} must produce a sound"
            );
        }
    }

    /// Silent tiers (IdleDone, None) always produce None.
    #[test]
    fn silent_tiers_produce_no_sound() {
        for u in [Urgency::IdleDone, Urgency::None] {
            assert!(
                urgency_to_sound(u).is_none(),
                "urgency {u:?} must produce no sound (silent tier)"
            );
        }
    }

    // ── tab_transition — fire cases ───────────────────────────────────────────

    /// New tab arrives already in Waiting → fire with the correct sound.
    #[test]
    fn new_tab_arrives_waiting_fires_notification() {
        let new_tab = waiting("s1", Urgency::Approval);
        let outcome = tab_transition(None, Some(&new_tab));
        match outcome {
            NotificationOutcome::Fire(intent) => {
                assert_eq!(intent.session_id, "s1");
                assert_eq!(intent.sound, Some(NotificationSound::Approval));
            }
            other => panic!("expected Fire, got {other:?}"),
        }
    }

    /// Transition from Working → Waiting fires a notification.
    #[test]
    fn working_to_waiting_fires_notification() {
        let old = working("s1");
        let new_tab = waiting("s1", Urgency::Approval);
        let outcome = tab_transition(Some(&old), Some(&new_tab));
        match outcome {
            NotificationOutcome::Fire(intent) => {
                assert_eq!(intent.session_id, "s1");
                assert_eq!(intent.sound, Some(NotificationSound::Approval));
            }
            other => panic!("expected Fire, got {other:?}"),
        }
    }

    /// Transition from Idle → Waiting fires a notification.
    #[test]
    fn idle_to_waiting_fires_notification() {
        let old = idle("s1");
        let new_tab = waiting("s1", Urgency::Question);
        let outcome = tab_transition(Some(&old), Some(&new_tab));
        match outcome {
            NotificationOutcome::Fire(intent) => {
                assert_eq!(intent.session_id, "s1");
                assert_eq!(intent.sound, Some(NotificationSound::Question));
            }
            other => panic!("expected Fire, got {other:?}"),
        }
    }

    /// Transition from Done → Waiting fires a notification.
    #[test]
    fn done_to_waiting_fires_notification() {
        let old = done("s1");
        let new_tab = waiting("s1", Urgency::Approval);
        let outcome = tab_transition(Some(&old), Some(&new_tab));
        assert!(
            matches!(outcome, NotificationOutcome::Fire(_)),
            "done→waiting must fire"
        );
    }

    /// IdleDone urgency fires a notification but with NO sound (silent tier).
    #[test]
    fn idle_done_urgency_fires_silently() {
        let old = working("s1");
        let new_tab = waiting("s1", Urgency::IdleDone);
        let outcome = tab_transition(Some(&old), Some(&new_tab));
        match outcome {
            NotificationOutcome::Fire(intent) => {
                assert_eq!(
                    intent.sound, None,
                    "IdleDone tier must produce no sound (silent)"
                );
            }
            other => panic!("expected Fire(silent), got {other:?}"),
        }
    }

    // ── tab_transition — auto-resolve cases ───────────────────────────────────

    /// PLAN §21.4 auto-resolve: Waiting → Working clears the notification.
    #[test]
    fn waiting_to_working_auto_resolves() {
        let old = waiting("s1", Urgency::Approval);
        let new_tab = working("s1");
        let outcome = tab_transition(Some(&old), Some(&new_tab));
        assert_eq!(
            outcome,
            NotificationOutcome::AutoResolve {
                session_id: "s1".into()
            },
            "waiting→working must auto-resolve"
        );
    }

    /// Waiting → Idle auto-resolves (user answered, agent went idle).
    #[test]
    fn waiting_to_idle_auto_resolves() {
        let old = waiting("s1", Urgency::Approval);
        let new_tab = idle("s1");
        let outcome = tab_transition(Some(&old), Some(&new_tab));
        assert_eq!(
            outcome,
            NotificationOutcome::AutoResolve {
                session_id: "s1".into()
            }
        );
    }

    /// Waiting → Done auto-resolves.
    #[test]
    fn waiting_to_done_auto_resolves() {
        let old = waiting("s1", Urgency::Question);
        let new_tab = done("s1");
        let outcome = tab_transition(Some(&old), Some(&new_tab));
        assert_eq!(
            outcome,
            NotificationOutcome::AutoResolve {
                session_id: "s1".into()
            }
        );
    }

    /// Tab removed while waiting → auto-resolve (session gone, clear badge).
    #[test]
    fn waiting_tab_removed_auto_resolves() {
        let old = waiting("s1", Urgency::Approval);
        let outcome = tab_transition(Some(&old), None);
        assert_eq!(
            outcome,
            NotificationOutcome::AutoResolve {
                session_id: "s1".into()
            }
        );
    }

    /// Auto-resolve includes the correct session id.
    #[test]
    fn auto_resolve_carries_session_id() {
        let old = waiting("my-session-99", Urgency::Approval);
        let new_tab = working("my-session-99");
        match tab_transition(Some(&old), Some(&new_tab)) {
            NotificationOutcome::AutoResolve { session_id } => {
                assert_eq!(session_id, "my-session-99");
            }
            other => panic!("expected AutoResolve, got {other:?}"),
        }
    }

    // ── tab_transition — noop cases ───────────────────────────────────────────

    /// Working → Idle: no notification.
    #[test]
    fn working_to_idle_is_noop() {
        let old = working("s1");
        let new_tab = idle("s1");
        assert_eq!(
            tab_transition(Some(&old), Some(&new_tab)),
            NotificationOutcome::Noop
        );
    }

    /// Idle → Working: no notification.
    #[test]
    fn idle_to_working_is_noop() {
        let old = idle("s1");
        let new_tab = working("s1");
        assert_eq!(
            tab_transition(Some(&old), Some(&new_tab)),
            NotificationOutcome::Noop
        );
    }

    /// Non-waiting tab removed: no notification.
    #[test]
    fn non_waiting_tab_removed_is_noop() {
        let old = working("s1");
        assert_eq!(tab_transition(Some(&old), None), NotificationOutcome::Noop);
    }

    /// New tab arrives in Working: no notification.
    #[test]
    fn new_tab_working_is_noop() {
        let new_tab = working("s1");
        assert_eq!(
            tab_transition(None, Some(&new_tab)),
            NotificationOutcome::Noop
        );
    }

    /// New tab arrives in Idle: no notification.
    #[test]
    fn new_tab_idle_is_noop() {
        let new_tab = idle("s1");
        assert_eq!(
            tab_transition(None, Some(&new_tab)),
            NotificationOutcome::Noop
        );
    }

    /// Degenerate call (None, None): no-op, no panic.
    #[test]
    fn both_none_is_noop() {
        assert_eq!(tab_transition(None, None), NotificationOutcome::Noop);
    }

    /// Same state (Waiting → Waiting, same urgency): no notification.
    #[test]
    fn waiting_to_waiting_same_urgency_is_noop() {
        let old = waiting("s1", Urgency::Approval);
        let new_tab = waiting("s1", Urgency::Approval);
        assert_eq!(
            tab_transition(Some(&old), Some(&new_tab)),
            NotificationOutcome::Noop
        );
    }

    // ── mute suppresses Fire but NOT AutoResolve ──────────────────────────────

    /// Muted tab entering Waiting → Noop (notification suppressed).
    #[test]
    fn muted_working_to_waiting_is_noop() {
        let old = make_tab("s1", TabState::Working, None, true);
        let new_tab = waiting_muted("s1", Urgency::Approval);
        assert_eq!(
            tab_transition(Some(&old), Some(&new_tab)),
            NotificationOutcome::Noop,
            "muted tab must not fire a notification"
        );
    }

    /// Muted tab leaving Waiting → AutoResolve (badge must still clear).
    #[test]
    fn muted_waiting_to_working_still_auto_resolves() {
        let old = waiting_muted("s1", Urgency::Approval);
        let new_tab = make_tab("s1", TabState::Working, None, true);
        assert_eq!(
            tab_transition(Some(&old), Some(&new_tab)),
            NotificationOutcome::AutoResolve {
                session_id: "s1".into()
            },
            "muted tab leaving Waiting must still auto-resolve"
        );
    }

    /// New muted tab arriving in Waiting → Noop.
    #[test]
    fn new_muted_tab_arriving_waiting_is_noop() {
        let new_tab = waiting_muted("s1", Urgency::Approval);
        assert_eq!(
            tab_transition(None, Some(&new_tab)),
            NotificationOutcome::Noop
        );
    }

    // ── urgency escalation while already waiting ──────────────────────────────

    /// Urgency escalation while already Waiting fires a new notification.
    #[test]
    fn urgency_escalation_while_waiting_fires() {
        let old = waiting("s1", Urgency::Question);
        let new_tab = waiting("s1", Urgency::Approval);
        let outcome = tab_transition(Some(&old), Some(&new_tab));
        match outcome {
            NotificationOutcome::Fire(intent) => {
                assert_eq!(intent.sound, Some(NotificationSound::Approval));
            }
            other => panic!("expected Fire on urgency escalation, got {other:?}"),
        }
    }

    /// Same urgency while already Waiting: no new fire.
    #[test]
    fn same_urgency_while_waiting_is_noop() {
        let old = waiting("s1", Urgency::Question);
        let new_tab = waiting("s1", Urgency::Question);
        assert_eq!(
            tab_transition(Some(&old), Some(&new_tab)),
            NotificationOutcome::Noop
        );
    }

    // ── notification intent fields ────────────────────────────────────────────

    /// Fire intent carries a non-empty title and body.
    #[test]
    fn fire_intent_has_non_empty_title_and_body() {
        let new_tab = waiting("s1", Urgency::Approval);
        match tab_transition(None, Some(&new_tab)) {
            NotificationOutcome::Fire(intent) => {
                assert!(!intent.title.is_empty(), "title must not be empty");
                assert!(!intent.body.is_empty(), "body must not be empty");
            }
            other => panic!("expected Fire, got {other:?}"),
        }
    }

    /// Approval notification title contains the session title.
    #[test]
    fn fire_intent_title_contains_session_title() {
        let new_tab = waiting("s1", Urgency::Approval);
        match tab_transition(None, Some(&new_tab)) {
            NotificationOutcome::Fire(intent) => {
                assert!(
                    intent.title.contains(&new_tab.title),
                    "title '{}' should contain session title '{}'",
                    intent.title,
                    new_tab.title
                );
            }
            other => panic!("expected Fire, got {other:?}"),
        }
    }

    // ── exhaustive: every pinging state entry fires exactly once ─────────────

    /// For every non-Waiting state → Waiting transition, the outcome is Fire.
    #[test]
    fn every_non_waiting_to_waiting_fires() {
        let non_waiting_states = [
            make_tab("s", TabState::Working, None, false),
            make_tab("s", TabState::Idle, None, false),
            make_tab("s", TabState::Done, None, false),
            make_tab("s", TabState::Error, None, false),
        ];
        for old_tab in &non_waiting_states {
            let new_tab = waiting("s", Urgency::Approval);
            let outcome = tab_transition(Some(old_tab), Some(&new_tab));
            assert!(
                matches!(outcome, NotificationOutcome::Fire(_)),
                "transition from {:?} → Waiting must Fire",
                old_tab.state
            );
        }
    }

    /// For every Waiting → non-Waiting transition, the outcome is AutoResolve.
    #[test]
    fn every_waiting_to_non_waiting_auto_resolves() {
        let non_waiting_new = [
            make_tab("s", TabState::Working, None, false),
            make_tab("s", TabState::Idle, None, false),
            make_tab("s", TabState::Done, None, false),
            make_tab("s", TabState::Error, None, false),
            make_tab("s", TabState::Dead, None, false),
        ];
        for new_tab in &non_waiting_new {
            let old = waiting("s", Urgency::Approval);
            let outcome = tab_transition(Some(&old), Some(new_tab));
            assert!(
                matches!(outcome, NotificationOutcome::AutoResolve { .. }),
                "transition from Waiting → {:?} must AutoResolve",
                new_tab.state
            );
        }
    }
}
