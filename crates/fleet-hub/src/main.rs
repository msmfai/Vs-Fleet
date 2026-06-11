//! Fleet Hub binary entry-point (the engineering spec).
//!
//! Starts the Hub daemon: acquires the single-instance lock (D2), binds a
//! WebSocket listener (always) plus a unix socket on `cfg(unix)` (D7), and
//! serves subscribe→snapshot + delta broadcast until killed. The Hub never
//! auto-exits (D2).

use fleet_hub::{HubConfig, LockError};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Minimal CLI: the Hub takes no runtime args, but must answer --help/--version
    // cleanly instead of silently starting the daemon (a surprising footgun).
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "fleet-hub — the Fleet Hub daemon\n\n\
             Usage: fleet-hub\n\n\
             Starts the always-on Hub (WebSocket 127.0.0.1:51777 + unix socket on Unix).\n\
             The Hub never auto-exits (D2); stop it with Ctrl-C or SIGTERM.\n\
             Logging honors RUST_LOG (default \"info\").\n\n\
             Options:\n  \
             -h, --help     Print this help\n  \
             -V, --version  Print version"
        );
        return std::process::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("fleet-hub {}", env!("CARGO_PKG_VERSION"));
        return std::process::ExitCode::SUCCESS;
    }

    // Structured logging (the engineering spec "structured logging"). Honors RUST_LOG.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = HubConfig::default();
    tracing::info!(
        ws = %config.ws_addr,
        unix = %config.unix_path.display(),
        "starting Fleet Hub"
    );

    match fleet_hub::run(config).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // A second-instance refusal is the expected, non-crash failure (D2):
            // report it cleanly with a distinct exit code rather than a panic.
            if e.downcast_ref::<LockError>().is_some() {
                tracing::error!(error = %e, "refusing to start: another Fleet Hub is running");
                std::process::ExitCode::from(2)
            } else {
                tracing::error!(error = %e, "Fleet Hub exited with error");
                std::process::ExitCode::FAILURE
            }
        }
    }
}
