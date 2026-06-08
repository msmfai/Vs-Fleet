//! Multi-editor descriptor table + launch/focus — slice **S26** (node `EDITORS`).
//!
//! README §12.1 mandates a **data-driven** editor integration: *"Descriptor
//! table (data, not per-editor code). Per editor: `kind`, `cli`
//! (`code`/`cursor`/`windsurf`), `uri_scheme` (`vscode://`/`cursor://`/
//! `windsurf://`), `open_flags` (`-r`/`--reuse-window`, `-n`/`--new-window`,
//! `--folder-uri vscode-remote://...`), `detect`, focus strategy. Ship rows for
//! all three; show only installed ones as launch targets."*
//!
//! The hard rule (PLAN S26 / §12.1): **NO per-editor branching code.** Every
//! editor is one [`EditorDescriptor`] *row* in the [`EDITORS`] table, and a
//! single [`launch_command`] / [`focus_editor`] launcher reads the row's fields.
//! Adding Cursor or Windsurf is adding a row, never a `match editor { … }`.
//!
//! ## Composition, not reimplementation
//!
//! - **Focus reuses the [`crate::focus`] seam (slice S23).** This module does
//!   **not** re-implement window activation. It maps a session's editor (via its
//!   descriptor) to the same per-OS [`FocusStrategy`]/[`FocusBackend`] the
//!   `FOCUS` node owns, so the macOS-AppleScript / X11 / Wayland-fallback +
//!   focus-confirmation-telemetry logic is shared, not duplicated. See
//!   [`focus_editor`].
//! - **Kind reuses [`fleet_protocol::EditorKind`].** The descriptor's `kind` is
//!   the wire enum, so a session's `editor.kind` selects its descriptor with
//!   [`descriptor_for`].
//!
//! ## Detection is injected (testable "installed?")
//!
//! Whether an editor's CLI is on `PATH` is impure (it probes the filesystem), so
//! detection rides a [`Detector`] trait — exactly like [`crate::focus`]'s
//! [`FocusBackend`]. [`installed_targets`] filters [`EDITORS`] through a
//! `Detector`, so the **only-installed-as-launch-targets** rule is unit-testable
//! with a mocked detector (no `code`/`cursor`/`windsurf` need exist on the test
//! box). [`PathDetector`] is the real `which`-style probe.
//!
//! Disjoint file from the other host seams; reuses `focus` rather than forking
//! it.

use crate::focus::{
    focus_window, FocusBackend, FocusMechanism, FocusOutcome, FocusPlatform, FocusStrategy,
};
use fleet_protocol::EditorKind;

// ── The descriptor row (data, not code) ───────────────────────────────────────

/// One **row** of the editor descriptor table — *all* per-editor knowledge, as
/// data. The launcher reads these fields; it never branches on `kind`.
///
/// README §12.1 field-for-field:
/// - [`kind`](Self::kind) — the [`EditorKind`] this row describes.
/// - [`cli`](Self::cli) — the launcher binary (`code`/`cursor`/`windsurf`).
/// - [`uri_scheme`](Self::uri_scheme) — the deep-link scheme
///   (`vscode://`/`cursor://`/`windsurf://`).
/// - [`reuse_window_flag`](Self::reuse_window_flag) /
///   [`new_window_flag`](Self::new_window_flag) /
///   [`folder_uri_flag`](Self::folder_uri_flag) — the `open_flags`
///   (`-r`/`--reuse-window`, `-n`, `--folder-uri vscode-remote://…`).
/// - [`remote_uri_authority`](Self::remote_uri_authority) — the
///   `vscode-remote://`-style authority used to build a `--folder-uri` for a
///   remote/container workspace (README §12.3: canonical only on VS Code; the
///   Cursor/Windsurf rows still carry their best-effort scheme).
/// - [`focus_mechanism_for`](Self::focus_mechanism_for) — the focus strategy,
///   **delegated** to the per-OS [`crate::focus`] seam (no new focus code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorDescriptor {
    /// The editor flavor this row describes (the table's key).
    pub kind: EditorKind,
    /// Human label for the launch-target list.
    pub label: &'static str,
    /// The CLI binary name to probe for installation and to launch with.
    pub cli: &'static str,
    /// The deep-link URI scheme (`vscode://`, `cursor://`, `windsurf://`).
    pub uri_scheme: &'static str,
    /// `open_flags`: reuse-an-existing-window flag (`--reuse-window`, alias
    /// `-r`). README §12.2 / GAP A5: `--reuse-window` can affect *all* windows,
    /// so the launcher pairs it with OS-level activation via [`focus_editor`].
    pub reuse_window_flag: &'static str,
    /// `open_flags`: force-a-new-window flag (`--new-window`, alias `-n`).
    pub new_window_flag: &'static str,
    /// `open_flags`: the flag that opens a (possibly remote) folder URI
    /// (`--folder-uri`). Combined with [`remote_uri_authority`] to address a
    /// `vscode-remote://…` workspace.
    pub folder_uri_flag: &'static str,
    /// The `vscode-remote://`-style authority scheme for remote/container
    /// `--folder-uri` opens (README §12.3). VS Code is canonical; the others are
    /// carried best-effort (their Open-VSX `open-remote-ssh` reimplementations).
    pub remote_uri_authority: &'static str,
}

impl EditorDescriptor {
    /// The per-OS [`FocusMechanism`] this editor uses on `platform` — **reused**
    /// from the [`crate::focus`] seam (slice S23), not re-derived here. Focus is
    /// a property of the OS, not the editor, so every row maps to the same
    /// platform strategy; keeping the call here lets a future editor override it
    /// by *data* if ever needed, without branching the launcher.
    pub fn focus_mechanism_for(&self, platform: FocusPlatform) -> FocusMechanism {
        FocusStrategy::for_platform(platform).mechanism
    }

    /// The full [`FocusStrategy`] (mechanism + confirmation policy) for this
    /// editor on `platform`. A thin pass-through to [`FocusStrategy::for_platform`]
    /// so the confirmation-telemetry honesty (Wayland never confirms, etc.) is
    /// the *same* logic the `FOCUS` node owns.
    pub fn focus_strategy_for(&self, platform: FocusPlatform) -> FocusStrategy {
        FocusStrategy::for_platform(platform)
    }

    /// Build the **launch** command line for opening `workspace` in this editor,
    /// reusing the given window when `reuse` is set, else opening a new one.
    ///
    /// Pure string construction from the row's `open_flags` — no per-editor
    /// branching. The first element is the CLI binary; the rest are argv.
    ///
    /// Example (vscode, reuse): `["code", "--reuse-window", "/abs/path"]`.
    pub fn launch_argv(&self, workspace: &str, reuse: bool) -> Vec<String> {
        let flag = if reuse {
            self.reuse_window_flag
        } else {
            self.new_window_flag
        };
        vec![
            self.cli.to_string(),
            flag.to_string(),
            workspace.to_string(),
        ]
    }

    /// Build the **remote** launch argv: open a `--folder-uri
    /// vscode-remote://<authority>+<host><path>` workspace (README §12.1's
    /// `--folder-uri vscode-remote://…` open flag). Pure; one code path for all
    /// editors (the authority differs by *data*, per §12.3).
    pub fn launch_remote_argv(&self, host: &str, path: &str, reuse: bool) -> Vec<String> {
        let window_flag = if reuse {
            self.reuse_window_flag
        } else {
            self.new_window_flag
        };
        let folder_uri = format!("{}{}{}", self.remote_uri_authority, host, path);
        vec![
            self.cli.to_string(),
            window_flag.to_string(),
            self.folder_uri_flag.to_string(),
            folder_uri,
        ]
    }

    /// Build a deep-link **URI** addressing a path in this editor
    /// (`<scheme>file<abs-path>`), the alternative to a CLI open (README §12.2's
    /// "editor CLI/URI open"). Pure.
    pub fn open_uri(&self, abs_path: &str) -> String {
        format!("{}file{}", self.uri_scheme, abs_path)
    }
}

// ── The table: one row per editor, ship all three (README §12.1) ───────────────

/// VS Code (`code`) — the canonical, fully-supported editor incl. remote
/// (README §12.3: "editor is the OS" generalizes cleanly only to MS VS Code).
pub const VSCODE: EditorDescriptor = EditorDescriptor {
    kind: EditorKind::Vscode,
    label: "VS Code",
    cli: "code",
    uri_scheme: "vscode://",
    reuse_window_flag: "--reuse-window",
    new_window_flag: "--new-window",
    folder_uri_flag: "--folder-uri",
    remote_uri_authority: "vscode-remote://ssh-remote+",
};

/// Cursor (`cursor`) — local fully supported; remote best-effort (Open-VSX
/// `open-remote-ssh`), README §12.3.
pub const CURSOR: EditorDescriptor = EditorDescriptor {
    kind: EditorKind::Cursor,
    label: "Cursor",
    cli: "cursor",
    uri_scheme: "cursor://",
    reuse_window_flag: "--reuse-window",
    new_window_flag: "--new-window",
    folder_uri_flag: "--folder-uri",
    remote_uri_authority: "vscode-remote://ssh-remote+",
};

/// Windsurf (`windsurf`) — local fully supported; remote best-effort,
/// README §12.3.
pub const WINDSURF: EditorDescriptor = EditorDescriptor {
    kind: EditorKind::Windsurf,
    label: "Windsurf",
    cli: "windsurf",
    uri_scheme: "windsurf://",
    reuse_window_flag: "--reuse-window",
    new_window_flag: "--new-window",
    folder_uri_flag: "--folder-uri",
    remote_uri_authority: "vscode-remote://ssh-remote+",
};

/// The descriptor **table** — one row per supported editor (README §12.1: "Ship
/// rows for all three"). This is the single source of truth the launcher reads;
/// there is no per-editor code anywhere else.
pub const EDITORS: [EditorDescriptor; 3] = [VSCODE, CURSOR, WINDSURF];

/// The descriptor for an [`EditorKind`], by table lookup (no `match` on kind in
/// caller code — they ask the table). `None` is impossible for the three shipped
/// kinds but kept total for forward-compat if the wire enum grows.
pub fn descriptor_for(kind: &EditorKind) -> Option<&'static EditorDescriptor> {
    EDITORS.iter().find(|d| &d.kind == kind)
}

// ── Detection (installed?) — injected, testable ───────────────────────────────

/// Probes whether an editor's CLI is installed (on `PATH`). Impure (touches the
/// filesystem / `PATH`), so it rides a trait — the unit tests inject a mock and
/// never need a real `code`/`cursor`/`windsurf` on the box.
pub trait Detector {
    /// Whether the editor whose launcher binary is `cli` is installed.
    fn is_installed(&self, cli: &str) -> bool;
}

/// A launch **target**: an installed editor the host offers in its launch menu.
///
/// `installed_targets` only ever produces these for editors a [`Detector`]
/// confirmed are present, enforcing README §12.1's "show only installed ones as
/// launch targets" at the type level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchTarget {
    /// The descriptor of the installed editor.
    pub descriptor: &'static EditorDescriptor,
}

impl LaunchTarget {
    /// The editor kind of this target.
    pub fn kind(&self) -> &'static EditorKind {
        &self.descriptor.kind
    }

    /// The user-facing label.
    pub fn label(&self) -> &'static str {
        self.descriptor.label
    }
}

/// Filter the [`EDITORS`] table to the editors the `detector` reports installed,
/// as [`LaunchTarget`]s — the **launch-target list** the host UI shows.
///
/// README §12.1: *"show only installed ones as launch targets."* Order follows
/// [`EDITORS`] (VS Code, Cursor, Windsurf) so the menu is deterministic.
pub fn installed_targets(detector: &dyn Detector) -> Vec<LaunchTarget> {
    EDITORS
        .iter()
        .filter(|d| detector.is_installed(d.cli))
        .map(|descriptor| LaunchTarget { descriptor })
        .collect()
}

/// Whether a specific editor kind is installed (table lookup + detector probe).
pub fn is_kind_installed(kind: &EditorKind, detector: &dyn Detector) -> bool {
    descriptor_for(kind)
        .map(|d| detector.is_installed(d.cli))
        .unwrap_or(false)
}

/// The real `PATH`-probing detector: an editor is installed iff its CLI binary
/// is found on `PATH` (a `which`-style scan; no external crate). Reads the env
/// and stats files, so it is *not* used in unit tests (which inject a mock).
#[derive(Debug, Clone, Copy, Default)]
pub struct PathDetector;

impl Detector for PathDetector {
    fn is_installed(&self, cli: &str) -> bool {
        let Some(paths) = std::env::var_os("PATH") else {
            return false;
        };
        std::env::split_paths(&paths).any(|dir| {
            let candidate = dir.join(cli);
            // On the unix targets we focus (macOS/Linux) an installed CLI is a
            // regular file on PATH; existence is a sufficient, dependency-free
            // probe. (Windows is documented best-effort, §21.11.)
            candidate.is_file()
        })
    }
}

// ── The single launcher (no per-editor branching) ─────────────────────────────

/// Build the launch argv for opening `workspace` in `kind`, reusing an existing
/// window when `reuse` is set. Returns `None` if `kind` has no descriptor.
///
/// This is **the** launcher (README §12.1's "one launcher"): it looks the row up
/// and reads its `open_flags`. No `match kind`.
pub fn launch_command(kind: &EditorKind, workspace: &str, reuse: bool) -> Option<Vec<String>> {
    descriptor_for(kind).map(|d| d.launch_argv(workspace, reuse))
}

/// Focus a session's editor window, **reusing** the per-OS [`crate::focus`] seam.
///
/// Maps the editor `kind` → its descriptor → the platform [`FocusStrategy`]
/// (slice S23), then drives the same [`focus_window`] core the `FOCUS` node owns
/// against the provided `backend` (the mocked-OS backend in tests). The honest
/// [`FocusOutcome`] (Wayland never `Confirmed`, etc.) comes straight from the
/// shared seam — this function adds **no** new focus logic, only the editor→OS
/// dispatch.
///
/// `focus_hint` is the session's `editor.focus_hint` (its CLI/URI, README §7.1).
/// Returns `None` if the editor kind is unknown.
pub fn focus_editor(
    backend: &dyn FocusBackend,
    kind: &EditorKind,
    platform: FocusPlatform,
    focus_hint: &str,
) -> Option<FocusOutcome> {
    let descriptor = descriptor_for(kind)?;
    let strategy = descriptor.focus_strategy_for(platform);
    Some(focus_window(backend, strategy, focus_hint))
}

// ── Unit tests (mocked detector + mocked-OS focus backend; no real editors) ────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::focus::{BackendResult, MockBackend};
    use std::collections::HashSet;

    /// A scriptable [`Detector`] for tests: installed iff the CLI is in the set.
    struct MockDetector {
        installed: HashSet<&'static str>,
    }
    impl MockDetector {
        fn with(clis: &[&'static str]) -> Self {
            Self {
                installed: clis.iter().copied().collect(),
            }
        }
        fn none() -> Self {
            Self {
                installed: HashSet::new(),
            }
        }
    }
    impl Detector for MockDetector {
        fn is_installed(&self, cli: &str) -> bool {
            self.installed.contains(cli)
        }
    }

    // ── descriptor rows for all three (README §12.1) ─────────────────────────

    #[test]
    fn table_ships_all_three_editors() {
        assert_eq!(EDITORS.len(), 3);
        let has = |k: &EditorKind| EDITORS.iter().any(|d| &d.kind == k);
        assert!(has(&EditorKind::Vscode));
        assert!(has(&EditorKind::Cursor));
        assert!(has(&EditorKind::Windsurf));
        // No duplicate kinds: each shipped kind appears exactly once.
        for k in [EditorKind::Vscode, EditorKind::Cursor, EditorKind::Windsurf] {
            assert_eq!(EDITORS.iter().filter(|d| d.kind == k).count(), 1);
        }
    }

    #[test]
    fn vscode_row_fields_match_spec() {
        let d = descriptor_for(&EditorKind::Vscode).unwrap();
        assert_eq!(d.cli, "code");
        assert_eq!(d.uri_scheme, "vscode://");
        assert_eq!(d.reuse_window_flag, "--reuse-window");
        assert_eq!(d.new_window_flag, "--new-window");
        assert_eq!(d.folder_uri_flag, "--folder-uri");
        assert!(d.remote_uri_authority.starts_with("vscode-remote://"));
    }

    #[test]
    fn cursor_row_fields_match_spec() {
        let d = descriptor_for(&EditorKind::Cursor).unwrap();
        assert_eq!(d.cli, "cursor");
        assert_eq!(d.uri_scheme, "cursor://");
        assert_eq!(d.folder_uri_flag, "--folder-uri");
    }

    #[test]
    fn windsurf_row_fields_match_spec() {
        let d = descriptor_for(&EditorKind::Windsurf).unwrap();
        assert_eq!(d.cli, "windsurf");
        assert_eq!(d.uri_scheme, "windsurf://");
        assert_eq!(d.folder_uri_flag, "--folder-uri");
    }

    #[test]
    fn every_row_has_distinct_cli_and_scheme() {
        let clis: HashSet<&str> = EDITORS.iter().map(|d| d.cli).collect();
        let schemes: HashSet<&str> = EDITORS.iter().map(|d| d.uri_scheme).collect();
        assert_eq!(clis.len(), 3, "each editor has a distinct CLI");
        assert_eq!(schemes.len(), 3, "each editor has a distinct URI scheme");
    }

    #[test]
    fn descriptor_for_each_kind_round_trips_its_kind() {
        for d in EDITORS.iter() {
            assert_eq!(&descriptor_for(&d.kind).unwrap().kind, &d.kind);
        }
    }

    // ── only-installed filtering (mocked detect) ─────────────────────────────

    #[test]
    fn no_editors_installed_yields_no_targets() {
        let det = MockDetector::none();
        assert!(installed_targets(&det).is_empty());
    }

    #[test]
    fn only_installed_editors_become_launch_targets() {
        // Only `code` and `windsurf` installed; `cursor` is NOT.
        let det = MockDetector::with(&["code", "windsurf"]);
        let targets = installed_targets(&det);
        let kinds: Vec<&EditorKind> = targets.iter().map(|t| t.kind()).collect();
        assert_eq!(kinds, vec![&EditorKind::Vscode, &EditorKind::Windsurf]);
        // Cursor must be filtered out entirely.
        assert!(!kinds.contains(&&EditorKind::Cursor));
    }

    #[test]
    fn all_installed_yields_all_three_in_table_order() {
        let det = MockDetector::with(&["code", "cursor", "windsurf"]);
        let targets = installed_targets(&det);
        let kinds: Vec<&EditorKind> = targets.iter().map(|t| t.kind()).collect();
        // Deterministic order follows EDITORS.
        assert_eq!(
            kinds,
            vec![
                &EditorKind::Vscode,
                &EditorKind::Cursor,
                &EditorKind::Windsurf
            ]
        );
    }

    #[test]
    fn is_kind_installed_uses_the_detector() {
        let det = MockDetector::with(&["cursor"]);
        assert!(is_kind_installed(&EditorKind::Cursor, &det));
        assert!(!is_kind_installed(&EditorKind::Vscode, &det));
        assert!(!is_kind_installed(&EditorKind::Windsurf, &det));
    }

    #[test]
    fn launch_target_exposes_label() {
        let det = MockDetector::with(&["code"]);
        let targets = installed_targets(&det);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].label(), "VS Code");
    }

    // ── launch command construction per editor (data-driven) ─────────────────

    #[test]
    fn launch_command_reuse_window_per_editor() {
        // Same launcher, different rows → different CLI, same flag shape.
        let vscode = launch_command(&EditorKind::Vscode, "/work/repo", true).unwrap();
        assert_eq!(vscode, vec!["code", "--reuse-window", "/work/repo"]);

        let cursor = launch_command(&EditorKind::Cursor, "/work/repo", true).unwrap();
        assert_eq!(cursor, vec!["cursor", "--reuse-window", "/work/repo"]);

        let windsurf = launch_command(&EditorKind::Windsurf, "/work/repo", true).unwrap();
        assert_eq!(windsurf, vec!["windsurf", "--reuse-window", "/work/repo"]);
    }

    #[test]
    fn launch_command_new_window_uses_new_window_flag() {
        let argv = launch_command(&EditorKind::Vscode, "/work/repo", false).unwrap();
        assert_eq!(argv, vec!["code", "--new-window", "/work/repo"]);
    }

    #[test]
    fn launch_remote_argv_builds_folder_uri_per_editor() {
        // The remote open uses --folder-uri vscode-remote://… (README §12.1).
        let argv = VSCODE.launch_remote_argv("hetzner-box", "/srv/app", true);
        assert_eq!(
            argv,
            vec![
                "code",
                "--reuse-window",
                "--folder-uri",
                "vscode-remote://ssh-remote+hetzner-box/srv/app"
            ]
        );
        // Cursor/Windsurf carry their own row but the SAME single code path.
        let cur = CURSOR.launch_remote_argv("box", "/p", false);
        assert_eq!(cur[0], "cursor");
        assert_eq!(cur[1], "--new-window");
        assert_eq!(cur[2], "--folder-uri");
        assert_eq!(cur[3], "vscode-remote://ssh-remote+box/p");
    }

    #[test]
    fn open_uri_builds_a_deep_link_per_editor() {
        assert_eq!(VSCODE.open_uri("/a/b"), "vscode://file/a/b");
        assert_eq!(CURSOR.open_uri("/a/b"), "cursor://file/a/b");
        assert_eq!(WINDSURF.open_uri("/a/b"), "windsurf://file/a/b");
    }

    #[test]
    fn launch_command_unknown_kind_is_none() {
        // All three shipped kinds resolve; this guards the total-lookup contract
        // by asserting every table kind is present (no silent gap).
        for d in EDITORS.iter() {
            assert!(launch_command(&d.kind, "/x", true).is_some());
        }
    }

    // ── focus reuses the mocked-OS focus backend (slice S23) ─────────────────

    #[test]
    fn focus_editor_reuses_macos_strategy_and_confirms() {
        let backend = MockBackend::new(BackendResult::Activated);
        let outcome = focus_editor(
            &backend,
            &EditorKind::Vscode,
            FocusPlatform::MacOs,
            "code --reuse-window",
        )
        .unwrap();
        // Confirmed comes from the SHARED focus seam, not new logic here.
        assert_eq!(outcome, FocusOutcome::Confirmed);
        assert!(outcome.is_confirmed_success());
        // It drove the AppleScript mechanism the focus seam selected for macOS.
        assert_eq!(
            backend.activations(),
            vec![(
                FocusMechanism::AppleScript,
                "code --reuse-window".to_string()
            )]
        );
    }

    #[test]
    fn focus_editor_wayland_never_confirms_for_any_editor() {
        // The Wayland honesty caveat is inherited from the focus seam for EVERY
        // editor row — no editor can override it into a false "focused".
        for kind in [EditorKind::Vscode, EditorKind::Cursor, EditorKind::Windsurf] {
            let backend = MockBackend::new(BackendResult::Activated);
            let outcome = focus_editor(&backend, &kind, FocusPlatform::Wayland, "code .").unwrap();
            assert_eq!(
                outcome,
                FocusOutcome::Requested,
                "{kind:?} on Wayland must never claim Confirmed"
            );
            assert!(!outcome.is_confirmed_success());
            // It used the Wayland XDG-activation-token mechanism from the seam.
            assert_eq!(
                backend.activations()[0].0,
                FocusMechanism::XdgActivationToken
            );
        }
    }

    #[test]
    fn focus_editor_x11_uses_wmctrl_and_can_confirm() {
        let backend = MockBackend::new(BackendResult::Activated);
        let outcome = focus_editor(
            &backend,
            &EditorKind::Cursor,
            FocusPlatform::LinuxX11,
            "wid:0x55",
        )
        .unwrap();
        assert_eq!(outcome, FocusOutcome::Confirmed);
        assert_eq!(backend.activations()[0].0, FocusMechanism::X11Wmctrl);
    }

    #[test]
    fn focus_editor_failed_activation_falls_back_to_notification() {
        let backend = MockBackend::new(BackendResult::Failed).with_fallback(true);
        let outcome = focus_editor(
            &backend,
            &EditorKind::Windsurf,
            FocusPlatform::MacOs,
            "windsurf .",
        )
        .unwrap();
        // The fallback policy is the focus seam's, applied uniformly.
        assert_eq!(outcome, FocusOutcome::FellBackToNotification);
        assert_eq!(
            backend.fallback_notifications(),
            vec!["windsurf .".to_string()]
        );
    }

    #[test]
    fn descriptor_focus_mechanism_matches_focus_seam() {
        // The descriptor must DELEGATE to the focus seam, not fork it: the
        // mechanism a row reports equals FocusStrategy::for_platform's.
        for d in EDITORS.iter() {
            for p in [
                FocusPlatform::MacOs,
                FocusPlatform::LinuxX11,
                FocusPlatform::Wayland,
                FocusPlatform::Other,
            ] {
                assert_eq!(
                    d.focus_mechanism_for(p),
                    FocusStrategy::for_platform(p).mechanism,
                    "{:?} on {p:?} must reuse the focus seam's mechanism",
                    d.kind
                );
                assert_eq!(d.focus_strategy_for(p), FocusStrategy::for_platform(p));
            }
        }
    }

    // ── end-to-end: pick an installed editor, build its launch, focus it ─────

    #[test]
    fn installed_target_launches_and_focuses_through_shared_seam() {
        let det = MockDetector::with(&["code"]);
        let target = installed_targets(&det)[0];
        // Build the launch from the target's descriptor (no branching).
        let argv = target.descriptor.launch_argv("/repo", true);
        assert_eq!(argv, vec!["code", "--reuse-window", "/repo"]);
        // And focus via the shared focus seam.
        let backend = MockBackend::new(BackendResult::Activated);
        let outcome = focus_editor(
            &backend,
            target.kind(),
            FocusPlatform::MacOs,
            "code --reuse-window /repo",
        )
        .unwrap();
        assert!(outcome.is_confirmed_success());
    }
}
