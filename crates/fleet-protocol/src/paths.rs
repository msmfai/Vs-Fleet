//! Canonical Fleet socket paths — the single source of truth that closes the
//! reporter-socket naming seam across the three places that must agree:
//!
//! 1. **`fleet-reporter --serve`** binds [`default_reporter_socket`] and listens
//!    for hook payloads on it.
//! 2. **`fleet init`** writes Claude/Codex hook commands that send their payloads
//!    *to that same path* (see `fleet-cli`'s `init::InitConfig`).
//! 3. **The VS Code extension** injects that same path into integrated-terminal
//!    shells as `FLEET_REPORTER_SOCKET`, so a shim/hook launched in the shell
//!    knows where the reporter is listening.
//!
//! Historically these drifted (the extension called it `FLEET_HUB_SOCKET`, which
//! conflated the Hub's own socket with the reporter's receiver socket). This
//! module fixes the drift: there is exactly **one** reporter-socket resolution
//! rule, and everyone calls it.
//!
//! ## The two distinct sockets (do not conflate)
//! - **Hub socket** (`hub.sock`) — where reporters and faces connect *to the
//!   Hub*. Owned by `fleet-hub`.
//! - **Reporter socket** (`reporter.sock`, this module) — where agent hooks send
//!   payloads *to the reporter*. Owned by `fleet-reporter --serve`.

use std::path::PathBuf;

/// Environment variable that overrides the reporter-socket path everywhere.
///
/// Set by the VS Code extension (per-window endpoint injection) and honored by
/// both `fleet-reporter --serve` and `fleet init`, so a window's hooks, its
/// reporter, and its init-written config all target the same socket.
pub const REPORTER_SOCKET_ENV: &str = "FLEET_REPORTER_SOCKET";

/// The reporter socket's file name within the Fleet runtime directory.
pub const REPORTER_SOCKET_NAME: &str = "reporter.sock";

/// The Fleet runtime sub-directory name (under `$XDG_RUNTIME_DIR` or the temp
/// dir). Shared with the Hub's own socket/db/lock location.
pub const RUNTIME_SUBDIR: &str = "fleet";

/// Resolve the canonical reporter-socket path.
///
/// Resolution order (first match wins):
/// 1. the [`REPORTER_SOCKET_ENV`] environment variable, verbatim;
/// 2. on unix, `$XDG_RUNTIME_DIR/fleet/reporter.sock`;
/// 3. `<temp dir>/fleet/reporter.sock`.
///
/// This is intentionally identical to the rule `fleet init` uses for its
/// default, so the hook commands it writes always target the socket the
/// reporter will bind.
pub fn default_reporter_socket() -> PathBuf {
    if let Ok(p) = std::env::var(REPORTER_SOCKET_ENV) {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    #[cfg(unix)]
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg)
                .join(RUNTIME_SUBDIR)
                .join(REPORTER_SOCKET_NAME);
        }
    }
    std::env::temp_dir()
        .join(RUNTIME_SUBDIR)
        .join(REPORTER_SOCKET_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests mutate process-global env vars; keep them in one test so they
    // run sequentially within this binary and never race each other.
    #[test]
    fn resolution_order_is_env_then_xdg_then_temp() {
        // 1. Explicit override wins over everything.
        std::env::set_var(REPORTER_SOCKET_ENV, "/run/custom/r.sock");
        assert_eq!(
            default_reporter_socket(),
            PathBuf::from("/run/custom/r.sock"),
            "explicit override must win"
        );

        // An empty override is ignored (treated as unset).
        std::env::set_var(REPORTER_SOCKET_ENV, "");
        let resolved = default_reporter_socket();
        assert!(
            resolved.ends_with(format!("{RUNTIME_SUBDIR}/{REPORTER_SOCKET_NAME}")),
            "empty override falls through to a default ending in fleet/reporter.sock, got {resolved:?}"
        );

        std::env::remove_var(REPORTER_SOCKET_ENV);

        // 3. With no override (and, on non-unix, no XDG path), it lands in the
        //    temp dir under fleet/. On unix with XDG set, it lands under XDG —
        //    either way it ends with the same fleet/reporter.sock suffix.
        let resolved = default_reporter_socket();
        assert!(resolved.ends_with(REPORTER_SOCKET_NAME));
        assert!(resolved
            .parent()
            .map(|p| p.ends_with(RUNTIME_SUBDIR))
            .unwrap_or(false));

        // 2. On unix, a non-empty XDG_RUNTIME_DIR is used verbatim as the
        //    parent of fleet/reporter.sock; an empty one falls through to temp.
        #[cfg(unix)]
        {
            // This is the only test that touches XDG_RUNTIME_DIR (the file's
            // env-mutating tests are deliberately consolidated), so it sets a
            // deterministic value and clears it unconditionally at the end —
            // no machine-dependent save/restore branch to leave uncovered.
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/4242");
            assert_eq!(
                default_reporter_socket(),
                PathBuf::from("/run/user/4242")
                    .join(RUNTIME_SUBDIR)
                    .join(REPORTER_SOCKET_NAME),
                "non-empty XDG_RUNTIME_DIR must root the reporter socket"
            );

            std::env::set_var("XDG_RUNTIME_DIR", "");
            let temp_fallback = default_reporter_socket();
            assert!(
                temp_fallback.starts_with(std::env::temp_dir()),
                "empty XDG_RUNTIME_DIR falls through to the temp dir, got {temp_fallback:?}"
            );

            std::env::remove_var("XDG_RUNTIME_DIR");
        }
    }
}
