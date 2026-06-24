//! Focus / jump-to-next-unread — slice **S23** (node `FOCUS`).
//!
//! This is the host face's editor-focus seam. It has two halves that the
//! the reducer test gate ("per-OS focus with mocked OS calls + focus-confirmation
//! telemetry") pins:
//!
//! 1. **Pure selection** (no OS, no I/O) — given the current [`InboxView`],
//!    decide *which* tab to focus next (jump-to-next-unread, or cycle order over
//!    the tabs) and emit the [`fleet_protocol::Command::focus`] for it. This is a
//!    deterministic function of the view + a cursor, unit-testable with no
//!    window. See [`next_unread`], [`cycle_next`], and [`focus_command`].
//!
//! 2. **Per-OS activation behind a trait** ([`FocusBackend`]) — actually raising
//!    the editor window. The real activation is inherently impure (it shells out
//!    to AppleScript / `wmctrl` / a Wayland CLI), so the **logic** here is
//!    expressed against the [`FocusBackend`] trait and a per-OS [`FocusStrategy`]
//!    that decides *how* to attempt activation and *whether the result can be
//!    trusted*. The unit tests drive it through a [`MockBackend`], never a real
//!    OS call (so the test suite runs with no codex/VS Code/window manager).
//!
//! ## Why a strategy + confirmation telemetry (the engineering spec, §15.3, invariant 5)
//!
//! Activation reliability differs sharply per platform, and the UI must **never
//! falsely claim it focused a window**:
//!
//! - **macOS** — `NSRunningApplication.activate` is *unreliable on Sonoma+*
//!   (returns `false` / only bounces the dock; `ActivateIgnoringOtherApps` is
//!   deprecated). The strategy therefore uses AppleScript `activate` /
//!   `NSWorkspace.openApplication`, and reports [`FocusOutcome::Confirmed`] only
//!   when the backend confirms the app became frontmost.
//! - **Linux X11** — `wmctrl`/`xdotool`/EWMH can activate a specific window and
//!   the WM confirms it, so `Confirmed` is achievable.
//! - **Wayland** — a compositor **will not** let an app activate an
//!   already-running *foreign* window. The most we can do is spawn/raise the
//!   editor via its own CLI passing a valid `XDG_ACTIVATION_TOKEN`, and fall back
//!   to a notification. So Wayland's strategy **never** returns `Confirmed` for a
//!   foreign window — at best [`FocusOutcome::Requested`] (token handed off) or
//!   [`FocusOutcome::FellBackToNotification`]. **We never promise auto-focus.**
//!
//! The [`FocusOutcome`] returned by [`focus_window`] is the *honest* telemetry
//! the host surfaces: only `Confirmed` lets the UI say "focused"; everything else
//! must be shown as "attempted" / "see notification". This is the focus analog of
//! invariant 5 (confidence honesty): the system never overstates success.
//!
//! Disjoint from the sort/notify/confidence/palette/mute seams (its own file).

use crate::{InboxView, SessionTab};
use fleet_protocol::{Command, Target};

// ── Part 1: pure selection (no OS) ────────────────────────────────────────────

/// The [`Command::focus`] addressing a session by id.
///
/// The host issues this to the Hub, which routes it to the editor face that owns
/// the window (README §12.2). Pure: a session id in, a command out.
pub fn focus_command(session_id: &str) -> Command {
    Command::focus(Target::session(session_id))
}

/// Find the **next unread** tab at or after `from` (wrapping once), returning its
/// index in [`InboxView::tabs`].
///
/// "Jump-to-next-unread" (the engineering spec): starting just **after** `from`, scan forward
/// wrapping around, and return the first tab whose [`SessionTab::unread`] is set.
/// `from = None` starts the scan at index 0. Returns `None` when no tab is unread.
///
/// The scan is deterministic and window-independent. `from` is the index of the
/// *currently focused* tab (the cursor); passing the previous result threads a
/// stable jump-through-all-unread cycle.
pub fn next_unread(view: &InboxView, from: Option<usize>) -> Option<usize> {
    let n = view.tabs.len();
    if n == 0 {
        return None;
    }
    // Start one past the cursor (or at 0 when there is no cursor).
    let start = match from {
        Some(i) => i + 1,
        None => 0,
    };
    for offset in 0..n {
        let idx = (start + offset) % n;
        if view.tabs[idx].unread {
            return Some(idx);
        }
    }
    None
}

/// The [`SessionTab`] (and its index) of the next unread tab, if any.
///
/// Convenience over [`next_unread`] that also borrows the tab so the caller can
/// build a [`focus_command`] from `tab.session_id` in one step.
pub fn next_unread_tab(view: &InboxView, from: Option<usize>) -> Option<(usize, &SessionTab)> {
    next_unread(view, from).map(|i| (i, &view.tabs[i]))
}

/// Cycle to the next tab after `from` **without** the unread filter (the engineering spec's
/// "cycle-without-clearing" relies on plain cycle order; S23 uses it for the
/// jump keybind when nothing is unread).
///
/// Returns the index of the tab after `from` (wrapping). `from = None` → 0.
/// `None` only when there are no tabs.
pub fn cycle_next(view: &InboxView, from: Option<usize>) -> Option<usize> {
    let n = view.tabs.len();
    if n == 0 {
        return None;
    }
    Some(match from {
        Some(i) => (i + 1) % n,
        None => 0,
    })
}

// ── Part 2: per-OS activation behind a trait ──────────────────────────────────

/// The platform whose activation strategy the host uses. Selected per `cfg`
/// (target OS) or at runtime (Linux: X11 vs Wayland is a *session* property, not
/// a compile-time one — `XDG_SESSION_TYPE` / `WAYLAND_DISPLAY`).
///
/// [`FocusStrategy::for_platform`] maps each variant to its [`FocusStrategy`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusPlatform {
    /// macOS — AppleScript `activate` / `NSWorkspace.openApplication`.
    /// (`NSRunningApplication.activate` is unreliable on Sonoma+; not used.)
    MacOs,
    /// Linux running an X11 session — `wmctrl`/`xdotool`/EWMH.
    LinuxX11,
    /// Linux running a Wayland session — `XDG_ACTIVATION_TOKEN` spawn + a
    /// notification fallback. Foreign-window activation is **never guaranteed**.
    Wayland,
    /// Any other / unknown platform (e.g. Windows best-effort) — attempt a
    /// best-effort activation and never claim confirmation.
    Other,
}

impl FocusPlatform {
    /// Detect the platform from the compile target and (on Linux) the runtime
    /// session type. Pure-ish: reads env vars on Linux but no OS *activation*.
    ///
    /// The env reads are confined to this one function so the rest of the module
    /// stays a pure function of its inputs (and the tests inject the platform
    /// directly via [`FocusStrategy::for_platform`]).
    pub fn detect() -> Self {
        #[cfg(target_os = "macos")]
        {
            FocusPlatform::MacOs
        }
        #[cfg(target_os = "linux")]
        {
            detect_linux_session()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            FocusPlatform::Other
        }
    }
}

/// On Linux, decide X11 vs Wayland from the session env. Wayland is indicated by
/// `XDG_SESSION_TYPE=wayland` or a non-empty `WAYLAND_DISPLAY`; otherwise X11.
#[cfg(target_os = "linux")]
fn detect_linux_session() -> FocusPlatform {
    let is_wayland = std::env::var("XDG_SESSION_TYPE")
        .map(|v| v.eq_ignore_ascii_case("wayland"))
        .unwrap_or(false)
        || std::env::var("WAYLAND_DISPLAY")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
    if is_wayland {
        FocusPlatform::Wayland
    } else {
        FocusPlatform::LinuxX11
    }
}

/// The per-OS activation strategy — *which* mechanism to try and *whether its
/// result can be trusted as confirmation*.
///
/// This is the data the platform-agnostic [`focus_window`] reads to decide how
/// to interpret a backend result. Keeping it data (not a second trait) means the
/// confirmation policy is itself unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusStrategy {
    /// The platform this strategy targets.
    pub platform: FocusPlatform,
    /// The activation mechanism the backend should attempt.
    pub mechanism: FocusMechanism,
    /// Whether a successful backend activation may be reported as
    /// [`FocusOutcome::Confirmed`]. **`false` on Wayland** (foreign-window
    /// activation can never be guaranteed → never promise auto-focus).
    pub confirmation_possible: bool,
    /// Whether, on a failed/unconfirmed activation, the strategy falls back to a
    /// user notification ("your editor is over here") rather than silently
    /// failing. `true` on Wayland (its primary path) and as a last resort
    /// elsewhere.
    pub notify_on_fallback: bool,
}

/// The concrete activation mechanism a [`FocusStrategy`] selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusMechanism {
    /// macOS: AppleScript `activate` / `NSWorkspace.openApplication`.
    AppleScript,
    /// Linux X11: `wmctrl`/`xdotool`/EWMH `_NET_ACTIVE_WINDOW`.
    X11Wmctrl,
    /// Wayland: spawn/raise the editor CLI with `XDG_ACTIVATION_TOKEN`.
    XdgActivationToken,
    /// Fallback / unknown platform: best-effort, unconfirmable.
    BestEffort,
}

impl FocusStrategy {
    /// The strategy for a given platform. This is the per-OS dispatch the node
    /// owns: each platform gets a distinct mechanism and confirmation policy.
    pub fn for_platform(platform: FocusPlatform) -> Self {
        match platform {
            FocusPlatform::MacOs => FocusStrategy {
                platform,
                mechanism: FocusMechanism::AppleScript,
                confirmation_possible: true,
                notify_on_fallback: true,
            },
            FocusPlatform::LinuxX11 => FocusStrategy {
                platform,
                mechanism: FocusMechanism::X11Wmctrl,
                confirmation_possible: true,
                notify_on_fallback: true,
            },
            FocusPlatform::Wayland => FocusStrategy {
                platform,
                // Spawn-with-token is the only sanctioned path; activation of an
                // already-running foreign window cannot be guaranteed, so
                // confirmation is impossible by construction.
                mechanism: FocusMechanism::XdgActivationToken,
                confirmation_possible: false,
                notify_on_fallback: true,
            },
            FocusPlatform::Other => FocusStrategy {
                platform,
                mechanism: FocusMechanism::BestEffort,
                confirmation_possible: false,
                notify_on_fallback: true,
            },
        }
    }

    /// The strategy for the detected current platform.
    pub fn current() -> Self {
        Self::for_platform(FocusPlatform::detect())
    }
}

/// What a [`FocusBackend`] reports after attempting activation — the *raw* OS
/// signal, before [`focus_window`] applies the strategy's confirmation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendResult {
    /// The OS reported the window became frontmost (e.g. AppleScript returned
    /// success and the app is frontmost; the WM set `_NET_ACTIVE_WINDOW`).
    Activated,
    /// The activation request was dispatched but the OS cannot confirm the
    /// window is now frontmost (e.g. Wayland token handed off; macOS `activate`
    /// returned but frontmost is unverified).
    Requested,
    /// The activation attempt failed outright (no such window, command not
    /// found, exec error).
    Failed,
}

/// The **honest** outcome the host surfaces. Only [`FocusOutcome::Confirmed`]
/// lets the UI claim it focused the window; every other variant must be shown as
/// "attempted" so the UI never falsely claims success (the engineering spec, invariant 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusOutcome {
    /// The window is confirmed frontmost — the UI may show "focused".
    /// Only reachable when the strategy's `confirmation_possible` is `true`
    /// **and** the backend reported [`BackendResult::Activated`].
    Confirmed,
    /// Activation was requested and the OS accepted it, but the result is
    /// **unconfirmed** (Wayland token handoff, or a platform where the backend
    /// activated but confirmation is not possible). The UI must say "attempted",
    /// never "focused".
    Requested,
    /// Activation failed and a notification fallback was raised ("your editor is
    /// over here"). The UI points the user at the notification.
    FellBackToNotification,
    /// Activation failed and no fallback was available. The UI reports failure.
    Failed,
}

impl FocusOutcome {
    /// Whether the UI may truthfully claim the window was focused.
    ///
    /// **Only** [`FocusOutcome::Confirmed`] returns `true`. This is the single
    /// predicate the host gates its "focused ✓" affordance on, guaranteeing the
    /// UI never overstates success (the engineering spec focus-confirmation telemetry).
    pub fn is_confirmed_success(self) -> bool {
        matches!(self, FocusOutcome::Confirmed)
    }

    /// Whether *any* activation work was dispatched (confirmed, requested, or a
    /// fallback notification was raised) — i.e. the user got *some* feedback.
    pub fn took_action(self) -> bool {
        !matches!(self, FocusOutcome::Failed)
    }

    /// A short telemetry label for logging / metrics.
    pub fn label(self) -> &'static str {
        match self {
            FocusOutcome::Confirmed => "confirmed",
            FocusOutcome::Requested => "requested",
            FocusOutcome::FellBackToNotification => "fell_back_to_notification",
            FocusOutcome::Failed => "failed",
        }
    }
}

/// The impure activation backend. Real implementations shell out to AppleScript /
/// `wmctrl` / a Wayland CLI; the unit tests use [`MockBackend`]. The trait is the
/// single seam between the pure strategy logic and the OS.
pub trait FocusBackend {
    /// Attempt to activate the window described by `focus_hint` using the given
    /// `mechanism`. Returns the raw [`BackendResult`] (not yet interpreted by the
    /// confirmation policy). The `focus_hint` is the session's
    /// [`fleet_protocol::Editor::focus_hint`] (the editor CLI/URI to focus).
    fn activate(&self, mechanism: FocusMechanism, focus_hint: &str) -> BackendResult;

    /// Raise a "your editor is over here" notification as the fallback when
    /// activation cannot be confirmed. Returns `true` if the notification was
    /// shown. Default: no fallback available (`false`).
    fn notify_fallback(&self, _focus_hint: &str) -> bool {
        false
    }
}

/// Drive a single focus attempt through a backend, applying the strategy's
/// confirmation policy to produce the **honest** [`FocusOutcome`].
///
/// This is the platform-agnostic core the the reducer test gate exercises with a mocked
/// backend. The policy:
///
/// | backend result | confirmation possible | → outcome |
/// |---|---|---|
/// | `Activated` | yes | `Confirmed` |
/// | `Activated` | no  | `Requested` (can't confirm → never claim success) |
/// | `Requested` | any | `Requested` |
/// | `Failed`    | any | fallback notification → `FellBackToNotification`, else `Failed` |
///
/// The crucial honesty property: an outcome of [`FocusOutcome::Confirmed`] is
/// **only** produced when the strategy allows confirmation *and* the OS actually
/// reported activation — so the UI's "focused ✓" can never be shown for a
/// Wayland foreign window, a failed activation, or an unverifiable request.
pub fn focus_window(
    backend: &dyn FocusBackend,
    strategy: FocusStrategy,
    focus_hint: &str,
) -> FocusOutcome {
    match backend.activate(strategy.mechanism, focus_hint) {
        BackendResult::Activated => {
            if strategy.confirmation_possible {
                FocusOutcome::Confirmed
            } else {
                // The OS "did something" but this platform cannot verify the
                // window is frontmost (Wayland) → report as merely requested.
                FocusOutcome::Requested
            }
        }
        BackendResult::Requested => FocusOutcome::Requested,
        BackendResult::Failed => {
            if strategy.notify_on_fallback && backend.notify_fallback(focus_hint) {
                FocusOutcome::FellBackToNotification
            } else {
                FocusOutcome::Failed
            }
        }
    }
}

/// Convenience: select the strategy for `platform` and focus through `backend`.
pub fn focus_on_platform(
    backend: &dyn FocusBackend,
    platform: FocusPlatform,
    focus_hint: &str,
) -> FocusOutcome {
    focus_window(backend, FocusStrategy::for_platform(platform), focus_hint)
}

// ── A mocked OS backend (also usable by host integration tests) ───────────────

/// A scriptable [`FocusBackend`] for tests: it returns a fixed [`BackendResult`]
/// and a fixed notification-availability, and records every call so tests can
/// assert *which mechanism* the strategy selected.
///
/// Lives in the crate (not behind `#[cfg(test)]`) so the host shell's own
/// integration tests can reuse it to exercise the focus path with no real OS.
#[derive(Debug)]
pub struct MockBackend {
    result: BackendResult,
    fallback_available: bool,
    calls: std::cell::RefCell<Vec<(FocusMechanism, String)>>,
    fallback_calls: std::cell::RefCell<Vec<String>>,
}

impl MockBackend {
    /// A backend that reports `result` for every activation and has no fallback.
    pub fn new(result: BackendResult) -> Self {
        Self {
            result,
            fallback_available: false,
            calls: std::cell::RefCell::new(Vec::new()),
            fallback_calls: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// Enable the notification fallback (so a `Failed` activation can fall back).
    pub fn with_fallback(mut self, available: bool) -> Self {
        self.fallback_available = available;
        self
    }

    /// The mechanism + focus_hint of every `activate` call, in order.
    pub fn activations(&self) -> Vec<(FocusMechanism, String)> {
        self.calls.borrow().clone()
    }

    /// The focus_hints of every `notify_fallback` call, in order.
    pub fn fallback_notifications(&self) -> Vec<String> {
        self.fallback_calls.borrow().clone()
    }
}

impl FocusBackend for MockBackend {
    fn activate(&self, mechanism: FocusMechanism, focus_hint: &str) -> BackendResult {
        self.calls
            .borrow_mut()
            .push((mechanism, focus_hint.to_string()));
        self.result
    }

    fn notify_fallback(&self, focus_hint: &str) -> bool {
        self.fallback_calls
            .borrow_mut()
            .push(focus_hint.to_string());
        self.fallback_available
    }
}

// ── Unit tests (mocked OS backend; no real codex/VS Code/window manager) ──────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InboxModel, TabState};
    use fleet_protocol::{
        Event, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind, Session, State,
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

    /// Build a view of sessions, marking the ones in `unread_ids` as unread.
    fn view_with(ids: &[&str], unread_ids: &[&str]) -> InboxView {
        let mut m = InboxModel::new();
        let sessions: Vec<Session> = ids
            .iter()
            .map(|id| {
                let mut s = Session::new(
                    *id,
                    *id,
                    loc(),
                    srv(),
                    State::Waiting,
                    "2026-06-08T00:00:00Z",
                );
                s.unread = unread_ids.contains(id);
                s
            })
            .collect();
        m.apply(Event::snapshot(sessions));
        m.view()
    }

    // ── Part 1: pure selection ────────────────────────────────────────────────

    #[test]
    fn focus_command_targets_session() {
        let c = focus_command("s1");
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["command"], "focus");
        assert_eq!(v["target"]["type"], "session");
        assert_eq!(v["target"]["session_id"], "s1");
    }

    #[test]
    fn next_unread_empty_view_is_none() {
        let v = InboxView::default();
        assert_eq!(next_unread(&v, None), None);
        assert_eq!(next_unread(&v, Some(0)), None);
    }

    #[test]
    fn next_unread_no_cursor_finds_first_unread() {
        let v = view_with(&["a", "b", "c"], &["b", "c"]);
        assert_eq!(next_unread(&v, None), Some(1)); // "b"
    }

    #[test]
    fn next_unread_none_when_nothing_unread() {
        let v = view_with(&["a", "b", "c"], &[]);
        assert_eq!(next_unread(&v, None), None);
    }

    #[test]
    fn next_unread_advances_past_cursor() {
        let v = view_with(&["a", "b", "c"], &["a", "c"]);
        // Cursor on "a" (idx 0) → next unread after it is "c" (idx 2).
        assert_eq!(next_unread(&v, Some(0)), Some(2));
    }

    #[test]
    fn next_unread_wraps_around() {
        let v = view_with(&["a", "b", "c"], &["a"]);
        // Cursor on "c" (idx 2) → wrap → "a" (idx 0).
        assert_eq!(next_unread(&v, Some(2)), Some(0));
    }

    #[test]
    fn next_unread_cycles_through_all_unread_in_order() {
        let v = view_with(&["a", "b", "c", "d"], &["b", "d"]);
        // Jump from no cursor: b(1) → d(3) → wrap → b(1) ...
        let first = next_unread(&v, None).unwrap();
        assert_eq!(first, 1);
        let second = next_unread(&v, Some(first)).unwrap();
        assert_eq!(second, 3);
        let third = next_unread(&v, Some(second)).unwrap();
        assert_eq!(third, 1, "wraps back to the first unread");
    }

    #[test]
    fn next_unread_on_self_finds_other_unread() {
        let v = view_with(&["a", "b"], &["a", "b"]);
        // On "a" (unread) → next is "b", not "a" again.
        assert_eq!(next_unread(&v, Some(0)), Some(1));
    }

    #[test]
    fn next_unread_single_unread_returns_to_self() {
        let v = view_with(&["a", "b"], &["a"]);
        // Only "a" is unread; from "a" it wraps back to "a".
        assert_eq!(next_unread(&v, Some(0)), Some(0));
    }

    #[test]
    fn next_unread_tab_borrows_the_tab() {
        let v = view_with(&["a", "b", "c"], &["c"]);
        let (idx, tab) = next_unread_tab(&v, None).unwrap();
        assert_eq!(idx, 2);
        assert_eq!(tab.session_id, "c");
        assert!(tab.unread);
        assert_eq!(tab.state, TabState::Waiting);
        // And it composes into a focus command.
        let c = focus_command(&tab.session_id);
        let val = serde_json::to_value(&c).unwrap();
        assert_eq!(val["target"]["session_id"], "c");
    }

    #[test]
    fn cycle_next_wraps_and_handles_empty() {
        let v = view_with(&["a", "b", "c"], &[]);
        assert_eq!(cycle_next(&v, None), Some(0));
        assert_eq!(cycle_next(&v, Some(0)), Some(1));
        assert_eq!(cycle_next(&v, Some(2)), Some(0)); // wrap
        let empty = InboxView::default();
        assert_eq!(cycle_next(&empty, None), None);
    }

    // ── Part 2: per-OS strategy selection (per cfg) ───────────────────────────

    #[test]
    fn macos_strategy_uses_applescript_and_can_confirm() {
        let s = FocusStrategy::for_platform(FocusPlatform::MacOs);
        assert_eq!(s.mechanism, FocusMechanism::AppleScript);
        assert!(
            s.confirmation_possible,
            "macOS AppleScript activation can confirm frontmost"
        );
        assert!(s.notify_on_fallback);
    }

    #[test]
    fn x11_strategy_uses_wmctrl_and_can_confirm() {
        let s = FocusStrategy::for_platform(FocusPlatform::LinuxX11);
        assert_eq!(s.mechanism, FocusMechanism::X11Wmctrl);
        assert!(
            s.confirmation_possible,
            "X11 WM confirms _NET_ACTIVE_WINDOW"
        );
    }

    #[test]
    fn wayland_strategy_uses_token_and_never_confirms() {
        let s = FocusStrategy::for_platform(FocusPlatform::Wayland);
        assert_eq!(s.mechanism, FocusMechanism::XdgActivationToken);
        assert!(
            !s.confirmation_possible,
            "Wayland foreign-window activation can never be guaranteed → never promise auto-focus"
        );
        assert!(
            s.notify_on_fallback,
            "Wayland relies on the notification fallback"
        );
    }

    #[test]
    fn other_platform_strategy_is_best_effort_unconfirmable() {
        let s = FocusStrategy::for_platform(FocusPlatform::Other);
        assert_eq!(s.mechanism, FocusMechanism::BestEffort);
        assert!(!s.confirmation_possible);
    }

    #[test]
    fn each_platform_selects_a_distinct_mechanism() {
        use std::collections::HashSet;
        let mechs: HashSet<FocusMechanism> = [
            FocusPlatform::MacOs,
            FocusPlatform::LinuxX11,
            FocusPlatform::Wayland,
            FocusPlatform::Other,
        ]
        .iter()
        .map(|&p| FocusStrategy::for_platform(p).mechanism)
        .collect();
        assert_eq!(mechs.len(), 4, "each platform picks a distinct mechanism");
    }

    #[test]
    fn current_delegates_to_detect_and_for_platform() {
        // `current()` must be exactly `for_platform(detect())` — driving the
        // real platform-detection path and proving the convenience wiring.
        let detected = FocusPlatform::detect();
        let current = FocusStrategy::current();
        assert_eq!(current.platform, detected);
        assert_eq!(current, FocusStrategy::for_platform(detected));
        // Whatever the host platform, the detected strategy is internally
        // consistent (its mechanism matches what `for_platform` assigns).
        assert_eq!(current.mechanism, FocusStrategy::for_platform(detected).mechanism);
    }

    /// On the macOS build host, detection resolves to macOS (compile-target arm).
    #[cfg(target_os = "macos")]
    #[test]
    fn detect_resolves_to_macos_on_macos_target() {
        assert_eq!(FocusPlatform::detect(), FocusPlatform::MacOs);
    }

    /// A backend that does not override `notify_fallback` exercises the trait's
    /// default impl (no fallback available), so a Failed activation → Failed.
    #[test]
    fn default_notify_fallback_yields_failed_outcome() {
        struct NoFallbackBackend;
        impl FocusBackend for NoFallbackBackend {
            fn activate(&self, _mechanism: FocusMechanism, _focus_hint: &str) -> BackendResult {
                BackendResult::Failed
            }
            // notify_fallback intentionally NOT overridden → trait default.
        }
        let outcome = focus_on_platform(&NoFallbackBackend, FocusPlatform::MacOs, "code");
        assert_eq!(
            outcome,
            FocusOutcome::Failed,
            "with no fallback available, a failed activation must report Failed"
        );
    }

    // ── Part 2: confirmation telemetry — success paths ────────────────────────

    #[test]
    fn macos_activated_is_confirmed() {
        let backend = MockBackend::new(BackendResult::Activated);
        let outcome = focus_on_platform(&backend, FocusPlatform::MacOs, "code --reuse-window");
        assert_eq!(outcome, FocusOutcome::Confirmed);
        assert!(outcome.is_confirmed_success());
        // It used the AppleScript mechanism with the given focus hint.
        assert_eq!(
            backend.activations(),
            vec![(
                FocusMechanism::AppleScript,
                "code --reuse-window".to_string()
            )]
        );
    }

    #[test]
    fn x11_activated_is_confirmed() {
        let backend = MockBackend::new(BackendResult::Activated);
        let outcome = focus_on_platform(&backend, FocusPlatform::LinuxX11, "wid:0x123");
        assert_eq!(outcome, FocusOutcome::Confirmed);
        assert!(outcome.is_confirmed_success());
        assert_eq!(backend.activations()[0].0, FocusMechanism::X11Wmctrl);
    }

    #[test]
    fn wayland_activated_is_only_requested_never_confirmed() {
        // Even if the Wayland backend "activated", we MUST NOT claim confirmed:
        // foreign-window activation can't be guaranteed.
        let backend = MockBackend::new(BackendResult::Activated);
        let outcome = focus_on_platform(&backend, FocusPlatform::Wayland, "code");
        assert_eq!(
            outcome,
            FocusOutcome::Requested,
            "Wayland never promises auto-focus, even on a reported activation"
        );
        assert!(
            !outcome.is_confirmed_success(),
            "the UI must NEVER claim focus succeeded on Wayland"
        );
        assert!(outcome.took_action());
    }

    #[test]
    fn requested_backend_result_is_requested_on_all_platforms() {
        for p in [
            FocusPlatform::MacOs,
            FocusPlatform::LinuxX11,
            FocusPlatform::Wayland,
            FocusPlatform::Other,
        ] {
            let backend = MockBackend::new(BackendResult::Requested);
            let outcome = focus_on_platform(&backend, p, "hint");
            assert_eq!(
                outcome,
                FocusOutcome::Requested,
                "a merely-requested activation is never confirmed on {p:?}"
            );
            assert!(!outcome.is_confirmed_success());
        }
    }

    // ── Part 2: confirmation telemetry — failure paths ────────────────────────

    #[test]
    fn failed_with_fallback_falls_back_to_notification() {
        let backend = MockBackend::new(BackendResult::Failed).with_fallback(true);
        let outcome = focus_on_platform(&backend, FocusPlatform::MacOs, "code");
        assert_eq!(outcome, FocusOutcome::FellBackToNotification);
        assert!(!outcome.is_confirmed_success());
        assert!(
            outcome.took_action(),
            "a fallback notification is still action"
        );
        // The fallback notification was raised with the focus hint.
        assert_eq!(backend.fallback_notifications(), vec!["code".to_string()]);
    }

    #[test]
    fn failed_without_fallback_is_failed() {
        let backend = MockBackend::new(BackendResult::Failed).with_fallback(false);
        let outcome = focus_on_platform(&backend, FocusPlatform::LinuxX11, "wid:0x1");
        assert_eq!(outcome, FocusOutcome::Failed);
        assert!(!outcome.is_confirmed_success());
        assert!(!outcome.took_action());
    }

    #[test]
    fn wayland_failed_falls_back_to_notification() {
        // Wayland's primary safety net is the notification fallback.
        let backend = MockBackend::new(BackendResult::Failed).with_fallback(true);
        let outcome = focus_on_platform(&backend, FocusPlatform::Wayland, "code .");
        assert_eq!(outcome, FocusOutcome::FellBackToNotification);
        assert_eq!(backend.fallback_notifications(), vec!["code .".to_string()]);
    }

    // ── The honesty invariant (focus analog of invariant 5) ───────────────────

    #[test]
    fn confirmed_is_the_only_success_and_requires_real_activation() {
        // Exhaustive truth table over (platform × backend result): assert that
        // `is_confirmed_success` is true ONLY for a confirmable platform that the
        // backend reported `Activated` for. The UI never overstates success.
        let platforms = [
            FocusPlatform::MacOs,
            FocusPlatform::LinuxX11,
            FocusPlatform::Wayland,
            FocusPlatform::Other,
        ];
        let results = [
            BackendResult::Activated,
            BackendResult::Requested,
            BackendResult::Failed,
        ];
        for p in platforms {
            let strat = FocusStrategy::for_platform(p);
            for r in results {
                let backend = MockBackend::new(r).with_fallback(true);
                let outcome = focus_window(&backend, strat, "hint");
                let should_be_confirmed =
                    strat.confirmation_possible && r == BackendResult::Activated;
                assert_eq!(
                    outcome.is_confirmed_success(),
                    should_be_confirmed,
                    "platform {p:?} + backend {r:?}: confirmed-success must equal \
                     (confirmation_possible && Activated)"
                );
            }
        }
    }

    #[test]
    fn no_platform_ever_confirms_a_failed_activation() {
        for p in [
            FocusPlatform::MacOs,
            FocusPlatform::LinuxX11,
            FocusPlatform::Wayland,
            FocusPlatform::Other,
        ] {
            let backend = MockBackend::new(BackendResult::Failed).with_fallback(true);
            let outcome = focus_on_platform(&backend, p, "hint");
            assert!(
                !outcome.is_confirmed_success(),
                "{p:?} must never confirm a failed activation"
            );
        }
    }

    #[test]
    fn outcome_labels_are_distinct_and_nonempty() {
        use std::collections::HashSet;
        let labels: HashSet<&str> = [
            FocusOutcome::Confirmed,
            FocusOutcome::Requested,
            FocusOutcome::FellBackToNotification,
            FocusOutcome::Failed,
        ]
        .iter()
        .map(|o| o.label())
        .collect();
        assert_eq!(
            labels.len(),
            4,
            "every outcome has a distinct telemetry label"
        );
        assert!(labels.iter().all(|l| !l.is_empty()));
    }

    // ── End-to-end: select an unread tab and focus it ─────────────────────────

    #[test]
    fn jump_to_next_unread_then_focus_confirmed_on_macos() {
        let v = view_with(&["a", "b", "c"], &["c"]);
        let (_, tab) = next_unread_tab(&v, None).expect("an unread tab exists");
        let cmd = focus_command(&tab.session_id);
        // The command targets the unread session.
        let val = serde_json::to_value(&cmd).unwrap();
        assert_eq!(val["target"]["session_id"], "c");
        // And focusing it on macOS, with the OS confirming, yields Confirmed.
        let backend = MockBackend::new(BackendResult::Activated);
        let outcome = focus_on_platform(&backend, FocusPlatform::MacOs, "code --goto c");
        assert!(outcome.is_confirmed_success());
    }

    #[test]
    fn jump_with_nothing_unread_yields_no_focus_command() {
        let v = view_with(&["a", "b"], &[]);
        assert!(next_unread_tab(&v, None).is_none(), "nothing to jump to");
    }
}
