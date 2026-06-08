//! Fleet Reporter binary entry-point (S4: fake mode).
//!
//! Flags:
//!   `--fake [--ws <url>] [--delay-ms <ms>]` — scripted fake lifecycle
//!   `--fake --unix <path>` — unix fast path (`cfg(unix)` only)
//!
//! Without `--fake`, prints a placeholder explaining the real implementation
//! lands in REPCORE (S5).

use std::time::Duration;

use anyhow::Result;
use fleet_reporter::{FakeReporter, FakeReporterConfig, Transport};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();

    if !args.contains(&"--fake".to_string()) {
        println!("fleet-reporter: real implementation lands in REPCORE (S5).");
        println!("Run with --fake to drive the scripted test lifecycle.");
        return Ok(());
    }

    // Parse optional flags.
    let ws_url = flag_value(&args, "--ws").unwrap_or_else(|| "ws://127.0.0.1:7703".into());
    let delay_ms: u64 = flag_value(&args, "--delay-ms")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);

    let config = FakeReporterConfig {
        session_id: "sess-fake-0001".into(),
        run_id: "run-fake-0001".into(),
        step_delay: Duration::from_millis(delay_ms),
    };

    #[cfg(unix)]
    if let Some(path) = flag_value(&args, "--unix") {
        let transport = Transport::Unix(std::path::PathBuf::from(path));
        let reporter = FakeReporter::new(transport, config);
        return reporter.run().await;
    }

    let transport = Transport::WebSocket(ws_url);
    let reporter = FakeReporter::new(transport, config);
    reporter.run().await
}

/// Return the value of `--flag <value>` from an args slice.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}
