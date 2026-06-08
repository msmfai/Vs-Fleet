//! Fleet CLI binary — `fleet ls` (PLAN S3, CLI node).
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

mod connection;
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
}

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
    }
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
        assert!(
            p.to_string_lossy().ends_with("hub.sock"),
            "got: {}",
            p.display()
        );
    }

    #[test]
    fn default_unix_path_with_fleet_runtime_dir() {
        std::env::set_var("FLEET_RUNTIME_DIR", "/tmp/test-fleet-cli");
        let p = default_unix_path();
        std::env::remove_var("FLEET_RUNTIME_DIR");
        assert_eq!(p, std::path::PathBuf::from("/tmp/test-fleet-cli/hub.sock"));
    }
}
