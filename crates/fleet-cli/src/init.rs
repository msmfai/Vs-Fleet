//! `fleet init` / `fleet uninit` — idempotent, reversible config writers.
//!
//! # What this module does
//!
//! `fleet init` writes Fleet's hook configuration into two tool config files:
//!
//! 1. **`~/.claude/settings.json`** (Claude Code) — injects a `hooks` block with
//!    `Stop`, `UserPromptSubmit`, `PreToolUse`, `SessionStart`, `SessionEnd` hooks
//!    so that Claude reports its state to the Fleet reporter socket.
//!
//! 2. **`~/.codex/config.toml`** (OpenAI Codex) — enables `[features] hooks`
//!    and installs **proper Codex hooks** (the documented `[[hooks.<Event>]]`
//!    mechanism, NOT the legacy `notify` toggle Codex ignores in project config)
//!    for SessionStart/UserPromptSubmit/PreToolUse/PostToolUse/Stop/SessionEnd/
//!    **PermissionRequest**. The hooks are installed *composably* — appended
//!    alongside any existing hooks (the monads stack), each a non-blocking,
//!    env-routed relay (reads `$FLEET_REPORTER_SOCKET` at runtime so the hook
//!    text is constant → trusted once via Codex's `/hooks`, never bypassed). Also
//!    sets `[tui] notifications = true` (OSC9 corroboration). The user trusts the
//!    Fleet hook via `/hooks` once (Codex keys trust by hook hash).
//!
//! Before modifying any file the original bytes are saved as a `.fleet-backup`
//! file (e.g. `settings.json.fleet-backup`). A manifest is written to
//! `~/.config/fleet/init-manifest.json` listing every touched file and its
//! backup path so `fleet uninit` can restore byte-identically.
//!
//! # Invariants enforced
//!
//! - **Idempotent**: running `fleet init` twice produces the same result as
//!   running it once. Fleet-injected keys are never duplicated.
//! - **Never silently overwrites**: if a file is modified when a backup already
//!   exists (i.e. it was previously managed by Fleet), the existing backup is
//!   kept (i.e. the very-first pre-Fleet state is preserved). A warning is
//!   printed if re-init is run while already initialised.
//! - **Reversible**: `fleet uninit` restores each file to its backup copy and
//!   removes the backup. If no backup exists (the file did not exist before),
//!   the file is removed entirely. After uninit the tree is byte-identical to
//!   the state before `fleet init`.
//!
//! # PLAN references
//! - S14 (node INIT), PLAN §3 invariant 6 (reversible), §3 invariant 5
//!   (confidence honesty), D10 (hooks-first), D14 (stable VSCode APIs only).

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ── Manifest (the uninit undo log) ───────────────────────────────────────────

/// One entry in the init manifest: what file was touched and where its backup
/// lives (or `None` if the file did not exist before `fleet init`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// The file that was modified (absolute path).
    pub target: PathBuf,
    /// The pre-Fleet backup (absolute path). `None` if the file was created
    /// fresh by `fleet init` (i.e. it did not exist before).
    pub backup: Option<PathBuf>,
}

/// The uninit undo log written to `~/.config/fleet/init-manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct InitManifest {
    pub entries: Vec<ManifestEntry>,
}

impl InitManifest {
    fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(path).with_context(|| format!("read manifest {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parse manifest {}", path.display()))
    }

    fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create manifest dir {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(self).context("serialise manifest")?;
        fs::write(path, bytes).with_context(|| format!("write manifest {}", path.display()))
    }
}

// ── Public config ─────────────────────────────────────────────────────────────

/// Parameters for `fleet init`.
///
/// All paths are normally derived from `home_dir` (the user's home directory).
/// Tests pass a temporary directory to avoid touching the real home.
#[derive(Debug, Clone)]
pub struct InitConfig {
    /// Home directory root (normally `$HOME`).
    pub home_dir: PathBuf,
    /// Path to write the reporter socket (surfaced in hook commands so Claude /
    /// Codex can reach the reporter). Defaults to
    /// `$XDG_RUNTIME_DIR/fleet/reporter.sock` if unset; otherwise
    /// `<tmpdir>/fleet/reporter.sock`.
    pub reporter_socket: Option<PathBuf>,
}

impl InitConfig {
    pub fn new(home_dir: PathBuf) -> Self {
        Self {
            home_dir,
            reporter_socket: None,
        }
    }

    /// Path to `~/.claude/settings.json`.
    pub fn claude_settings_path(&self) -> PathBuf {
        self.home_dir.join(".claude").join("settings.json")
    }

    /// Path to `~/.codex/config.toml`.
    pub fn codex_config_path(&self) -> PathBuf {
        self.home_dir.join(".codex").join("config.toml")
    }

    /// Path to the manifest file.
    pub fn manifest_path(&self) -> PathBuf {
        self.home_dir
            .join(".config")
            .join("fleet")
            .join("init-manifest.json")
    }

    /// The path to pass to hook commands for the reporter socket.
    ///
    /// Defers to [`fleet_protocol::default_reporter_socket`] — the single source
    /// of truth shared with `fleet-reporter --serve` and the VS Code extension —
    /// unless an explicit `reporter_socket` override is set (tests / custom
    /// layouts). This guarantees the hooks we write target exactly the socket the
    /// reporter binds.
    pub fn effective_reporter_socket(&self) -> PathBuf {
        if let Some(p) = &self.reporter_socket {
            return p.clone();
        }
        fleet_protocol::default_reporter_socket()
    }
}

// ── Claude settings.json helpers ─────────────────────────────────────────────

/// The marker we embed in `settings.json` to detect Fleet-managed hooks.
const FLEET_MARKER: &str = "fleet-managed";

/// POSIX single-quote a string for safe interpolation into a `/bin/sh` command.
/// Wraps in single quotes and escapes any embedded single quote as `'\''`.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build the Fleet hooks block for `settings.json`.
///
/// Each hook calls `fleet-hook-relay` (a thin shell glue script, installed
/// by the shim in S10) with the hook type and the reporter socket path so
/// the reporter can receive state transitions.
///
/// For the config-only fallback (S14) — where the shim is NOT installed —
/// the hooks write directly to the reporter socket via `nc` or `socat` as
/// the best-available mechanism, with a comment noting the limitation.
fn build_claude_hooks(reporter_socket: &Path) -> serde_json::Value {
    // Single-quote the socket path for the shell so a path containing spaces or
    // metacharacters can neither break the hook command nor inject (the path
    // comes from $XDG_RUNTIME_DIR / the temp dir, outside Fleet's control).
    let socket_path = sh_quote(&reporter_socket.display().to_string());
    // Each hook sends a JSON payload to the reporter socket using a POSIX
    // shell one-liner. We use `printf … | nc -U <socket>` as the most
    // portable option (nc with -U is available on macOS + Linux via OpenBSD
    // netcat or ncat). The hook command receives the hook payload on stdin
    // and the hook type is embedded literally.
    //
    // The `|| true` ensures Claude never fails a hook relay error (observer,
    // not owner — we must not break Claude's own flow, PLAN §3 invariant 3).
    let make_hook = |hook_type: &str| -> serde_json::Value {
        serde_json::json!({
            "hooks": [{
                "type": "command",
                // Frame the hook payload as the reporter `--serve` receiver
                // expects: one line of `"<agent-tag> <compact-json>\n"`. We strip
                // any CR/LF from the payload (`tr -d '\r\n'`) so the whole JSON is
                // exactly one line, prepend the `claude` agent tag (the payload
                // shape alone can't disambiguate Claude vs Codex), and terminate
                // with a newline. `nc -U` delivers it to the reporter socket;
                // `|| true` keeps Claude's own flow alive on any relay error
                // (observer-not-owner, PLAN §3 invariant 3).
                "command": format!(
                    "printf 'claude %s\\n' \"$(cat | tr -d '\\r\\n')\" | nc -U {socket_path} 2>/dev/null || true",
                ),
                "description": format!("fleet: relay {} to reporter", hook_type),
                // The FLEET_MARKER tag lets `fleet init --check` / `fleet uninit`
                // identify Fleet-managed hooks without parsing the command text.
                "tags": [FLEET_MARKER]
            }]
        })
    };

    serde_json::json!({
        "Stop":              make_hook("Stop"),
        "UserPromptSubmit":  make_hook("UserPromptSubmit"),
        "PreToolUse":        make_hook("PreToolUse"),
        "SessionStart":      make_hook("SessionStart"),
        "SessionEnd":        make_hook("SessionEnd")
    })
}

/// Returns `true` if `settings.json` already contains Fleet-managed hooks.
fn claude_settings_has_fleet_hooks(value: &serde_json::Value) -> bool {
    let Some(hooks_obj) = value.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    for (_hook_type, hook_val) in hooks_obj {
        let Some(hooks_arr) = hook_val.get("hooks").and_then(|h| h.as_array()) else {
            continue;
        };
        for entry in hooks_arr {
            if let Some(tags) = entry.get("tags").and_then(|t| t.as_array()) {
                if tags.iter().any(|t| t.as_str() == Some(FLEET_MARKER)) {
                    return true;
                }
            }
        }
    }
    false
}

/// Inject Fleet hooks into a mutable `settings.json` value, merging with any
/// existing hooks (non-Fleet hooks are preserved). Returns `true` if a
/// modification was made.
fn inject_claude_hooks(value: &mut serde_json::Value, reporter_socket: &Path) -> bool {
    if claude_settings_has_fleet_hooks(value) {
        return false; // idempotent
    }
    let fleet_hooks = build_claude_hooks(reporter_socket);
    let fleet_obj = fleet_hooks.as_object().unwrap();

    let hooks_entry = value
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

    let hooks_obj = hooks_entry.as_object_mut().unwrap();
    for (hook_type, fleet_val) in fleet_obj {
        let existing = hooks_obj
            .entry(hook_type.clone())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

        // Merge: append Fleet's hooks array into the existing hooks array.
        let fleet_inner = fleet_val
            .get("hooks")
            .and_then(|h| h.as_array())
            .cloned()
            .unwrap_or_default();

        let existing_hooks = existing
            .as_object_mut()
            .unwrap()
            .entry("hooks")
            .or_insert_with(|| serde_json::Value::Array(vec![]));

        if let serde_json::Value::Array(arr) = existing_hooks {
            arr.extend(fleet_inner);
        }
    }
    true
}

/// Remove all Fleet-managed hook entries from a `settings.json` value.
/// Returns `true` if any entries were removed.
#[cfg(test)]
fn remove_claude_hooks(value: &mut serde_json::Value) -> bool {
    let Some(hooks_obj) = value
        .as_object_mut()
        .and_then(|o| o.get_mut("hooks"))
        .and_then(|h| h.as_object_mut())
    else {
        return false;
    };

    let mut modified = false;
    for (_hook_type, hook_val) in hooks_obj.iter_mut() {
        if let Some(hooks_arr) = hook_val.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            let before = hooks_arr.len();
            hooks_arr.retain(|entry| {
                !entry
                    .get("tags")
                    .and_then(|t| t.as_array())
                    .map(|tags| tags.iter().any(|t| t.as_str() == Some(FLEET_MARKER)))
                    .unwrap_or(false)
            });
            if hooks_arr.len() != before {
                modified = true;
            }
        }
    }
    modified
}

// ── Codex config.toml helpers ─────────────────────────────────────────────────
//
// Fleet writes **proper Codex hooks** (the documented `[[hooks.<Event>]]`
// mechanism) — not the legacy `notify` toggle (which Codex ignores in
// project-local config). The hooks are installed the *composable* way:
//
//  - **Stacks, never replaces.** Fleet *appends* one command-hook group to each
//    event's array; any existing hooks (the user's, or another wrapper's) are
//    preserved and still run — Codex executes every hook for an event.
//  - **Never denies.** The relay exits `0`, makes no permission decision, and is
//    a no-op when `$FLEET_REPORTER_SOCKET` is unset (a non-Fleet terminal). It
//    can't block a tool or short-circuit another wrapper's hook.
//  - **Stable trust.** The command reads the per-window reporter socket from the
//    `$FLEET_REPORTER_SOCKET` env var at runtime (the extension injects it per
//    window) — so the hook *text* is constant across windows and needs trusting
//    via Codex's `/hooks` **once** (Codex keys trust by hook hash). Fleet never
//    uses `--dangerously-bypass-hook-trust`.

/// Marker embedded (as a trailing `#` comment) in Fleet's Codex hook command, so
/// re-init is idempotent and uninstall can find Fleet's hook without a custom
/// TOML field (Codex's `--strict-config` would reject unknown fields).
const FLEET_CODEX_MARKER: &str = "fleet-managed-codex-hook";

/// The Codex hook events Fleet relays. `PermissionRequest` is Codex's
/// authoritative *waiting-on-the-user* signal (it fires reliably, unlike Claude's
/// native UI), so Codex `waiting` can be high-confidence.
const CODEX_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "SessionEnd",
    "PermissionRequest",
];

/// The env-routed, non-blocking relay command Fleet installs for every event.
/// Reads `$FLEET_REPORTER_SOCKET` at runtime (constant text → trust-stable),
/// tags the payload `codex` for the receiver, and `|| true`s so it never breaks
/// Codex's own flow.
fn codex_hook_relay_command() -> String {
    format!(
        "printf 'codex %s\\n' \"$(cat | tr -d '\\r\\n')\" | nc -U \"$FLEET_REPORTER_SOCKET\" 2>/dev/null || true # {FLEET_CODEX_MARKER}"
    )
}

/// One `[[hooks.<Event>]]` group carrying Fleet's single command hook.
fn fleet_codex_hook_group() -> toml::Value {
    let mut cmd = toml::Table::new();
    cmd.insert("type".into(), toml::Value::String("command".into()));
    cmd.insert(
        "command".into(),
        toml::Value::String(codex_hook_relay_command()),
    );
    let mut group = toml::Table::new();
    group.insert(
        "hooks".into(),
        toml::Value::Array(vec![toml::Value::Table(cmd)]),
    );
    toml::Value::Table(group)
}

/// Whether an event's hook-group array already contains Fleet's hook (by marker).
fn event_array_has_fleet_hook(arr: &[toml::Value]) -> bool {
    arr.iter().any(|group| {
        group
            .get("hooks")
            .and_then(|h| h.as_array())
            .map(|hooks| {
                hooks.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .map(|c| c.contains(FLEET_CODEX_MARKER))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    })
}

/// Returns `true` if the TOML value already has the full Fleet Codex config
/// (the `hooks` feature on, and a Fleet hook present for every relayed event).
/// (Used by tests; `inject_codex_config` checks idempotency per-event inline.)
#[cfg(test)]
fn codex_toml_has_fleet_config(table: &toml::Table) -> bool {
    let features_ok = table
        .get("features")
        .and_then(|f| f.as_table())
        .and_then(|f| f.get("hooks"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !features_ok {
        return false;
    }
    let Some(hooks) = table.get("hooks").and_then(|h| h.as_table()) else {
        return false;
    };
    CODEX_HOOK_EVENTS.iter().all(|ev| {
        hooks
            .get(*ev)
            .and_then(|a| a.as_array())
            .map(|arr| event_array_has_fleet_hook(arr))
            .unwrap_or(false)
    })
}

/// Inject Fleet's Codex config (the `hooks` feature + a composable, env-routed
/// relay hook per event), merging with any existing content. Idempotent;
/// preserves every non-Fleet hook. Returns `true` if a modification was made.
fn inject_codex_config(table: &mut toml::Table) -> bool {
    let mut modified = false;

    // [features] hooks = true — enable the hooks subsystem (D10).
    {
        let features = table
            .entry("features")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let toml::Value::Table(ft) = features {
            if ft.get("hooks").and_then(|v| v.as_bool()) != Some(true) {
                ft.insert("hooks".into(), toml::Value::Boolean(true));
                modified = true;
            }
        }
    }

    // [tui] notifications = true — OSC9 corroboration channel (PLAN §2).
    {
        let tui = table
            .entry("tui")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let toml::Value::Table(tt) = tui {
            if tt.get("notifications").and_then(|v| v.as_bool()) != Some(true) {
                tt.insert("notifications".into(), toml::Value::Boolean(true));
                modified = true;
            }
        }
    }

    // [[hooks.<Event>]] — APPEND Fleet's relay group to each event's array,
    // preserving any existing groups (the user's / another wrapper's). The
    // monads stack: Codex runs every hook for the event.
    {
        let hooks = table
            .entry("hooks")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let toml::Value::Table(hooks) = hooks {
            for ev in CODEX_HOOK_EVENTS {
                let arr_val = hooks
                    .entry((*ev).to_string())
                    .or_insert_with(|| toml::Value::Array(Vec::new()));
                if let toml::Value::Array(arr) = arr_val {
                    if !event_array_has_fleet_hook(arr) {
                        arr.push(fleet_codex_hook_group());
                        modified = true;
                    }
                }
            }
        }
    }

    modified
}

// ── Backup helpers ────────────────────────────────────────────────────────────

/// Compute the backup path for a target: `<path>.fleet-backup`.
fn backup_path(target: &Path) -> PathBuf {
    let mut p = target.to_path_buf();
    let mut name = p.file_name().unwrap_or_default().to_owned();
    name.push(".fleet-backup");
    p.set_file_name(name);
    p
}

/// Write a backup of `target` to `backup`. If the target does not exist, no
/// backup is written. Returns `Some(backup_path)` if a backup was written,
/// `None` if the file did not exist (i.e. it will be created fresh).
fn maybe_backup(target: &Path, backup: &Path) -> Result<Option<PathBuf>> {
    if !target.exists() {
        return Ok(None);
    }
    // Only write the backup if it does not already exist (preserving the
    // very first pre-Fleet state, "never silently overwrite" invariant).
    if !backup.exists() {
        let bytes =
            fs::read(target).with_context(|| format!("read {} for backup", target.display()))?;
        if let Some(parent) = backup.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create backup dir {}", parent.display()))?;
        }
        fs::write(backup, bytes).with_context(|| format!("write backup {}", backup.display()))?;
    }
    Ok(Some(backup.to_path_buf()))
}

// ── Core init / uninit logic ──────────────────────────────────────────────────

/// Outcome of a single `fleet init` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitResult {
    /// Whether Claude `settings.json` was modified (false if already managed).
    pub claude_modified: bool,
    /// Whether Codex `config.toml` was modified (false if already managed).
    pub codex_modified: bool,
    /// Whether the manifest was created (false if already existed / no changes).
    pub manifest_written: bool,
}

/// Outcome of a single `fleet uninit` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninitResult {
    /// Files that were restored from backup.
    pub restored: Vec<PathBuf>,
    /// Files that were removed (they did not exist before `fleet init`).
    pub removed: Vec<PathBuf>,
}

/// Run `fleet init`: inject Fleet hooks and back up originals.
///
/// This function is idempotent: calling it twice is safe and produces the same
/// end state. The second call will report `claude_modified = false` and
/// `codex_modified = false`.
pub fn do_init(cfg: &InitConfig) -> Result<InitResult> {
    let reporter_socket = cfg.effective_reporter_socket();
    let mut manifest = InitManifest::load(&cfg.manifest_path())?;
    let mut claude_modified = false;
    let mut codex_modified = false;

    // ── 1. Claude settings.json ────────────────────────────────────────────────
    {
        let path = cfg.claude_settings_path();
        let backup = backup_path(&path);

        // Ensure parent directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create {} dir", parent.display()))?;
        }

        // Read existing content (or start with an empty object).
        let existing_bytes = if path.exists() {
            fs::read(&path).with_context(|| format!("read {}", path.display()))?
        } else {
            b"{}".to_vec()
        };

        let mut value: serde_json::Value = serde_json::from_slice(&existing_bytes)
            .with_context(|| format!("parse {} as JSON", path.display()))?;

        if inject_claude_hooks(&mut value, &reporter_socket) {
            // Write backup before modifying the file.
            let backup_opt = maybe_backup(&path, &backup)?;

            let new_bytes = serde_json::to_vec_pretty(&value).context("serialise settings.json")?;
            fs::write(&path, new_bytes).with_context(|| format!("write {}", path.display()))?;

            // Record in manifest only if not already there.
            if !manifest.entries.iter().any(|e| e.target == path) {
                manifest.entries.push(ManifestEntry {
                    target: path,
                    backup: backup_opt,
                });
            }
            claude_modified = true;
        }
    }

    // ── 2. Codex config.toml ───────────────────────────────────────────────────
    {
        let path = cfg.codex_config_path();
        let backup = backup_path(&path);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create {} dir", parent.display()))?;
        }

        // Read existing content (or start with an empty table).
        let existing_str = if path.exists() {
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?
        } else {
            String::new()
        };

        let mut table: toml::Table = existing_str
            .parse::<toml::Table>()
            .with_context(|| format!("parse {} as TOML", path.display()))?;

        if inject_codex_config(&mut table) {
            let backup_opt = maybe_backup(&path, &backup)?;

            let new_str = toml::to_string_pretty(&table).context("serialise config.toml")?;
            fs::write(&path, new_str).with_context(|| format!("write {}", path.display()))?;

            if !manifest.entries.iter().any(|e| e.target == path) {
                manifest.entries.push(ManifestEntry {
                    target: path,
                    backup: backup_opt,
                });
            }
            codex_modified = true;
        }
    }

    // ── 3. Write manifest if anything changed ─────────────────────────────────
    let manifest_written = claude_modified || codex_modified;
    if manifest_written {
        manifest.save(&cfg.manifest_path())?;
    }

    Ok(InitResult {
        claude_modified,
        codex_modified,
        manifest_written,
    })
}

/// Run `fleet uninit`: restore all files tracked in the manifest to their
/// pre-Fleet state, byte-identically.
pub fn do_uninit(cfg: &InitConfig) -> Result<UninitResult> {
    let manifest_path = cfg.manifest_path();
    let manifest = InitManifest::load(&manifest_path)?;

    let mut restored = Vec::new();
    let mut removed = Vec::new();

    for entry in &manifest.entries {
        match &entry.backup {
            Some(backup) => {
                // Restore from backup.
                if backup.exists() {
                    let bytes = fs::read(backup)
                        .with_context(|| format!("read backup {}", backup.display()))?;
                    fs::write(&entry.target, bytes)
                        .with_context(|| format!("restore {}", entry.target.display()))?;
                    fs::remove_file(backup)
                        .with_context(|| format!("remove backup {}", backup.display()))?;
                    restored.push(entry.target.clone());
                } else {
                    // Backup was lost; remove the file rather than leave stale Fleet config.
                    if entry.target.exists() {
                        fs::remove_file(&entry.target)
                            .with_context(|| format!("remove {}", entry.target.display()))?;
                        removed.push(entry.target.clone());
                    }
                }
            }
            None => {
                // File was created by fleet init (did not exist before).
                if entry.target.exists() {
                    fs::remove_file(&entry.target)
                        .with_context(|| format!("remove {}", entry.target.display()))?;
                    removed.push(entry.target.clone());
                }
            }
        }
    }

    // Remove the manifest itself.
    if manifest_path.exists() {
        fs::remove_file(&manifest_path)
            .with_context(|| format!("remove manifest {}", manifest_path.display()))?;
    }

    Ok(UninitResult { restored, removed })
}

/// Check whether `fleet init` has been run (manifest exists and is non-empty).
pub fn is_initialised(cfg: &InitConfig) -> bool {
    let manifest_path = cfg.manifest_path();
    if !manifest_path.exists() {
        return false;
    }
    InitManifest::load(&manifest_path)
        .map(|m| !m.entries.is_empty())
        .unwrap_or(false)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn tmp_cfg(dir: &TempDir) -> InitConfig {
        InitConfig::new(dir.path().to_path_buf())
    }

    // ── basic init ────────────────────────────────────────────────────────────

    #[test]
    fn init_creates_files_from_scratch() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);

        let result = do_init(&cfg).unwrap();
        assert!(result.claude_modified, "claude should be modified");
        assert!(result.codex_modified, "codex should be modified");
        assert!(result.manifest_written, "manifest should be written");

        // Both files must exist after init.
        assert!(cfg.claude_settings_path().exists());
        assert!(cfg.codex_config_path().exists());
        assert!(cfg.manifest_path().exists());
    }

    #[test]
    fn init_produces_valid_claude_settings_json() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);
        do_init(&cfg).unwrap();

        let bytes = fs::read(cfg.claude_settings_path()).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        // Must have a `hooks` key.
        assert!(value.get("hooks").is_some(), "hooks key required");
        // Must contain Fleet-managed entries.
        assert!(
            claude_settings_has_fleet_hooks(&value),
            "Fleet hooks must be present"
        );
    }

    #[test]
    fn init_produces_valid_codex_config_toml() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);
        do_init(&cfg).unwrap();

        let content = fs::read_to_string(cfg.codex_config_path()).unwrap();
        let table: toml::Table = content.parse().unwrap();

        assert!(
            table
                .get("features")
                .and_then(|f| f.as_table())
                .and_then(|f| f.get("hooks"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            "[features] hooks = true required"
        );
        assert!(
            table
                .get("tui")
                .and_then(|t| t.as_table())
                .and_then(|t| t.get("notifications"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            "[tui] notifications = true required"
        );
    }

    // ── idempotency ───────────────────────────────────────────────────────────

    #[test]
    fn init_is_idempotent_second_call_makes_no_changes() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);

        do_init(&cfg).unwrap();
        let claude_after_first = fs::read(cfg.claude_settings_path()).unwrap();
        let codex_after_first = fs::read(cfg.codex_config_path()).unwrap();

        let second = do_init(&cfg).unwrap();
        assert!(
            !second.claude_modified,
            "second init must not re-modify claude"
        );
        assert!(
            !second.codex_modified,
            "second init must not re-modify codex"
        );

        // File contents must be identical to the first run.
        assert_eq!(
            fs::read(cfg.claude_settings_path()).unwrap(),
            claude_after_first,
            "claude settings must be unchanged after second init"
        );
        assert_eq!(
            fs::read(cfg.codex_config_path()).unwrap(),
            codex_after_first,
            "codex config must be unchanged after second init"
        );
    }

    #[test]
    fn init_does_not_duplicate_hooks_in_settings_json() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);
        do_init(&cfg).unwrap();
        do_init(&cfg).unwrap();
        do_init(&cfg).unwrap();

        let bytes = fs::read(cfg.claude_settings_path()).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        // Each hook type should have exactly one Fleet-managed entry.
        for hook_type in &[
            "Stop",
            "UserPromptSubmit",
            "PreToolUse",
            "SessionStart",
            "SessionEnd",
        ] {
            let empty = vec![];
            let hooks_arr = value
                .get("hooks")
                .and_then(|h| h.get(hook_type))
                .and_then(|hv| hv.get("hooks"))
                .and_then(|h| h.as_array())
                .unwrap_or(&empty);
            let fleet_count = hooks_arr
                .iter()
                .filter(|e| {
                    e.get("tags")
                        .and_then(|t| t.as_array())
                        .map(|tags| tags.iter().any(|t| t.as_str() == Some(FLEET_MARKER)))
                        .unwrap_or(false)
                })
                .count();
            assert_eq!(
                fleet_count, 1,
                "hook {hook_type} must have exactly 1 Fleet entry, got {fleet_count}"
            );
        }
    }

    // ── uninit ────────────────────────────────────────────────────────────────

    #[test]
    fn uninit_after_fresh_init_removes_created_files() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);

        // No prior files — init creates them, uninit removes them.
        do_init(&cfg).unwrap();
        assert!(cfg.claude_settings_path().exists());
        assert!(cfg.codex_config_path().exists());

        let result = do_uninit(&cfg).unwrap();
        assert!(
            result.removed.len() >= 2,
            "both created files must be removed; got {:?}",
            result.removed
        );
        assert!(
            !cfg.claude_settings_path().exists(),
            "settings.json must be gone after uninit"
        );
        assert!(
            !cfg.codex_config_path().exists(),
            "config.toml must be gone after uninit"
        );
        assert!(
            !cfg.manifest_path().exists(),
            "manifest must be gone after uninit"
        );
    }

    #[test]
    fn init_uninit_round_trip_byte_identical_for_existing_files() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);

        // Pre-populate both files with known content.
        let claude_original = b"{\"someKey\": \"someValue\"}\n";
        let codex_original = b"[someSection]\nfoo = \"bar\"\n";

        let claude_path = cfg.claude_settings_path();
        let codex_path = cfg.codex_config_path();

        fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
        fs::write(&claude_path, claude_original).unwrap();
        fs::write(&codex_path, codex_original).unwrap();

        // Save snapshots.
        let claude_before = fs::read(&claude_path).unwrap();
        let codex_before = fs::read(&codex_path).unwrap();

        // Init modifies the files.
        do_init(&cfg).unwrap();
        // Uninit must restore.
        do_uninit(&cfg).unwrap();

        // Files must be byte-identical to the originals.
        assert_eq!(
            fs::read(&claude_path).unwrap(),
            claude_before,
            "claude settings.json must be byte-identical after uninit"
        );
        assert_eq!(
            fs::read(&codex_path).unwrap(),
            codex_before,
            "codex config.toml must be byte-identical after uninit"
        );
    }

    #[test]
    fn uninit_without_prior_init_is_a_noop() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);

        // No manifest — uninit must not fail.
        let result = do_uninit(&cfg).unwrap();
        assert!(result.restored.is_empty());
        assert!(result.removed.is_empty());
    }

    // ── backup preservation ───────────────────────────────────────────────────

    #[test]
    fn second_init_does_not_overwrite_existing_backup() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);

        let claude_path = cfg.claude_settings_path();
        fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        let original = b"{\"original\": true}\n";
        fs::write(&claude_path, original).unwrap();

        // First init: creates backup.
        do_init(&cfg).unwrap();
        let backup = backup_path(&claude_path);
        let backup_bytes_after_first = fs::read(&backup).unwrap();

        // Manually modify the target (simulating an external change).
        fs::write(&claude_path, b"{\"modified\": true}\n").unwrap();

        // Second init is idempotent (Fleet markers already present) — backup unchanged.
        do_init(&cfg).unwrap();
        let backup_bytes_after_second = fs::read(&backup).unwrap();

        assert_eq!(
            backup_bytes_after_first, backup_bytes_after_second,
            "existing backup must not be overwritten on second init"
        );
        assert_eq!(
            backup_bytes_after_first, original,
            "backup must contain the very-first original"
        );
    }

    #[test]
    fn backup_restores_to_original_content_after_uninit() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);

        let codex_path = cfg.codex_config_path();
        fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
        let original = b"[model]\nname = \"o3\"\n";
        fs::write(&codex_path, original).unwrap();

        do_init(&cfg).unwrap();
        // Content must be different after init.
        let after_init = fs::read(&codex_path).unwrap();
        assert_ne!(after_init, original.to_vec());

        do_uninit(&cfg).unwrap();
        assert_eq!(fs::read(&codex_path).unwrap(), original.to_vec());
    }

    // ── is_initialised ────────────────────────────────────────────────────────

    #[test]
    fn is_initialised_false_before_init() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);
        assert!(!is_initialised(&cfg));
    }

    #[test]
    fn is_initialised_true_after_init() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);
        do_init(&cfg).unwrap();
        assert!(is_initialised(&cfg));
    }

    #[test]
    fn is_initialised_false_after_uninit() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);
        do_init(&cfg).unwrap();
        do_uninit(&cfg).unwrap();
        assert!(!is_initialised(&cfg));
    }

    // ── inject helpers (pure unit tests, no I/O) ─────────────────────────────

    #[test]
    fn inject_claude_hooks_adds_all_required_hook_types() {
        let mut value = serde_json::json!({});
        let socket = PathBuf::from("/tmp/fleet/reporter.sock");
        inject_claude_hooks(&mut value, &socket);

        for hook_type in &[
            "Stop",
            "UserPromptSubmit",
            "PreToolUse",
            "SessionStart",
            "SessionEnd",
        ] {
            assert!(
                value.get("hooks").and_then(|h| h.get(hook_type)).is_some(),
                "hook type {hook_type} must be present"
            );
        }
    }

    #[test]
    fn inject_claude_hooks_preserves_existing_non_fleet_hooks() {
        let mut value = serde_json::json!({
            "hooks": {
                "Stop": {
                    "hooks": [{"type": "command", "command": "user-script.sh"}]
                }
            }
        });
        let socket = PathBuf::from("/tmp/fleet/reporter.sock");
        inject_claude_hooks(&mut value, &socket);

        let stop_hooks = value
            .get("hooks")
            .and_then(|h| h.get("Stop"))
            .and_then(|hv| hv.get("hooks"))
            .and_then(|h| h.as_array())
            .unwrap();

        // Must have both the original hook AND the Fleet hook.
        assert!(stop_hooks.len() >= 2, "original hook must be preserved");
        let has_user = stop_hooks
            .iter()
            .any(|e| e.get("command").and_then(|c| c.as_str()) == Some("user-script.sh"));
        assert!(has_user, "user's original hook must still be present");
        let has_fleet = stop_hooks.iter().any(|e| {
            e.get("tags")
                .and_then(|t| t.as_array())
                .map(|tags| tags.iter().any(|t| t.as_str() == Some(FLEET_MARKER)))
                .unwrap_or(false)
        });
        assert!(has_fleet, "Fleet hook must be added");
    }

    #[test]
    fn inject_claude_hooks_is_idempotent() {
        let mut value = serde_json::json!({});
        let socket = PathBuf::from("/tmp/fleet/reporter.sock");

        let modified_first = inject_claude_hooks(&mut value, &socket);
        let modified_second = inject_claude_hooks(&mut value, &socket);

        assert!(modified_first, "first inject should modify");
        assert!(!modified_second, "second inject should be a no-op");
    }

    #[test]
    fn remove_claude_hooks_removes_only_fleet_entries() {
        let mut value = serde_json::json!({
            "hooks": {
                "Stop": {
                    "hooks": [
                        {"type": "command", "command": "user-script.sh"},
                        {"type": "command", "command": "fleet-relay.sh", "tags": [FLEET_MARKER]}
                    ]
                }
            }
        });

        let modified = remove_claude_hooks(&mut value);
        assert!(modified);

        let stop_hooks = value
            .get("hooks")
            .and_then(|h| h.get("Stop"))
            .and_then(|hv| hv.get("hooks"))
            .and_then(|h| h.as_array())
            .unwrap();

        assert_eq!(stop_hooks.len(), 1, "only user hook should remain");
        assert_eq!(
            stop_hooks[0].get("command").and_then(|c| c.as_str()),
            Some("user-script.sh")
        );
    }

    #[test]
    fn inject_codex_config_adds_required_keys() {
        let mut table = toml::Table::new();
        let modified = inject_codex_config(&mut table);
        assert!(modified);
        assert!(codex_toml_has_fleet_config(&table));
    }

    #[test]
    fn inject_codex_config_is_idempotent() {
        let mut table = toml::Table::new();
        let first = inject_codex_config(&mut table);
        let second = inject_codex_config(&mut table);
        assert!(first);
        assert!(!second);
    }

    #[test]
    fn inject_codex_config_preserves_other_toml_keys() {
        let toml_str = "[model]\nname = \"o3\"\n[features]\nsome_other = true\n";
        let mut table: toml::Table = toml_str.parse().unwrap();
        inject_codex_config(&mut table);

        // Original keys must be preserved.
        assert_eq!(
            table
                .get("model")
                .and_then(|m| m.as_table())
                .and_then(|m| m.get("name"))
                .and_then(|v| v.as_str()),
            Some("o3")
        );
        assert!(
            table
                .get("features")
                .and_then(|f| f.as_table())
                .and_then(|f| f.get("some_other"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            "pre-existing [features] key must be preserved"
        );
    }

    #[test]
    fn backup_path_is_correct() {
        let path = PathBuf::from("/home/user/.claude/settings.json");
        let bp = backup_path(&path);
        assert_eq!(
            bp,
            PathBuf::from("/home/user/.claude/settings.json.fleet-backup")
        );
    }

    #[test]
    fn manifest_round_trips() {
        let dir = TempDir::new().unwrap();
        let manifest_path = dir.path().join("manifest.json");

        let original = InitManifest {
            entries: vec![
                ManifestEntry {
                    target: PathBuf::from("/home/user/.claude/settings.json"),
                    backup: Some(PathBuf::from(
                        "/home/user/.claude/settings.json.fleet-backup",
                    )),
                },
                ManifestEntry {
                    target: PathBuf::from("/home/user/.codex/config.toml"),
                    backup: None,
                },
            ],
        };
        original.save(&manifest_path).unwrap();
        let loaded = InitManifest::load(&manifest_path).unwrap();
        assert_eq!(original, loaded);
    }

    // ── full round-trip: init→uninit with existing + non-existing files ────────

    #[test]
    fn full_round_trip_both_files_exist_before_init() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);

        let claude_original = b"{\"theme\": \"dark\"}\n";
        let codex_original = b"[model]\nname = \"o4-mini\"\n";

        let claude_path = cfg.claude_settings_path();
        let codex_path = cfg.codex_config_path();
        fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
        fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
        fs::write(&claude_path, claude_original).unwrap();
        fs::write(&codex_path, codex_original).unwrap();

        // Init modifies both.
        let init_result = do_init(&cfg).unwrap();
        assert!(init_result.claude_modified);
        assert!(init_result.codex_modified);

        // Files now differ from originals.
        assert_ne!(fs::read(&claude_path).unwrap(), claude_original.to_vec());
        assert_ne!(fs::read(&codex_path).unwrap(), codex_original.to_vec());

        // Uninit restores both.
        let uninit_result = do_uninit(&cfg).unwrap();
        assert_eq!(uninit_result.restored.len(), 2);
        assert!(uninit_result.removed.is_empty());

        assert_eq!(fs::read(&claude_path).unwrap(), claude_original.to_vec());
        assert_eq!(fs::read(&codex_path).unwrap(), codex_original.to_vec());

        // is_initialised must be false after uninit.
        assert!(!is_initialised(&cfg));
    }

    #[test]
    fn full_round_trip_neither_file_exists_before_init() {
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);

        let claude_path = cfg.claude_settings_path();
        let codex_path = cfg.codex_config_path();

        assert!(!claude_path.exists());
        assert!(!codex_path.exists());

        do_init(&cfg).unwrap();
        assert!(claude_path.exists());
        assert!(codex_path.exists());

        let uninit_result = do_uninit(&cfg).unwrap();
        // Both files were created from scratch — they should be removed.
        assert!(uninit_result.removed.len() >= 2);
        assert!(uninit_result.restored.is_empty());

        assert!(
            !claude_path.exists(),
            "created file must be removed after uninit"
        );
        assert!(
            !codex_path.exists(),
            "created file must be removed after uninit"
        );
    }

    #[test]
    fn init_is_idempotent_for_codex_and_does_not_duplicate_hooks() {
        // Running init twice produces no second modification and exactly one
        // Fleet hook per event (no duplicates).
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);

        let first = do_init(&cfg).unwrap();
        assert!(first.codex_modified, "first init installs the codex hooks");
        let second = do_init(&cfg).unwrap();
        assert!(
            !second.codex_modified,
            "second init is a no-op (idempotent)"
        );

        let content = fs::read_to_string(cfg.codex_config_path()).unwrap();
        // Exactly one Fleet relay per event — count the marker occurrences.
        let marker_count = content.matches(FLEET_CODEX_MARKER).count();
        assert_eq!(
            marker_count,
            CODEX_HOOK_EVENTS.len(),
            "one Fleet hook per relayed event, no duplicates"
        );
    }

    #[test]
    fn codex_init_preserves_an_existing_non_fleet_hook() {
        // The monads stack: a user's own Codex hook must survive init, alongside
        // Fleet's — Fleet appends, never replaces.
        let dir = TempDir::new().unwrap();
        let cfg = tmp_cfg(&dir);
        let codex_path = cfg.codex_config_path();
        fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
        let existing = concat!(
            "[[hooks.PreToolUse]]\n",
            "[[hooks.PreToolUse.hooks]]\n",
            "type = \"command\"\n",
            "command = \"my-own-policy.sh\"\n",
        );
        fs::write(&codex_path, existing).unwrap();

        do_init(&cfg).unwrap();
        let content = fs::read_to_string(&codex_path).unwrap();
        assert!(
            content.contains("my-own-policy.sh"),
            "the user's existing hook must be preserved"
        );
        assert!(
            content.contains(FLEET_CODEX_MARKER),
            "Fleet's hook is added alongside it"
        );
        // PreToolUse now has BOTH groups: the user's and Fleet's.
        let table: toml::Table = content.parse().unwrap();
        let arr = table["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2, "user group + fleet group coexist");
    }

    #[test]
    fn codex_hook_relay_is_env_routed_and_non_blocking() {
        // Trust-stable (constant text via $FLEET_REPORTER_SOCKET) + never denies
        // (exits 0, no permission decision, no-op when the var is unset).
        let cmd = codex_hook_relay_command();
        assert!(
            cmd.contains("\"$FLEET_REPORTER_SOCKET\""),
            "env-routed, not a baked path"
        );
        assert!(
            cmd.starts_with("printf 'codex %s"),
            "tags the payload `codex`"
        );
        assert!(
            cmd.contains("|| true"),
            "non-blocking: never breaks codex's flow"
        );
        assert!(
            cmd.contains(FLEET_CODEX_MARKER),
            "carries the idempotency marker"
        );
        // No permission/deny decision is emitted (pure side-effect relay).
        assert!(!cmd.contains("\"decision\""));
    }

    #[test]
    fn hook_command_contains_socket_path() {
        let dir = TempDir::new().unwrap();
        let mut cfg = tmp_cfg(&dir);
        cfg.reporter_socket = Some(PathBuf::from("/custom/path/reporter.sock"));

        do_init(&cfg).unwrap();

        let bytes = fs::read(cfg.claude_settings_path()).unwrap();
        let content = String::from_utf8(bytes).unwrap();
        assert!(
            content.contains("/custom/path/reporter.sock"),
            "hook command must embed the reporter socket path"
        );
    }
}
