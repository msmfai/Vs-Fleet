/**
 * Canonical Fleet socket paths (TS side) — the mirror of Rust's
 * `fleet_protocol::paths`. Keeps the extension, the spawned `fleet-reporter
 * --serve`, and the shim-written Claude hooks all pointed at the SAME per-window
 * reporter socket.
 *
 * ## The two distinct sockets (do not conflate)
 * - **Hub socket / URL** — where the extension's `HubConnection` and the
 *   reporter connect *to the Hub* (`FLEET_HUB_SOCKET` / `fleet.hubWsUrl`).
 *   Resolved by `connection.resolveEndpoint`.
 * - **Reporter socket** (this module) — where Claude/Codex hooks send payloads
 *   *to the per-window `fleet-reporter --serve`*. Injected into terminals as
 *   `FLEET_REPORTER_SOCKET`, bound by the reporter, targeted by the shim's hooks.
 */

import * as os from "os";
import * as path from "path";

/**
 * Env var that pins the reporter-socket path. The extension injects it into
 * integrated terminals (so the shim/hooks know where to send), AND passes it to
 * the spawned `fleet-reporter --serve` (whose Rust `default_reporter_socket`
 * reads the same var first), so window, reporter, and hooks all agree.
 */
export const FLEET_REPORTER_SOCKET_VAR = "FLEET_REPORTER_SOCKET";

/** The Fleet runtime sub-directory (under `$XDG_RUNTIME_DIR` or the temp dir). */
export const RUNTIME_SUBDIR = "fleet";

/** Sanitize a session id into a filesystem-safe, bounded path segment. */
function safeSegment(sessionId: string): string {
    return (sessionId || "default").replace(/[^A-Za-z0-9._-]/g, "_").slice(0, 128);
}

/**
 * The Fleet runtime directory: `$XDG_RUNTIME_DIR/fleet` when set (unix), else
 * `<os.tmpdir()>/fleet`. Mirrors the Rust resolution order (minus the explicit
 * env override, which is the *socket* var handled by the caller).
 */
export function runtimeDir(): string {
    const xdg = process.env["XDG_RUNTIME_DIR"];
    if (xdg && xdg.length > 0) return path.join(xdg, RUNTIME_SUBDIR);
    return path.join(os.tmpdir(), RUNTIME_SUBDIR);
}

/**
 * The per-window reporter socket path. Each editor window gets its own socket
 * (keyed by session id) so concurrent windows never share a reporter. This is
 * the value the extension injects as `FLEET_REPORTER_SOCKET` and passes to the
 * spawned reporter.
 */
export function reporterSocketPath(sessionId: string): string {
    return path.join(runtimeDir(), `reporter-${safeSegment(sessionId)}.sock`);
}
