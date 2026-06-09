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
pub mod persist;
pub mod reclaim;
pub mod server;
pub mod wire;

use std::net::SocketAddr;
use std::path::PathBuf;

pub use lockfile::{InstanceLock, LockError};
pub use merge::MergeEngine;
pub use persist::{EventLog, PersistError, PersistEvent, StateStore, DEFAULT_REAP_GRACE};
pub use reclaim::{Decision, DurableId, ReclaimTable};
pub use server::HubState;
pub use wire::{ClientMessage, SeqStamp};

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
    /// Path to the durable SQLite event log (D3, S7). A restart replays this to
    /// restore all sessions/runs.
    pub db_path: PathBuf,
    /// Reap grace before a `dead` run is GC'd (D17: 1 h default).
    pub reap_grace: std::time::Duration,
    /// TTL before a session untouched this long is swept (S6 session-expiry GC).
    pub session_ttl: std::time::Duration,
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
            db_path: dir.join("hub.db"),
            // D17: 1 h reap grace before a `dead` run is GC'd.
            reap_grace: crate::persist::DEFAULT_REAP_GRACE,
            // Session expiry is far more lenient than dead-reaping: a live but
            // quiet session must not vanish. 24 h by default (reuses the D17
            // timer plumbing — S6/S7).
            session_ttl: std::time::Duration::from_secs(24 * 60 * 60),
        }
    }
}

/// The interval between Hub GC passes (reap + session sweep). Frequent enough
/// that a `dead` run is reaped within a minute of crossing its grace, cheap
/// enough to be negligible on a local daemon.
const GC_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Run the Hub daemon to completion (i.e. forever — D2: never auto-exits).
///
/// Acquires the single-instance lock first (refusing if another Hub is up),
/// then binds the WebSocket listener (always) and the unix socket (`cfg(unix)`),
/// and serves both until the process is killed.
pub async fn run(config: HubConfig) -> anyhow::Result<()> {
    // D2: single-instance guard. Held for the lifetime of the daemon.
    let _lock = InstanceLock::acquire(&config.lock_path)?;
    tracing::info!(lock = %config.lock_path.display(), "single-instance lock acquired");

    // The inbox is a LIVE MIRROR of whatever is currently phoning home — live
    // reporters re-register on restart. So by DEFAULT the Hub keeps state only in
    // memory: a restart is a clean slate, repopulated by live pings. This avoids
    // resurrecting dead "ghost" sessions and stale same-id reclaim across restarts.
    // Set `FLEET_PERSIST` to opt into the durable on-disk event log (D3/S7), which
    // replays to restore every session/run before serving.
    let state = if std::env::var_os("FLEET_PERSIST").is_some() {
        if let Some(parent) = config.db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let state = HubState::with_db(&config.db_path)?;
        tracing::info!(db = %config.db_path.display(), "durable state restored (FLEET_PERSIST)");
        state
    } else {
        tracing::info!("ephemeral state — live mirror, no restore across restart");
        HubState::new()
    };

    // S7/D17: periodic GC — reap `dead` runs past the grace, sweep expired
    // sessions. Spawned as a background task; the Hub itself never auto-exits (D2).
    {
        let gc_state = state.clone();
        let grace = config.reap_grace;
        let ttl = config.session_ttl;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(GC_INTERVAL);
            // Skip the immediate first tick so we don't GC the instant we start.
            tick.tick().await;
            loop {
                tick.tick().await;
                let now = persist::now_iso();
                match gc_state.gc(&now, grace, ttl).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(reaped = n, "GC pass removed entries"),
                    Err(e) => tracing::error!(error = %e, "GC pass failed"),
                }
            }
        });
    }

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
