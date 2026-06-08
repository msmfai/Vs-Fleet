//! Fleet Hub binary entry-point (PLAN S2).
//!
//! Starts the Hub daemon: acquires the single-instance lock (D2), binds a
//! WebSocket listener (always) plus a unix socket on `cfg(unix)` (D7), and
//! serves subscribe→snapshot + delta broadcast until killed. The Hub never
//! auto-exits (D2).

use fleet_hub::{HubConfig, LockError};

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Structured logging (PLAN S2 "structured logging"). Honors RUST_LOG.
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
