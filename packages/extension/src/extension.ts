/**
 * Fleet VS Code Extension — EXTSKEL (S8).
 *
 * Activation entrypoint. On activation:
 *   1. Reads config to find the Hub endpoint (WS URL or unix socket path).
 *   2. Opens a `HubConnection` that sends `subscribe` and periodic heartbeats.
 *   3. Creates a `FleetStatusBar` that mirrors the connection state.
 *   4. Registers the `fleet.showStatus` command.
 *
 * All cleanup happens via `context.subscriptions` — VS Code disposes them on
 * deactivation (the extension deactivates on window close or disable), so the
 * connection is closed cleanly without a reference cycle.
 *
 * Locked decisions honored:
 *   - D14 — Open-VSX-publishable, NO `enabledApiProposals`, engine `^1.93.0`.
 *   - D14 — stable `EnvironmentVariableCollection` + shell-integration ONLY
 *     (S9/S18; not needed yet at S8 but the architecture is ready for it).
 *   - Observer-not-owner (invariant 3): the extension only registers presence;
 *     it never intercepts keystrokes, launches agents, or owns terminals.
 *   - Reversibility (invariant 6): deactivate disposes all subscriptions,
 *     closes the socket, and leaves the editor's state unchanged.
 */

import * as vscode from "vscode";
import {
    HubConnection,
    resolveEndpoint,
    resolveSessionId,
} from "./connection";
import { EnvInjector } from "./envInject";
import { reporterSocketPath } from "./paths";
import { ReadStreamManager } from "./readStream";
import { ReporterSupervisor } from "./reporter";
import { PathShimmer, defaultShimDir, hasAnyReliabilityFlag, type ReliabilityConfig } from "./shim";
import { FleetStatusBar } from "./statusBar";

/** The single live connection (null until activate() runs). */
let _connection: HubConnection | null = null;

/** The single live env injector (null until activate() runs). */
let _envInjector: EnvInjector | null = null;

/** The single live PATH shimmer (null until activate() runs). */
let _pathShimmer: PathShimmer | null = null;

/** The single live read-stream OSC recovery manager (null until activate() runs). */
let _readStreamManager: ReadStreamManager | null = null;

/** The single live reporter supervisor (null until activate() runs). */
let _reporterSupervisor: ReporterSupervisor | null = null;

/**
 * VS Code calls this when the extension activates (onStartupFinished).
 *
 * The function is intentionally synchronous at the top level — we kick off
 * the connection asynchronously and surface status through the status bar, so
 * VS Code startup is not gated on the Hub being reachable.
 */
export function activate(context: vscode.ExtensionContext): void {
    const cfg = vscode.workspace.getConfiguration("fleet");

    const endpoint = resolveEndpoint(cfg);
    const sessionId = resolveSessionId();
    const heartbeatIntervalMs = cfg.get<number>(
        "heartbeatIntervalMs",
        10_000
    );

    // The per-window reporter socket: where this window's `fleet-reporter
    // --serve` listens for Claude/Codex hooks. Injected into terminals, bound by
    // the reporter, targeted by the shim's hooks — one path, three consumers.
    const reporterSocket = reporterSocketPath(sessionId);

    // ENVINJ (S9): inject FLEET_SESSION_ID (per window) + the reporter endpoint
    // into every integrated-terminal shell via the STABLE
    // EnvironmentVariableCollection API (workspace-scoped, reversible). This is
    // what lets a `claude`/`codex` run started in this window's terminal be
    // correlated back to the editor window, and tells SHIM/hooks (S10+) where the
    // reporter socket lives. `context.environmentVariableCollection` is the
    // extension's own per-workspace collection; the platform auto-clears it on
    // uninstall, and dispose() clears it on disable/reload.
    const envInjector = new EnvInjector(context.environmentVariableCollection);
    _envInjector = envInjector;
    envInjector.inject({
        sessionId,
        reporterSocket,
    });

    // SHIM (S10): prepend a per-window dir of transparent `claude`/`codex`
    // wrapper scripts (B′) to the integrated-terminal PATH (same stable
    // EnvironmentVariableCollection, via `prepend` — the list-like PATH case).
    // The user still TYPES `claude`/`codex`; the wrapper transparently execs the
    // real binary with args forwarded verbatim, getting hooks pointed at the
    // reporter (CODEX/CLUSETERM, S11+). Outside the editor the shim dir is absent
    // from PATH (pass-through). Any reliability flag is OPT-IN + surfaced, never
    // silent (confidence-honesty, §3 invariant 3): we DO NOT default
    // --allow-dangerously-skip-permissions.
    const reliability: ReliabilityConfig = {
        claudeSkipPermissions: cfg.get<boolean>(
            "claude.allowDangerouslySkipPermissions",
            false
        ),
    };
    const pathShimmer = new PathShimmer(context.environmentVariableCollection, {
        shimDir: defaultShimDir(sessionId),
        reliability,
        // The shim writes a `fleet-hooks.json` pointed at THIS window's reporter
        // socket and launches `claude --settings <that file>`, so Claude relays
        // its hooks to the reporter without touching ~/.claude/settings.json.
        reporterSocket,
    });
    _pathShimmer = pathShimmer;
    try {
        pathShimmer.install();
        // Surface active reliability flags to the user (never silent).
        if (hasAnyReliabilityFlag(reliability)) {
            const active = pathShimmer.activeReliabilityFlags;
            const all = [...active.claude, ...active.codex].join(" ");
            vscode.window.showWarningMessage(
                `Fleet: shimmed agents will launch with opt-in reliability flag(s): ${all}. ` +
                    `Disable in Settings (fleet.claude.allowDangerouslySkipPermissions) to remove.`
            );
        }
    } catch (err) {
        // The shim is best-effort: a write failure must not break the editor.
        // Without it, agents fall back to the config-only path (fleet init, S14).
        vscode.window.showWarningMessage(
            `Fleet: could not install PATH shim (${err}); falling back to config-only detection.`
        );
        _pathShimmer = null;
    }

    // REPSERVE: spawn this window's `fleet-reporter --serve`. It binds the
    // reporter socket, registers the window session with the Hub, and turns the
    // shim-installed Claude/Codex hooks into Hub deltas. Best-effort: a missing
    // binary surfaces a warning and falls back to the extension's own Hub
    // presence (the reporter is what makes hooks flow, not editor presence).
    const reporterBin = cfg.get<string>("reporterBinPath", "fleet-reporter");
    const reporterSupervisor = new ReporterSupervisor({
        binPath: reporterBin,
        reporterSocket,
        sessionId,
        hubEndpoint: endpoint,
    });
    _reporterSupervisor = reporterSupervisor;
    try {
        reporterSupervisor.start();
    } catch (err) {
        vscode.window.showWarningMessage(
            `Fleet: could not start the reporter (${err}); set "fleet.reporterBinPath" to the fleet-reporter binary.`
        );
    }

    // Status bar: shows connection state throughout the window lifetime.
    const statusBar = new FleetStatusBar(context);

    // READSTREAM (S18): subscribe to onDidStartTerminalShellExecution (STABLE,
    // ^1.93 — NOT onDidWriteTerminalData which is permanently proposed / D14) to
    // recover dropped OSC 9/777/99 frames from the integrated-terminal read-stream.
    // Observer-not-owner (invariant 3): the stream is read-only; we never write
    // back to the terminal. Reversible (invariant 6): dispose() cancels all readers.
    const readStreamMgr = new ReadStreamManager(vscode.window as unknown as import("./readStream").WindowLike);
    _readStreamManager = readStreamMgr;
    // Open the Hub connection.
    const conn = HubConnection.open(endpoint, sessionId, heartbeatIntervalMs);
    _connection = conn;

    // Route recovered OSC frames to the Hub connection so the reporter can
    // corroborate agent state. Each frame carries enough structure for the Hub to
    // apply the appropriate state update without a real agent binary being present.
    readStreamMgr.onFrame(frame => {
        // Use the local `conn` reference (closed over); it is always initialized
        // at this point because onFrame fires only after shell executions begin.
        conn.send({
            type: "osc.frame",
            session_id: sessionId,
            frame,
        });
    });

    // Mirror connection status → status bar.
    const unsub = conn.onStatusChange(ev => statusBar.update(ev));

    // Register `fleet.showStatus` command (contributes.commands in package.json).
    const cmd = vscode.commands.registerCommand("fleet.showStatus", () => {
        vscode.window.showInformationMessage(
            `Fleet Hub: ${conn.status} — ${conn.detail || endpoint}`
        );
    });

    // Push everything that must be cleaned up into subscriptions.
    // VS Code calls dispose() on each entry when the extension deactivates.
    context.subscriptions.push(
        cmd,
        {
            dispose(): void {
                unsub();
                conn.dispose();
                _connection = null;
                // Stop this window's reporter (reversibility): kill the child and
                // cancel any pending restart.
                reporterSupervisor.dispose();
                _reporterSupervisor = null;
                // READSTREAM cleanup (reversibility): cancel all in-flight readers
                // and deregister the shell execution event listener.
                readStreamMgr.dispose();
                _readStreamManager = null;
                // Reversibility (invariant 6): remove the PATH shim (mutator +
                // on-disk scripts) BEFORE the env injector clears the whole
                // collection, then remove all injected env vars. On uninstall the
                // platform also invalidates the collection, but for disable/reload
                // we clean up ourselves.
                pathShimmer.dispose();
                _pathShimmer = null;
                envInjector.dispose();
                _envInjector = null;
            },
        }
    );
}

/**
 * VS Code calls this when the extension deactivates (window close, disable,
 * reload). The subscriptions registered in `activate` are already disposed by
 * VS Code before this runs, so this is a belt-and-suspenders safety net.
 */
export function deactivate(): void {
    if (_connection) {
        _connection.dispose();
        _connection = null;
    }
    if (_readStreamManager) {
        _readStreamManager.dispose();
        _readStreamManager = null;
    }
    if (_reporterSupervisor) {
        _reporterSupervisor.dispose();
        _reporterSupervisor = null;
    }
    if (_pathShimmer) {
        _pathShimmer.dispose();
        _pathShimmer = null;
    }
    if (_envInjector) {
        _envInjector.dispose();
        _envInjector = null;
    }
}

/**
 * Accessor for the current connection (used by tests and future slices like
 * ENVINJ/S9 that need to send session deltas over the same socket).
 */
export function getConnection(): HubConnection | null {
    return _connection;
}

/**
 * Accessor for the current env injector (used by tests). Null before activate()
 * and after deactivate().
 */
export function getEnvInjector(): EnvInjector | null {
    return _envInjector;
}

/**
 * Accessor for the current PATH shimmer (used by tests). Null before activate(),
 * after deactivate(), or if shim installation failed (config-only fallback).
 */
export function getPathShimmer(): PathShimmer | null {
    return _pathShimmer;
}

/**
 * Accessor for the current read-stream manager (used by tests). Null before
 * activate() and after deactivate().
 */
export function getReadStreamManager(): ReadStreamManager | null {
    return _readStreamManager;
}

/**
 * Accessor for the current reporter supervisor (used by tests). Null before
 * activate() and after deactivate().
 */
export function getReporterSupervisor(): ReporterSupervisor | null {
    return _reporterSupervisor;
}
