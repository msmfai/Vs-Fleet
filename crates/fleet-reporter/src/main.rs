//! Fleet Reporter binary entry-point.
//!
//! ## Real mode (default, REPCORE / S5)
//! Opens an outbound connection to the Hub, **registers** a session, **assigns a
//! fleet run-id**, and runs the reporter framework loop (heartbeats, buffering,
//! reconnect-with-backoff). Flags:
//!   `[--ws <url>]`        — Hub WebSocket URL (default `ws://127.0.0.1:51777`)
//!   `[--unix <path>]`     — Hub unix-socket fast path (`cfg(unix)` only)
//!   `[--session-id <id>]` — durable session id to register under
//!
//! ## Serve mode (the hook-receiver — makes Fleet actually run)
//!   `--serve [--ws <url>] [--unix <hub.sock>] [--socket <reporter.sock>]
//!            [--session-id <id>]`
//! Connects to the Hub (like real mode), then **binds the reporter socket** and
//! listens for Claude/Codex hook payloads, turning each into Hub deltas via the
//! detection adapters (see [`fleet_reporter::serve`]). The reporter socket
//! defaults to [`fleet_protocol::default_reporter_socket`] — the same path
//! `fleet init` writes hooks toward and the VS Code extension injects as
//! `FLEET_REPORTER_SOCKET`. The window session id defaults to the
//! `FLEET_SESSION_ID` the extension injects.
//!
//! ## Fake mode (S4)
//!   `--fake [--ws <url>] [--delay-ms <ms>]` — scripted fake lifecycle
//!   `--fake --unix <path>` — unix fast path (`cfg(unix)` only)

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

#[cfg(unix)]
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use fleet_protocol::{
    AgentKind, AgentRun, Confidence, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session, State, SCHEMA_VERSION,
};
#[cfg(unix)]
use fleet_reporter::Receiver;
use fleet_reporter::{
    FakeReporter, FakeReporterConfig, Reporter, ReporterConfig, Transport, WsConnector,
};
#[cfg(unix)]
use tokio::sync::Mutex;
use tracing::info;
use tracing_subscriber::EnvFilter;

// Thin binary entrypoint: arg routing + tracing init. Daemon dispatch only.
#[cfg_attr(coverage_nightly, coverage(off))]
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let ws_url = flag_value(&args, "--ws").unwrap_or_else(|| "ws://127.0.0.1:51777".into());

    if args.contains(&"--fake".to_string()) {
        return run_fake(&args, &ws_url).await;
    }

    if args.contains(&"--serve".to_string()) {
        return run_serve(&args, ws_url).await;
    }

    run_real(&args, ws_url).await
}

/// SERVE mode on non-unix platforms: the hook-receiver needs a unix domain
/// socket, which Windows does not have yet. Refuse loudly instead of binding
/// nothing — Fleet degrades to "no agent state" on these platforms.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(not(unix))]
async fn run_serve(_args: &[String], _ws_url: String) -> Result<()> {
    anyhow::bail!(
        "`fleet-reporter --serve` requires unix domain sockets and is not supported on this platform yet"
    )
}

/// SERVE mode: connect to the Hub, register the window session, then bind the
/// reporter socket and feed every hook payload through the detection adapters.
// Hub-connect + bind + accept-loop daemon; raced against Ctrl-C. Not unit-tested
// (binds a real socket and runs until interrupted).
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(unix)]
async fn run_serve(args: &[String], ws_url: String) -> Result<()> {
    // The window session id is injected by the VS Code extension as
    // FLEET_SESSION_ID; `--session-id` overrides; otherwise a local fallback.
    let session_id = flag_value(args, "--session-id")
        .or_else(|| std::env::var("FLEET_SESSION_ID").ok())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "sess-local-window".into());

    // The reporter socket to bind: `--socket`, else FLEET_REPORTER_SOCKET / the
    // canonical default — the SAME path `fleet init` + the extension target.
    let reporter_socket = flag_value(args, "--socket")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(fleet_protocol::default_reporter_socket);

    // Connector to the HUB (note: `--unix` here is the *Hub* socket, distinct
    // from the reporter socket we bind below).
    let connector: Box<dyn fleet_reporter::Connector> = {
        #[cfg(unix)]
        {
            if let Some(path) = flag_value(args, "--unix") {
                Box::new(fleet_reporter::UnixConnector::new(
                    std::path::PathBuf::from(path),
                ))
            } else {
                Box::new(WsConnector::new(ws_url.clone()))
            }
        }
        #[cfg(not(unix))]
        {
            Box::new(WsConnector::new(ws_url.clone()))
        }
    };

    let reporter = Reporter::new(ReporterConfig::new(&session_id), connector);
    let (reporter, handle, rx) = reporter.with_channel();
    let task = tokio::spawn(reporter.run(rx));

    // Register the window session so the Hub (and faces) show it immediately,
    // before any agent run arrives.
    handle.upsert_session(window_session(&session_id));
    info!(%session_id, socket = %reporter_socket.display(), "REPSERVE hook-receiver starting");

    let receiver = Arc::new(Mutex::new(Receiver::new()));

    // Race the accept loop against Ctrl-C so the binary shuts down cleanly.
    let serve = fleet_reporter::serve_unix(reporter_socket.clone(), receiver, handle.clone());
    tokio::select! {
        res = serve => {
            res?;
        }
        _ = tokio::signal::ctrl_c() => {
            info!("interrupt received; shutting down hook-receiver");
        }
    }

    // Best-effort: remove our socket file so we don't leave a stale path.
    std::fs::remove_file(&reporter_socket).ok();
    handle.shutdown();
    // The reporter task owns the Hub link; give it a moment to flush, ignoring a
    // join error on abrupt shutdown.
    let _ = task.await;
    Ok(())
}

/// The session shell for a VS Code editor window hosting agent terminals.
#[cfg(unix)]
fn window_session(id: &str) -> Session {
    let mut s = window_session_base(id);
    s.editor = Some(fleet_protocol::Editor {
        kind: Some(fleet_protocol::EditorKind::Vscode),
        focus_hint: Some(id.to_string()),
        extra: Extra::new(),
    });
    s
}

#[cfg(unix)]
fn window_session_base(id: &str) -> Session {
    Session {
        schema_version: SCHEMA_VERSION,
        session_id: id.into(),
        // `FLEET_SESSION_TITLE` overrides the displayed title (containerized
        // environments set it to their id so sessions are distinguishable); else
        // the working-directory basename.
        title: std::env::var("FLEET_SESSION_TITLE")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            })
            .unwrap_or_else(|| "fleet window".into()),
        location: Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        },
        editor: None,
        server: Server {
            kind: ServerKind::Local,
            version: None,
            extra: Extra::new(),
        },
        runs: vec![],
        rollup_state: State::Idle,
        rollup_urgency: None,
        muted: false,
        soloed: false,
        unread: false,
        tags: vec![],
        policy: None,
        updated_at: fleet_protocol::now_iso8601(),
        extra: Extra::new(),
    }
}

/// REPCORE real mode: register a session + working run, then run the framework.
// Spawns the reporter task and blocks on Ctrl-C; a daemon loop, not unit-tested.
#[cfg_attr(coverage_nightly, coverage(off))]
async fn run_real(args: &[String], ws_url: String) -> Result<()> {
    let session_id = flag_value(args, "--session-id").unwrap_or_else(|| "sess-local-0001".into());

    // Build the connector (unix fast path if requested, else WS).
    let connector: Box<dyn fleet_reporter::Connector> = {
        #[cfg(unix)]
        {
            if let Some(path) = flag_value(args, "--unix") {
                Box::new(fleet_reporter::UnixConnector::new(
                    std::path::PathBuf::from(path),
                ))
            } else {
                Box::new(WsConnector::new(ws_url.clone()))
            }
        }
        #[cfg(not(unix))]
        {
            Box::new(WsConnector::new(ws_url.clone()))
        }
    };

    let mut reporter = Reporter::new(ReporterConfig::new(&session_id), connector);
    // Assign a fleet run-id (D4 — reporter-assigned, no broker).
    let run_id = reporter.core_mut().assign_run_id();
    info!(%session_id, %run_id, "REPCORE reporter registering");

    let (reporter, handle, rx) = reporter.with_channel();
    let task = tokio::spawn(reporter.run(rx));

    handle.upsert_session(local_session(&session_id));
    handle.upsert_run(local_run(&run_id, State::Working));

    // Graceful shutdown on Ctrl-C (the agent observer would drive this in
    // production; here we keep the framework alive until interrupted).
    tokio::signal::ctrl_c().await.ok();
    info!("interrupt received; shutting down reporter");
    handle.confirm_exit(run_id, "reporter interrupted");
    handle.shutdown();
    task.await??;
    Ok(())
}

fn local_session(id: &str) -> Session {
    Session {
        schema_version: SCHEMA_VERSION,
        session_id: id.into(),
        title: "local reporter".into(),
        location: Location {
            kind: LocationKind::Local,
            label: "laptop".into(),
            glyph: LocationGlyph::Laptop,
            attach_hint: None,
            extra: Extra::new(),
        },
        editor: None,
        server: Server {
            kind: ServerKind::Local,
            version: None,
            extra: Extra::new(),
        },
        runs: vec![],
        rollup_state: State::Idle,
        rollup_urgency: None,
        muted: false,
        soloed: false,
        unread: false,
        tags: vec![],
        policy: None,
        updated_at: fleet_protocol::now_iso8601(),
        extra: Extra::new(),
    }
}

fn local_run(run_id: &str, state: State) -> AgentRun {
    AgentRun::new(
        run_id,
        AgentKind::Other,
        run_id,
        std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "/".into()),
        state,
        Confidence::Inferred,
        fleet_protocol::now_iso8601(),
    )
}

/// S4 fake mode: drive the scripted lifecycle.
// Opens a real transport and runs the scripted reporter to completion; the fake
// driver itself is exhaustively tested in `fake.rs`. Not unit-tested here.
#[cfg_attr(coverage_nightly, coverage(off))]
async fn run_fake(args: &[String], ws_url: &str) -> Result<()> {
    let delay_ms: u64 = flag_value(args, "--delay-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);

    let config = FakeReporterConfig {
        session_id: "sess-fake-0001".into(),
        run_id: "run-fake-0001".into(),
        step_delay: Duration::from_millis(delay_ms),
    };

    #[cfg(unix)]
    if let Some(path) = flag_value(args, "--unix") {
        let transport = Transport::Unix(std::path::PathBuf::from(path));
        let reporter = FakeReporter::new(transport, config);
        return reporter.run().await;
    }

    let transport = Transport::WebSocket(ws_url.to_string());
    let reporter = FakeReporter::new(transport, config);
    reporter.run().await
}

/// Return the value of `--flag <value>` from an args slice.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flag_value_finds_present_flag() {
        let args = argv(&["--ws", "ws://x", "--session-id", "sess-7"]);
        assert_eq!(flag_value(&args, "--ws"), Some("ws://x".into()));
        assert_eq!(flag_value(&args, "--session-id"), Some("sess-7".into()));
    }

    #[test]
    fn flag_value_missing_flag_is_none() {
        let args = argv(&["--fake"]);
        assert_eq!(flag_value(&args, "--ws"), None);
        // A flag with no following value is also None (no 2-window match).
        let trailing = argv(&["--ws"]);
        assert_eq!(flag_value(&trailing, "--ws"), None);
    }

    #[test]
    fn local_session_has_window_shape() {
        let s = local_session("sess-abc");
        assert_eq!(s.session_id, "sess-abc");
        assert_eq!(s.title, "local reporter");
        assert_eq!(s.location.kind, LocationKind::Local);
        assert!(s.editor.is_none());
        assert_eq!(s.rollup_state, State::Idle);
    }

    #[test]
    fn local_run_uses_run_id_and_state() {
        let r = local_run("run-xyz", State::Working);
        assert_eq!(r.run_id, "run-xyz");
        assert_eq!(r.agent_kind, AgentKind::Other);
        assert_eq!(r.state, State::Working);
        assert_eq!(r.confidence, Confidence::Inferred);
        // cwd is the process working dir (or "/" fallback) — always non-empty.
        assert!(!r.cwd.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn window_session_sets_vscode_editor() {
        // window_session wraps the base and attaches a VS Code editor focus hint.
        let s = window_session("sess-win");
        assert_eq!(s.session_id, "sess-win");
        let editor = s.editor.expect("window session must carry an editor");
        assert_eq!(editor.kind, Some(fleet_protocol::EditorKind::Vscode));
        assert_eq!(editor.focus_hint.as_deref(), Some("sess-win"));
    }

    // The window-session title resolution reads two env vars; serialize the
    // mutations in one test and restore unconditionally so other tests are
    // unaffected and every title branch is exercised deterministically.
    #[cfg(unix)]
    #[test]
    fn window_session_base_title_branches() {
        use std::env;

        let prev_title = env::var("FLEET_SESSION_TITLE").ok();

        // (1) Explicit FLEET_SESSION_TITLE wins.
        env::set_var("FLEET_SESSION_TITLE", "container-42");
        let s = window_session_base("sess-1");
        assert_eq!(s.title, "container-42");

        // (2) Empty title is ignored → falls back to the cwd basename, which is
        // always a non-empty string for a real working directory.
        env::set_var("FLEET_SESSION_TITLE", "");
        let s = window_session_base("sess-2");
        assert!(!s.title.is_empty(), "fallback title must be non-empty");
        assert_ne!(s.title, "");

        // (3) Unset → same cwd-basename fallback path.
        env::remove_var("FLEET_SESSION_TITLE");
        let s = window_session_base("sess-3");
        assert!(!s.title.is_empty());

        // Exercise both arms of the save/restore helper deterministically: a set
        // value is restored verbatim; an unset value is removed.
        restore_env("FLEET_SESSION_TITLE", Some("restored".into()));
        assert_eq!(env::var("FLEET_SESSION_TITLE").unwrap(), "restored");
        restore_env("FLEET_SESSION_TITLE", None);
        assert!(env::var("FLEET_SESSION_TITLE").is_err());

        // Restore the original environment unconditionally.
        restore_env("FLEET_SESSION_TITLE", prev_title);
    }

    #[cfg(unix)]
    fn restore_env(key: &str, prev: Option<String>) {
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}
