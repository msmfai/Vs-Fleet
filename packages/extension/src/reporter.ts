/**
 * Reporter supervisor — spawns and supervises the per-window
 * `fleet-reporter --serve` child process (REPSERVE).
 *
 * Each editor window runs its own reporter: it binds the window's reporter
 * socket (`FLEET_REPORTER_SOCKET`), registers the window session with the Hub,
 * and receives the Claude/Codex hooks the shim points at that socket — turning
 * them into Hub deltas. The extension owns the process lifetime: it starts the
 * reporter on activation and kills it on deactivation (reversibility).
 *
 * Observer-not-owner: the reporter only *observes* agents via their hooks; it
 * never launches or drives an agent. The supervisor only manages the reporter
 * process, not any agent.
 */

import { type ChildProcess, spawn } from "child_process";

/** How the supervisor locates and launches the reporter binary. */
export interface ReporterOptions {
    /** Absolute path to the `fleet-reporter` binary. */
    binPath: string;
    /** The per-window reporter socket to bind (`FLEET_REPORTER_SOCKET`). */
    reporterSocket: string;
    /** The window's Fleet session id (registered with the Hub). */
    sessionId: string;
    /** Hub endpoint as resolved by `connection.resolveEndpoint` (`unix:/p` or `ws://…`). */
    hubEndpoint: string;
    /** Restart backoff in ms (default 1000). A crashed reporter is restarted. */
    restartDelayMs?: number;
    /** Injected spawner (tests). Defaults to Node's `child_process.spawn`. */
    spawnFn?: typeof spawn;
}

/**
 * Translate a resolved Hub endpoint into `fleet-reporter` CLI args. `unix:/p` →
 * `--unix /p`; a `ws://…` URL → `--ws <url>`. Pure + exported for tests.
 */
export function reporterArgs(opts: {
    reporterSocket: string;
    sessionId: string;
    hubEndpoint: string;
}): string[] {
    const args = ["--serve", "--session-id", opts.sessionId, "--socket", opts.reporterSocket];
    if (opts.hubEndpoint.startsWith("unix:")) {
        args.push("--unix", opts.hubEndpoint.slice("unix:".length));
    } else {
        args.push("--ws", opts.hubEndpoint);
    }
    return args;
}

/**
 * Supervises one `fleet-reporter --serve` child. Starts it, restarts it on
 * unexpected exit (with backoff), and stops it on `dispose()`.
 */
export class ReporterSupervisor {
    private readonly _opts: Required<Pick<ReporterOptions, "restartDelayMs">> & ReporterOptions;
    private _child: ChildProcess | null = null;
    private _disposed = false;
    private _restartTimer: ReturnType<typeof setTimeout> | null = null;
    private _starts = 0;

    constructor(opts: ReporterOptions) {
        this._opts = { restartDelayMs: 1000, ...opts };
    }

    /** Number of times the reporter has been (re)started (observability/tests). */
    get startCount(): number {
        return this._starts;
    }

    /** The current child process (null before start / after a crash). */
    get child(): ChildProcess | null {
        return this._child;
    }

    /** Start (or restart) the reporter child. Idempotent while one is alive. */
    start(): void {
        if (this._disposed || this._child) return;
        const spawnFn = this._opts.spawnFn ?? spawn;
        this._starts += 1;
        const child = spawnFn(
            this._opts.binPath,
            reporterArgs(this._opts),
            {
                // Pin the reporter socket via env too, so the Rust
                // `default_reporter_socket` agrees even if `--socket` is dropped.
                env: { ...process.env, FLEET_REPORTER_SOCKET: this._opts.reporterSocket },
                stdio: "ignore",
                detached: false,
            }
        );
        this._child = child;
        child.on("exit", () => {
            this._child = null;
            if (this._disposed) return;
            // Unexpected exit — restart with backoff.
            this._restartTimer = setTimeout(() => {
                this._restartTimer = null;
                this.start();
            }, this._opts.restartDelayMs);
        });
        child.on("error", () => {
            // A spawn error (e.g. binary missing) leaves _child set; clear it so a
            // later start() can retry, but do not hot-loop here.
            this._child = null;
        });
    }

    /** Stop the reporter and cancel any pending restart (reversibility). */
    dispose(): void {
        this._disposed = true;
        if (this._restartTimer) {
            clearTimeout(this._restartTimer);
            this._restartTimer = null;
        }
        if (this._child) {
            this._child.kill();
            this._child = null;
        }
    }
}
