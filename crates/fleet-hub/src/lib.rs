//! Fleet Hub library (PLAN S2).
//!
//! The Hub is the single authoritative broker (README §4.3, §6): reporters push
//! session/run deltas in; faces subscribe and receive a snapshot followed by a
//! live delta stream. This crate provides:
//!
//! - [`merge::MergeEngine`] — the canonical merge engine holding sessions/runs,
//!   applying add/update/remove deltas, and maintaining each session's
//!   `rollup_state`/`rollup_urgency` as the most-urgent across its runs.
//! - [`wire::ClientMessage`] — the Hub-internal inbound vocabulary (`subscribe`,
//!   reporter deltas, face commands).
//! - [`server`] — the tokio server binding a WebSocket listener (always) and a
//!   unix-domain socket on `cfg(unix)` (PLAN D7), with subscribe→snapshot and
//!   delta broadcast.
//! - [`lockfile::InstanceLock`] — the single-instance guard (PLAN D2).
//!
//! Locked decisions honored: **D1** (Rust), **D2** (never auto-exit; lockfile
//! single-instance), **D6** (JSON wire), **D7** (WebSocket everywhere + unix
//! fast path on `cfg(unix)`). The Hub is an **observer, not owner** (invariant
//! 3): it never launches an agent; it only merges what reporters tell it.

// The crate is unsafe-free except for one small `kill(2)` FFI in `lockfile`
// (under `cfg(unix)`, with a SAFETY note). We `deny` rather than `forbid` so
// that single, audited use can opt in with a localized `#[allow(unsafe_code)]`.
#![deny(unsafe_code)]

pub mod lockfile;
pub mod merge;
pub mod server;
pub mod wire;

use std::net::SocketAddr;
use std::path::PathBuf;

pub use lockfile::{InstanceLock, LockError};
pub use merge::MergeEngine;
pub use server::HubState;
pub use wire::ClientMessage;

/// Runtime configuration for a Hub daemon.
#[derive(Debug, Clone)]
pub struct HubConfig {
    /// Address the WebSocket listener binds (always). Default loopback:0 picks
    /// an OS port; production binds a fixed port (see [`Self::default`]).
    pub ws_addr: SocketAddr,
    /// Path to the unix-domain socket (`cfg(unix)` fast path, D7).
    pub unix_path: PathBuf,
    /// Path to the single-instance lockfile (D2).
    pub lock_path: PathBuf,
}

/// The default Hub state directory, `$XDG_RUNTIME_DIR/fleet` or a temp fallback.
fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("FLEET_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(unix)]
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        let mut p = PathBuf::from(dir);
        p.push("fleet");
        return p;
    }
    let mut p = std::env::temp_dir();
    p.push("fleet");
    p
}

/// The default WebSocket port the Hub advertises (loopback only — the Hub is a
/// local daemon, not a network service).
pub const DEFAULT_WS_PORT: u16 = 51_777;

impl Default for HubConfig {
    fn default() -> Self {
        let dir = state_dir();
        // `FLEET_WS_PORT` overrides the bind port (0 ⇒ OS-assigned ephemeral).
        // Used by tests to avoid colliding on the fixed default port.
        let port = std::env::var("FLEET_WS_PORT")
            .ok()
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(DEFAULT_WS_PORT);
        HubConfig {
            ws_addr: SocketAddr::from(([127, 0, 0, 1], port)),
            unix_path: dir.join("hub.sock"),
            lock_path: dir.join("hub.lock"),
        }
    }
}

/// Run the Hub daemon to completion (i.e. forever — D2: never auto-exits).
///
/// Acquires the single-instance lock first (refusing if another Hub is up),
/// then binds the WebSocket listener (always) and the unix socket (`cfg(unix)`),
/// and serves both until the process is killed.
pub async fn run(config: HubConfig) -> anyhow::Result<()> {
    // D2: single-instance guard. Held for the lifetime of the daemon.
    let _lock = InstanceLock::acquire(&config.lock_path)?;
    tracing::info!(lock = %config.lock_path.display(), "single-instance lock acquired");

    let state = HubState::new();

    // WS listener (always — D7).
    let (ws_local, ws_fut) = server::run_ws_listener(state.clone(), config.ws_addr).await?;
    tracing::info!(%ws_local, "Fleet Hub up (WebSocket)");

    #[cfg(unix)]
    {
        let unix_fut = server::run_unix_listener(state.clone(), config.unix_path.clone()).await?;
        tracing::info!(path = %config.unix_path.display(), "Fleet Hub up (unix socket)");
        tokio::join!(ws_fut, unix_fut);
    }
    #[cfg(not(unix))]
    {
        ws_fut.await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_loopback_and_named() {
        let c = HubConfig::default();
        assert!(c.ws_addr.ip().is_loopback());
        assert_eq!(c.ws_addr.port(), DEFAULT_WS_PORT);
        assert!(c.unix_path.to_string_lossy().ends_with("hub.sock"));
        assert!(c.lock_path.to_string_lossy().ends_with("hub.lock"));
    }

    #[test]
    fn fleet_runtime_dir_override_honored() {
        std::env::set_var("FLEET_RUNTIME_DIR", "/tmp/fleet-test-override");
        let c = HubConfig::default();
        assert!(c.unix_path.starts_with("/tmp/fleet-test-override"));
        std::env::remove_var("FLEET_RUNTIME_DIR");
    }
}
