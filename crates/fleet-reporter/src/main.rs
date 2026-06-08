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
//! ## Fake mode (S4)
//!   `--fake [--ws <url>] [--delay-ms <ms>]` — scripted fake lifecycle
//!   `--fake --unix <path>` — unix fast path (`cfg(unix)` only)

use std::time::Duration;

use anyhow::Result;
use fleet_protocol::{
    AgentKind, AgentRun, Confidence, Extra, Location, LocationGlyph, LocationKind, Server,
    ServerKind, Session, State, SCHEMA_VERSION,
};
use fleet_reporter::{
    FakeReporter, FakeReporterConfig, Reporter, ReporterConfig, Transport, WsConnector,
};
use tracing::info;
use tracing_subscriber::EnvFilter;

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

    run_real(&args, ws_url).await
}

/// REPCORE real mode: register a session + working run, then run the framework.
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
        updated_at: fleet_reporter::fake::now_iso8601(),
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
        fleet_reporter::fake::now_iso8601(),
    )
}

/// S4 fake mode: drive the scripted lifecycle.
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
