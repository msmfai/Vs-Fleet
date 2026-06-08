/**
 * Fleet terminal read-stream OSC recovery — READSTREAM (S18).
 *
 * Uses the STABLE `onDidStartTerminalShellExecution` + `execution.read()` APIs
 * (VS Code ^1.93.0) to subscribe to raw terminal output from shell-integrated
 * terminals and pass it through the OSC parser to recover dropped OSC 9/777
 * (and future OSC 99) frames.
 *
 * ── WHY NOT `onDidWriteTerminalData` ─────────────────────────────────────────
 *
 * `onDidWriteTerminalData` is permanently proposed (vscode #145234) — it is NOT
 * stable and is NOT accessible without `enabledApiProposals`. Using it would
 * violate D14 (Open-VSX-publishable, no proposed APIs, engine ^1.93.0). This
 * node uses ONLY the stable `onDidStartTerminalShellExecution` +
 * `TerminalShellExecution.createStream()` API available since VS Code 1.93.
 *
 * ── WHAT THIS RECOVERS ────────────────────────────────────────────────────────
 *
 * When an agent (Claude, Codex, or any shell command) emits OSC 9/777 sequences
 * into the terminal, the VS Code renderer may drop them before the extension host
 * processes them (a known timing issue). The read-stream receives the raw text
 * output of the shell execution BEFORE the renderer has a chance to drop OSC
 * frames, allowing Fleet to recover them and corroborate the state reported via
 * hooks.
 *
 * Specifically:
 *   - OSC 9;3 / OSC 777;postexec  → command finished (corroborates `done`/`idle`)
 *   - OSC 777;notify              → agent turn-complete notification
 *   - OSC 9;1 / OSC 777;preexec   → command about to execute (corroborates `working`)
 *
 * ── OBSERVER-NOT-OWNER ────────────────────────────────────────────────────────
 *
 * The read-stream is READ-ONLY. Fleet never writes to the terminal, never
 * intercepts keystrokes, and never drives the agent. `onDidStartTerminalShellExecution`
 * delivers events passively; `createStream()` creates an async iterable that Fleet
 * reads but cannot write back to (invariant 3: observer-not-owner).
 *
 * ── REVERSIBILITY ────────────────────────────────────────────────────────────
 *
 * `ReadStreamManager.dispose()` cancels all active stream subscriptions and
 * deregisters the event listener. No state is left behind (invariant 6).
 *
 * ── DECOUPLED FROM REPORTER ──────────────────────────────────────────────────
 *
 * This module emits `OscFrame` objects via a callback; the REPORTER integration
 * (routing recovered frames to the Hub connection) is the caller's responsibility.
 * This keeps the parser+recovery logic independently testable without any network.
 */

import type * as vscodeNs from "vscode";
import type { OscFrame } from "./oscParser";
import { OscParser } from "./oscParser";

// ── Terminal execution event types (stable ^1.93 surface) ─────────────────────

/**
 * The subset of the stable `TerminalShellExecution` API that `ReadStreamManager`
 * depends on. Declaring it locally (structural typing) makes the unit trivially
 * mockable without importing the full vscode module.
 *
 * API reference: https://code.visualstudio.com/api/references/vscode-api
 *   `TerminalShellExecution` — available since VS Code 1.93 (stable).
 */
export interface TerminalShellExecutionLike {
    /** Returns an async iterable of raw terminal output chunks. */
    createStream(): AsyncIterable<string>;
}

/**
 * The subset of `TerminalShellExecutionStartEvent` we use.
 */
export interface TerminalShellExecutionStartEventLike {
    execution: TerminalShellExecutionLike;
}

/**
 * The subset of the `vscode.window` namespace we use (structural, mockable).
 */
export interface WindowLike {
    onDidStartTerminalShellExecution(
        listener: (e: TerminalShellExecutionStartEventLike) => void
    ): vscodeNs.Disposable;
}

// ── ReadStreamManager ─────────────────────────────────────────────────────────

/**
 * Manages Fleet's subscription to terminal read-streams.
 *
 * On construction it subscribes to `window.onDidStartTerminalShellExecution`.
 * For each new shell execution it spawns an async reader that passes raw chunks
 * through the OSC parser, emitting recovered OSC frames to the registered
 * callback.
 *
 * Lifecycle mirrors `EnvInjector` and `PathShimmer`: construct on activation,
 * call `dispose()` on deactivation.
 */
export class ReadStreamManager {
    private readonly _window: WindowLike;
    private readonly _parser: OscParser;
    private _onFrame: ((frame: OscFrame) => void) | null = null;
    private _subscription: vscodeNs.Disposable | null = null;
    /** Set of currently-active reader cancellation tokens (AbortController). */
    private readonly _readers = new Set<AbortController>();
    private _disposed = false;

    constructor(window: WindowLike) {
        this._window = window;
        this._parser = new OscParser();
        this._listen();
    }

    /**
     * Register a callback to receive OSC frames recovered from the read-stream.
     * Returns an unsubscribe function (the manager allows at most one handler;
     * assigning a new one replaces the previous one).
     */
    onFrame(cb: (frame: OscFrame) => void): () => void {
        this._onFrame = cb;
        return () => {
            if (this._onFrame === cb) this._onFrame = null;
        };
    }

    /** True if the manager is active (not yet disposed). */
    get active(): boolean {
        return !this._disposed;
    }

    /**
     * Cancel all active readers and deregister the event listener.
     * Safe to call multiple times (idempotent). Observer-not-owner: we only
     * unsubscribe; we never write to any terminal.
     */
    dispose(): void {
        if (this._disposed) return;
        this._disposed = true;

        // Cancel all in-flight async readers.
        for (const ctrl of this._readers) {
            ctrl.abort();
        }
        this._readers.clear();

        // Deregister the event listener.
        if (this._subscription) {
            this._subscription.dispose();
            this._subscription = null;
        }

        this._onFrame = null;
        this._parser.reset();
    }

    // ── Private ───────────────────────────────────────────────────────────────

    private _listen(): void {
        this._subscription = this._window.onDidStartTerminalShellExecution(event => {
            if (this._disposed) return;
            this._startReader(event.execution);
        });
    }

    /**
     * Spawn an async reader for a single shell execution. The reader pumps
     * chunks from `execution.createStream()` into the OSC parser until the
     * stream ends or the manager is disposed.
     *
     * Each execution gets its own OSC parser instance so that a partially
     * buffered sequence from one execution cannot contaminate the next.
     */
    private _startReader(execution: TerminalShellExecutionLike): void {
        const ctrl = new AbortController();
        this._readers.add(ctrl);

        // Each execution gets a fresh parser to avoid cross-execution contamination.
        const parser = new OscParser();
        parser.onFrame(frame => {
            if (!this._disposed && this._onFrame) {
                this._onFrame(frame);
            }
        });

        const run = async (): Promise<void> => {
            try {
                const stream = execution.createStream();
                for await (const chunk of stream) {
                    if (ctrl.signal.aborted || this._disposed) break;
                    parser.push(chunk);
                }
            } catch {
                // Stream errors (e.g. terminal closed) are expected and should not
                // throw — the reader simply ends.
            } finally {
                parser.reset();
                this._readers.delete(ctrl);
            }
        };

        run().catch(() => {
            // Should not happen (errors are caught inside), but be safe.
            this._readers.delete(ctrl);
        });
    }
}
