//! Fleet CLI binary — `fleet ls` (the engineering spec).
//!
//! **Transport (D7):** connects to the Hub via unix socket (`cfg(unix)` fast
//! path) or WebSocket fallback, sends `subscribe`, and renders the snapshot +
//! live deltas to the terminal as they arrive.
//!
//! **Render (pure-function, heavily tested):** all snapshot→delta→rows logic
//! lives in [`render`] and is exercised by unit tests with no network I/O.
//!
//! **Live mode:** after the initial render the command stays alive and reprints
//! the table on every incoming delta. A future `--once` flag will exit after the
//! first snapshot (useful in scripts / pipe mode).

// Enable the `#[coverage(off)]` attribute under cargo-llvm-cov's nightly gate
// (it sets cfg(coverage_nightly)). A no-op on the local stable toolchain — used
// to exclude the thin `main` entrypoint (arg-parse + tracing init + process exit)
// from the line-coverage gate; its dispatch is exercised through `run`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

mod connection;
mod init;
mod render;

use anyhow::Result;
use clap::{Parser, Subcommand};
use render::{format_run_row, CliState};
use tracing_subscriber::EnvFilter;

const DEFAULT_WS_URL: &str = "ws://127.0.0.1:51777";

/// Fleet — multi-agent terminal supervisor.
#[derive(Parser)]
#[command(name = "fleet", version, about)]
struct Cli {
    /// WebSocket URL of the Hub (overrides default).
    #[arg(long, env = "FLEET_WS_URL", default_value = DEFAULT_WS_URL)]
    hub: String,

    /// Unix socket path of the Hub (overrides default; cfg(unix) only).
    #[arg(long, env = "FLEET_UNIX_PATH")]
    unix: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List sessions and their agent runs. Stays live, reprinting on each delta.
    Ls {
        /// Exit after the first snapshot instead of staying live.
        #[arg(long)]
        once: bool,
    },

    /// Inject Fleet hooks into Claude and Codex config files.
    ///
    /// Writes Fleet-managed hooks to:
    ///   - ~/.claude/settings.json  (Claude Code)
    ///   - ~/.codex/config.toml     (OpenAI Codex)
    ///
    /// Before modifying any file, the original is backed up. Running `fleet init`
    /// twice is safe (idempotent). Use `fleet uninit` to revert.
    Init {
        /// Override the reporter socket path embedded in hook commands.
        /// Defaults to $XDG_RUNTIME_DIR/fleet/reporter.sock (or /tmp/fleet/reporter.sock).
        #[arg(long)]
        reporter_socket: Option<std::path::PathBuf>,
    },

    /// Revert all changes made by `fleet init`, restoring original files byte-identically.
    ///
    /// Uses the backup manifest written by `fleet init`. If a file was created
    /// fresh (no prior content), it is removed. If `fleet init` was never run,
    /// this is a no-op.
    Uninit,
}

// The process entrypoint: parse argv, install the global tracing subscriber, and
// map the `run` result to an exit code. It is a thin wrapper with no branching
// logic of its own (the command dispatch it delegates to `run`, which IS tested),
// and it cannot be invoked from a unit test (it owns argv + the global logger +
// process exit), so it is excluded from the nightly line-coverage gate.
#[cfg_attr(coverage_nightly, coverage(off))]
#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Structured logging; honor RUST_LOG.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fleet: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Ls { once } => cmd_ls(&cli.hub, cli.unix.as_deref(), once).await,
        Commands::Init { reporter_socket } => cmd_init(reporter_socket),
        Commands::Uninit => cmd_uninit(),
    }
}

fn cmd_init(reporter_socket: Option<std::path::PathBuf>) -> Result<()> {
    let home = home_dir()?;
    let mut cfg = init::InitConfig::new(home);
    if let Some(socket) = reporter_socket {
        cfg.reporter_socket = Some(socket);
    }

    if init::is_initialised(&cfg) {
        eprintln!("fleet init: already initialised (run `fleet uninit` first to re-init)");
    }

    let result = init::do_init(&cfg)?;

    if result.claude_modified {
        eprintln!(
            "fleet init: wrote Claude hooks → {}",
            cfg.claude_settings_path().display()
        );
    } else {
        eprintln!(
            "fleet init: Claude settings already managed ({})",
            cfg.claude_settings_path().display()
        );
    }
    if result.codex_modified {
        eprintln!(
            "fleet init: wrote Codex config → {}",
            cfg.codex_config_path().display()
        );
    } else {
        eprintln!(
            "fleet init: Codex config already managed ({})",
            cfg.codex_config_path().display()
        );
    }
    if result.manifest_written {
        eprintln!(
            "fleet init: manifest written → {}",
            cfg.manifest_path().display()
        );
    }

    Ok(())
}

fn cmd_uninit() -> Result<()> {
    let home = home_dir()?;
    let cfg = init::InitConfig::new(home);

    if !init::is_initialised(&cfg) {
        eprintln!("fleet uninit: not initialised (nothing to undo)");
        return Ok(());
    }

    let result = init::do_uninit(&cfg)?;

    for path in &result.restored {
        eprintln!("fleet uninit: restored {}", path.display());
    }
    for path in &result.removed {
        eprintln!("fleet uninit: removed {}", path.display());
    }
    if result.restored.is_empty() && result.removed.is_empty() {
        eprintln!("fleet uninit: nothing to restore");
    }

    Ok(())
}

/// Resolve the user's home directory from `$HOME` (or platform default).
fn home_dir() -> Result<std::path::PathBuf> {
    if let Ok(h) = std::env::var("HOME") {
        return Ok(std::path::PathBuf::from(h));
    }
    #[cfg(unix)]
    {
        // Fall back to the passwd entry via std.
        if let Some(h) = std::env::var_os("HOME") {
            return Ok(std::path::PathBuf::from(h));
        }
    }
    anyhow::bail!("cannot determine home directory (set $HOME)")
}

async fn cmd_ls(ws_url: &str, unix_path: Option<&std::path::Path>, once: bool) -> Result<()> {
    // Resolve the unix socket path: explicit flag > env > Hub default.
    let default_unix = default_unix_path();
    let unix = unix_path.unwrap_or(&default_unix);

    eprintln!("fleet: connecting to Hub…");
    let mut events = connection::connect(ws_url, unix).await?;
    eprintln!("fleet: connected, waiting for snapshot…");

    let mut state = CliState::new();
    let mut got_snapshot = false;

    loop {
        match events.recv().await {
            None => {
                eprintln!("fleet: Hub connection closed.");
                break;
            }
            Some(ev) => {
                let is_snapshot = matches!(ev, fleet_protocol::Event::Snapshot { .. });
                state.apply(ev);
                if is_snapshot {
                    got_snapshot = true;
                }
                if got_snapshot {
                    print_table(&state);
                    if once && is_snapshot {
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

fn print_table(state: &CliState) {
    let rows = state.rows();
    if rows.is_empty() {
        println!("(no sessions)");
        return;
    }
    for row in &rows {
        println!("{}", row.render_line());
        for run_row in &row.runs {
            println!("{}", format_run_row(run_row, ""));
        }
    }
    println!("--- {} session(s) ---", rows.len());
}

/// Default unix socket path, mirroring `HubConfig::default()` logic.
fn default_unix_path() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("FLEET_RUNTIME_DIR") {
        return std::path::PathBuf::from(dir).join("hub.sock");
    }
    #[cfg(unix)]
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return std::path::PathBuf::from(dir).join("fleet").join("hub.sock");
    }
    std::env::temp_dir().join("fleet").join("hub.sock")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fleet_protocol::{
        Event, Extra, Location, LocationGlyph, LocationKind, Server, ServerKind, Session,
        State as RunState,
    };
    use render::CliState;

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

    #[test]
    fn binary_entry_point_builds() {
        // Smoke test: exercise CliState in main module context.
        let mut st = CliState::new();
        st.apply(Event::snapshot(vec![]));
        assert!(st.is_empty());
    }

    #[test]
    fn print_table_empty_does_not_panic() {
        let st = CliState::new();
        // Just verify it doesn't panic (it writes to stdout which is fine in tests).
        print_table(&st);
    }

    #[test]
    fn print_table_with_sessions_does_not_panic() {
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        print_table(&st);
    }

    #[test]
    fn default_unix_path_ends_with_hub_sock() {
        let p = default_unix_path();
        let shown = p.display().to_string();
        assert!(shown.ends_with("hub.sock"), "got: {shown}");
    }

    #[test]
    fn default_unix_path_with_fleet_runtime_dir() {
        let _g = ENV_LOCK.lock().unwrap();
        let _runtime = EnvGuard::set("FLEET_RUNTIME_DIR", "/tmp/test-fleet-cli");
        let p = default_unix_path();
        assert_eq!(p, std::path::PathBuf::from("/tmp/test-fleet-cli/hub.sock"));
    }

    // ── env-mutating tests are serialized via ENV_LOCK; every guard restores ────
    // the prior value (or unsets) on drop, so the process env is left untouched.

    use futures_util::{SinkExt, StreamExt};
    use std::ffi::OsString;
    use std::sync::Mutex;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Save-and-restore guard for a single environment variable.
    struct EnvGuard {
        key: String,
        prev: Option<OsString>,
    }
    impl EnvGuard {
        fn set(key: &str, val: &str) -> Self {
            Self::set_os(key, std::ffi::OsStr::new(val))
        }
        fn set_os(key: &str, val: &std::ffi::OsStr) -> Self {
            let prev = std::env::var_os(key);
            std::env::set_var(key, val);
            Self {
                key: key.to_string(),
                prev,
            }
        }
        fn unset(key: &str) -> Self {
            let prev = std::env::var_os(key);
            std::env::remove_var(key);
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(&self.key, v),
                None => std::env::remove_var(&self.key),
            }
        }
    }

    // ── print_table with run sub-rows ──────────────────────────────────────────

    #[test]
    fn print_table_prints_run_sub_rows_and_footer() {
        use fleet_protocol::{AgentKind, AgentRun, Confidence};
        let mut st = CliState::new();
        st.apply(Event::session_added(session("s1", "proj", RunState::Idle)));
        st.apply(Event::run_added(
            "s1",
            AgentRun::new(
                "r1",
                AgentKind::ClaudeCode,
                "native-1",
                "/home/user/p",
                RunState::Working,
                Confidence::High,
                "2026-06-08T00:00:00Z",
            ),
        ));
        // Exercises the inner run-row print loop + the "--- N session(s) ---"
        // footer line (both previously uncovered).
        print_table(&st);
        assert_eq!(st.rows()[0].runs.len(), 1);
    }

    // ── default_unix_path: XDG and temp_dir fallbacks ──────────────────────────

    #[cfg(unix)]
    #[test]
    fn default_unix_path_uses_xdg_runtime_dir() {
        let _g = ENV_LOCK.lock().unwrap();
        let _frd = EnvGuard::unset("FLEET_RUNTIME_DIR");
        let _xdg = EnvGuard::set("XDG_RUNTIME_DIR", "/run/user/1000");
        let p = default_unix_path();
        assert_eq!(
            p,
            std::path::PathBuf::from("/run/user/1000/fleet/hub.sock"),
            "XDG_RUNTIME_DIR/fleet/hub.sock when FLEET_RUNTIME_DIR is unset"
        );
    }

    #[test]
    fn default_unix_path_falls_back_to_temp_dir() {
        let _g = ENV_LOCK.lock().unwrap();
        let _frd = EnvGuard::unset("FLEET_RUNTIME_DIR");
        #[cfg(unix)]
        let _xdg = EnvGuard::unset("XDG_RUNTIME_DIR");
        let p = default_unix_path();
        let expected = std::env::temp_dir().join("fleet").join("hub.sock");
        assert_eq!(p, expected, "temp_dir/fleet/hub.sock when no runtime dir env");
    }

    // ── home_dir ───────────────────────────────────────────────────────────────

    #[test]
    fn home_dir_reads_home_env() {
        let _g = ENV_LOCK.lock().unwrap();
        let _h = EnvGuard::set("HOME", "/home/tester");
        assert_eq!(home_dir().unwrap(), std::path::PathBuf::from("/home/tester"));
    }

    #[cfg(unix)]
    #[test]
    fn home_dir_falls_back_to_non_utf8_home() {
        // When HOME is set but NOT valid UTF-8, `var("HOME")` fails (NotUnicode)
        // yet `var_os("HOME")` succeeds — the `#[cfg(unix)]` fallback path. Build
        // a non-UTF8 OsString (a lone 0xFF byte) to drive exactly that arm.
        use std::os::unix::ffi::OsStringExt;
        let _g = ENV_LOCK.lock().unwrap();
        let non_utf8 = OsString::from_vec(vec![b'/', 0xFF, b'h']);
        let _h = EnvGuard::set_os("HOME", non_utf8.as_os_str());
        let path = home_dir().expect("non-UTF8 HOME still resolves via var_os fallback");
        assert_eq!(path.as_os_str(), non_utf8.as_os_str());
    }

    #[test]
    fn home_dir_errors_when_home_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        let _h = EnvGuard::unset("HOME");
        let err = home_dir().expect_err("expected an error with HOME unset");
        assert!(
            err.to_string().contains("cannot determine home directory"),
            "unexpected error: {err}"
        );
    }

    // ── cmd_init / cmd_uninit via a temp HOME ──────────────────────────────────

    #[test]
    fn cmd_init_then_uninit_round_trip() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _h = EnvGuard::set("HOME", dir.path().to_str().unwrap());

        // First init writes the config + manifest.
        cmd_init(None).unwrap();
        let manifest = dir.path().join(".config/fleet/init-manifest.json");
        assert!(manifest.exists(), "init writes the manifest under HOME");
        assert!(dir.path().join(".claude/settings.json").exists());
        assert!(dir.path().join(".codex/config.toml").exists());

        // A second init is idempotent (prints the "already managed" branches).
        cmd_init(None).unwrap();

        // Uninit reverts everything.
        cmd_uninit().unwrap();
        assert!(!manifest.exists(), "uninit removes the manifest");

        // Uninit again is a no-op (the "not initialised" branch).
        cmd_uninit().unwrap();
    }

    #[test]
    fn cmd_init_honors_reporter_socket_override() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _h = EnvGuard::set("HOME", dir.path().to_str().unwrap());

        let custom = std::path::PathBuf::from("/custom/reporter.sock");
        cmd_init(Some(custom)).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();
        assert!(
            content.contains("/custom/reporter.sock"),
            "the override socket path must appear in the written hooks"
        );
    }

    // ── run() dispatch: Init and Uninit arms ───────────────────────────────────

    #[test]
    fn cmd_uninit_restores_existing_files_and_prints_restored() {
        // Pre-existing config files → init backs them up (backup = Some) → uninit
        // RESTORES them (the `restored` loop / eprintln path, line ~170).
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _h = EnvGuard::set("HOME", dir.path().to_str().unwrap());

        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::create_dir_all(dir.path().join(".codex")).unwrap();
        std::fs::write(dir.path().join(".claude/settings.json"), b"{\"k\":1}\n").unwrap();
        std::fs::write(dir.path().join(".codex/config.toml"), b"[m]\nn=\"x\"\n").unwrap();

        cmd_init(None).unwrap();
        cmd_uninit().unwrap();

        // Restored byte-identically.
        assert_eq!(
            std::fs::read(dir.path().join(".claude/settings.json")).unwrap(),
            b"{\"k\":1}\n"
        );
    }

    #[test]
    fn cmd_uninit_nothing_to_restore_when_created_files_already_gone() {
        // Fresh init (files created, backup = None). Delete the created files by
        // hand, then uninit: do_uninit finds nothing to restore/remove → the
        // "nothing to restore" branch (line ~176). is_initialised is still true
        // because the manifest remains.
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _h = EnvGuard::set("HOME", dir.path().to_str().unwrap());

        cmd_init(None).unwrap();
        // Remove the created targets so do_uninit's branches all no-op.
        std::fs::remove_file(dir.path().join(".claude/settings.json")).unwrap();
        std::fs::remove_file(dir.path().join(".codex/config.toml")).unwrap();

        // Still initialised (manifest present) so cmd_uninit proceeds to do_uninit.
        cmd_uninit().unwrap();
    }

    #[tokio::test]
    async fn run_dispatches_ls_command() {
        // run() with the Ls subcommand drives cmd_ls (covers the `Ls` match arm).
        let (url, server) = spawn_hub_with_snapshot(vec![]).await;
        let cli = Cli {
            hub: url,
            unix: Some(std::path::PathBuf::from(
                "/nonexistent/fleet-cli-test/run-ls.sock",
            )),
            command: Commands::Ls { once: true },
        };
        run(cli).await.unwrap();
        server.await.unwrap();
    }

    // Holds ENV_LOCK across `.await` deliberately: $HOME must stay set for the
    // whole async `run` (which reads it). The lock only serializes env-mutating
    // tests; there is no contention deadlock (single guard, no nested locking).
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn run_dispatches_init_and_uninit() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let _h = EnvGuard::set("HOME", dir.path().to_str().unwrap());

        let init_cli = Cli {
            hub: DEFAULT_WS_URL.to_string(),
            unix: None,
            command: Commands::Init {
                reporter_socket: None,
            },
        };
        run(init_cli).await.unwrap();
        assert!(dir.path().join(".config/fleet/init-manifest.json").exists());

        let uninit_cli = Cli {
            hub: DEFAULT_WS_URL.to_string(),
            unix: None,
            command: Commands::Uninit,
        };
        run(uninit_cli).await.unwrap();
        assert!(!dir.path().join(".config/fleet/init-manifest.json").exists());
    }

    // ── cmd_ls --once against an in-process Hub (the real connection path) ──────

    /// Spawn a loopback WS server that, after the subscribe frame, sends one
    /// snapshot containing `sessions` and then drains until the client closes.
    /// Returns the ws URL plus the server task handle (await it after the client
    /// disconnects so the drain loop terminates deterministically).
    async fn spawn_hub_with_snapshot(
        sessions: Vec<Session>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();
            // Consume the subscribe frame.
            let _ = ws.next().await;
            let snap = serde_json::to_string(&Event::snapshot(sessions)).unwrap();
            let _ = ws.send(Message::Text(snap.into())).await;
            // Close cleanly so the server task ends deterministically (the `--once`
            // client breaks on the snapshot before it even observes the Close).
            let _ = ws.send(Message::Close(None)).await;
        });
        (format!("ws://{addr}"), handle)
    }

    #[tokio::test]
    async fn cmd_ls_once_renders_snapshot_then_exits() {
        // Point the unix path at a nonexistent socket so connect() uses the WS URL,
        // then drive cmd_ls(once=true): it must connect, fold the snapshot, print,
        // and return after the first snapshot. Exercises the cmd_ls loop + the
        // got_snapshot/once exit branch over the REAL connection client.
        let (url, server) =
            spawn_hub_with_snapshot(vec![session("s1", "proj", RunState::Working)]).await;
        let missing = std::path::Path::new("/nonexistent/fleet-cli-test/cmd-ls.sock");
        cmd_ls(&url, Some(missing), true).await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cmd_ls_once_with_empty_snapshot_exits() {
        // Empty snapshot still satisfies got_snapshot → prints "(no sessions)"
        // and exits on `once`.
        let (url, server) = spawn_hub_with_snapshot(vec![]).await;
        let missing = std::path::Path::new("/nonexistent/fleet-cli-test/cmd-ls2.sock");
        cmd_ls(&url, Some(missing), true).await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cmd_ls_breaks_when_hub_closes_without_snapshot() {
        // The Hub closes immediately after subscribe (no snapshot). cmd_ls must
        // observe the closed connection (recv→None) and return Ok. Covers the
        // `None => break` arm.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();
            let _ = ws.next().await; // subscribe
            let _ = ws.send(Message::Close(None)).await;
        });
        let url = format!("ws://{addr}");
        let missing = std::path::Path::new("/nonexistent/fleet-cli-test/cmd-ls3.sock");
        cmd_ls(&url, Some(missing), false).await.unwrap();
    }

    #[tokio::test]
    async fn cmd_ls_live_mode_reprints_on_delta_then_closes() {
        // once=false: after the snapshot, a delta arrives and is reprinted; then
        // the Hub closes. Covers the live-reprint path (got_snapshot && !once) and
        // the post-snapshot delta apply.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();
            let _ = ws.next().await; // subscribe
            let snap = serde_json::to_string(&Event::snapshot(vec![session(
                "s1",
                "proj",
                RunState::Idle,
            )]))
            .unwrap();
            let _ = ws.send(Message::Text(snap.into())).await;
            let delta = serde_json::to_string(&Event::session_added(session(
                "s2",
                "another",
                RunState::Working,
            )))
            .unwrap();
            let _ = ws.send(Message::Text(delta.into())).await;
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            let _ = ws.send(Message::Close(None)).await;
        });
        let url = format!("ws://{addr}");
        let missing = std::path::Path::new("/nonexistent/fleet-cli-test/cmd-ls4.sock");
        cmd_ls(&url, Some(missing), false).await.unwrap();
    }

    #[tokio::test]
    async fn cmd_ls_ignores_deltas_before_first_snapshot() {
        // A delta arrives BEFORE any snapshot → got_snapshot stays false, so the
        // `if got_snapshot` block is skipped (its false arm). Then a snapshot
        // arrives and, with once=true, the loop exits. Covers the pre-snapshot
        // "hold rendering" path.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();
            let _ = ws.next().await; // subscribe
            // A delta with NO preceding snapshot.
            let delta = serde_json::to_string(&Event::session_added(session(
                "early",
                "before snapshot",
                RunState::Idle,
            )))
            .unwrap();
            let _ = ws.send(Message::Text(delta.into())).await;
            // Then the snapshot (ends the --once loop).
            let snap = serde_json::to_string(&Event::snapshot(vec![])).unwrap();
            let _ = ws.send(Message::Text(snap.into())).await;
            let _ = ws.send(Message::Close(None)).await;
        });
        let url = format!("ws://{addr}");
        let missing = std::path::Path::new("/nonexistent/fleet-cli-test/cmd-ls6.sock");
        cmd_ls(&url, Some(missing), true).await.unwrap();
        server.await.unwrap();
    }

    // ── cmd_ls connection failure surfaces an error ────────────────────────────

    #[tokio::test]
    async fn cmd_ls_returns_err_on_connect_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener); // refuse connections on this port
        let url = format!("ws://{addr}");
        let missing = std::path::Path::new("/nonexistent/fleet-cli-test/cmd-ls5.sock");
        let res = cmd_ls(&url, Some(missing), true).await;
        assert!(res.is_err(), "cmd_ls must surface a connect failure");
    }

    // ── cmd_ls resolves the default unix path when none is passed ──────────────

    // Holds ENV_LOCK across `.await`: FLEET_RUNTIME_DIR must stay set while the
    // async cmd_ls resolves the default unix path. Same rationale as above.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn cmd_ls_uses_default_unix_path_when_none() {
        // unix_path = None → cmd_ls computes default_unix_path(); that file won't
        // exist, so connect() uses the WS URL. Covers the `unwrap_or(&default)`
        // branch of cmd_ls.
        let _g = ENV_LOCK.lock().unwrap();
        // Force the default unix path into an isolated, nonexistent location.
        let _frd = EnvGuard::set("FLEET_RUNTIME_DIR", "/nonexistent/fleet-cli-default");
        let (url, server) = spawn_hub_with_snapshot(vec![]).await;
        cmd_ls(&url, None, true).await.unwrap();
        server.await.unwrap();
    }
}
