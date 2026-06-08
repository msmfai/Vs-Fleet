/**
 * Unit tests for the terminal read-stream OSC recovery manager (READSTREAM S18).
 *
 * These tests verify that:
 *  - `ReadStreamManager` subscribes to `onDidStartTerminalShellExecution` on
 *    construction (stable ^1.93 API; NOT the proposed `onDidWriteTerminalData`).
 *  - OSC frames are recovered from the async read-stream and emitted to the
 *    registered callback.
 *  - `dispose()` cancels all active readers and deregisters the event listener.
 *  - Each execution gets an independent parser (cross-execution contamination is
 *    prevented).
 *  - Stream errors are handled gracefully (observer-not-owner: we just stop reading).
 *
 * ── Mock strategy ─────────────────────────────────────────────────────────────
 *
 * We use `makeShellExecEventEmitter()` from the vscode mock to create a
 * controllable `onDidStartTerminalShellExecution` emitter, and
 * `makeTerminalShellExecution(chunks)` to inject pre-recorded terminal output.
 * No real VS Code, no real terminals.
 */

import {
    makeShellExecEventEmitter,
    makeTerminalShellExecution,
    Disposable,
} from "../__mocks__/vscode";
import { ReadStreamManager } from "../readStream";
import type { OscFrame, Osc777Frame, Osc9Frame } from "../oscParser";

// ── OSC fixture helpers ───────────────────────────────────────────────────────

const BEL = "\x07";
function osc7bel(param: string): string {
    return `\x1B]${param}${BEL}`;
}

// ── Async stream helpers ──────────────────────────────────────────────────────

/**
 * Returns a Promise that resolves after all currently-queued microtasks have
 * run. Used to wait for the async reader loop to process the injected chunks.
 */
function flushMicrotasks(): Promise<void> {
    return new Promise(resolve => setImmediate(resolve));
}

// ── Basic subscription ────────────────────────────────────────────────────────

describe("ReadStreamManager construction", () => {
    it("subscribes to onDidStartTerminalShellExecution on construction", () => {
        const emitter = makeShellExecEventEmitter();
        let subscribed = false;
        const mockWindow = {
            onDidStartTerminalShellExecution: (listener: unknown) => {
                subscribed = true;
                return new Disposable(() => {});
            },
        };

        new ReadStreamManager(mockWindow as never);
        expect(subscribed).toBe(true);
    });

    it("is active immediately after construction", () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);
        expect(mgr.active).toBe(true);
    });
});

// ── OSC frame recovery ─────────────────────────────────────────────────────────

describe("OSC frame recovery via read-stream", () => {
    it("delivers an OSC 777 notify frame from a single-chunk execution", async () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        mgr.onFrame(f => frames.push(f));

        // Fire a shell execution with a single OSC frame.
        const exec = makeTerminalShellExecution([osc7bel("777;notify;done")]);
        emitter.fire({ execution: exec });

        await flushMicrotasks();
        // Give the async iterator a moment to drain.
        await flushMicrotasks();

        expect(frames).toHaveLength(1);
        expect(frames[0].kind).toBe("osc777");
        expect((frames[0] as Osc777Frame).action).toBe("notify");
        expect((frames[0] as Osc777Frame).payload).toBe("done");
    });

    it("delivers an OSC 9 CmdEnd frame", async () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        mgr.onFrame(f => frames.push(f));

        const exec = makeTerminalShellExecution([osc7bel("9;3;0")]);
        emitter.fire({ execution: exec });

        await flushMicrotasks();
        await flushMicrotasks();

        expect(frames.filter(f => f.kind === "osc9")).toHaveLength(1);
        expect((frames[0] as Osc9Frame).subcommand).toBe(3);
    });

    it("recovers multiple frames spread across chunks", async () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        mgr.onFrame(f => frames.push(f));

        const exec = makeTerminalShellExecution([
            osc7bel("777;preexec;codex"),
            "some output\r\n",
            osc7bel("9;3;0"),
        ]);
        emitter.fire({ execution: exec });

        await flushMicrotasks();
        await flushMicrotasks();
        await flushMicrotasks();

        expect(frames).toHaveLength(2);
        expect(frames[0].kind).toBe("osc777");
        expect(frames[1].kind).toBe("osc9");
    });

    it("recovers a frame whose OSC sequence is split across two chunks", async () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        mgr.onFrame(f => frames.push(f));

        // OSC sequence split: "\x1B]777;not" in chunk 1, "ify;hello\x07" in chunk 2.
        const exec = makeTerminalShellExecution([
            "\x1B]777;not",
            "ify;hello\x07",
        ]);
        emitter.fire({ execution: exec });

        await flushMicrotasks();
        await flushMicrotasks();
        await flushMicrotasks();

        expect(frames).toHaveLength(1);
        expect((frames[0] as Osc777Frame).action).toBe("notify");
        expect((frames[0] as Osc777Frame).payload).toBe("hello");
    });
});

// ── Multiple concurrent executions ────────────────────────────────────────────

describe("Multiple concurrent shell executions", () => {
    it("handles two concurrent executions independently", async () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        mgr.onFrame(f => frames.push(f));

        // Fire two executions whose streams both have frames.
        emitter.fire({ execution: makeTerminalShellExecution([osc7bel("777;preexec;a")]) });
        emitter.fire({ execution: makeTerminalShellExecution([osc7bel("9;2")]) });

        await flushMicrotasks();
        await flushMicrotasks();
        await flushMicrotasks();

        expect(frames).toHaveLength(2);
        const kinds = frames.map(f => f.kind).sort();
        expect(kinds).toContain("osc777");
        expect(kinds).toContain("osc9");
    });

    it("each execution gets an independent parser (cross-contamination guard)", async () => {
        // Execution 1: starts an OSC but never terminates it (partial frame).
        // Execution 2: has a complete, valid frame.
        // The partial frame from execution 1 must NOT contaminate execution 2.
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        mgr.onFrame(f => frames.push(f));

        // Execution 1: unterminated OSC (no ST/BEL).
        emitter.fire({ execution: makeTerminalShellExecution(["\x1B]777;pending"]) });
        // Execution 2: complete OSC.
        emitter.fire({ execution: makeTerminalShellExecution([osc7bel("9;3;0")]) });

        await flushMicrotasks();
        await flushMicrotasks();
        await flushMicrotasks();

        // Only execution 2's frame should come through.
        const osc9Frames = frames.filter(f => f.kind === "osc9");
        expect(osc9Frames).toHaveLength(1);
    });
});

// ── dispose() ────────────────────────────────────────────────────────────────

describe("dispose()", () => {
    it("deregisters the event listener (no more events after dispose)", async () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        mgr.onFrame(f => frames.push(f));

        mgr.dispose();

        // Fire after dispose — should be ignored.
        emitter.fire({ execution: makeTerminalShellExecution([osc7bel("777;notify;after-dispose")]) });

        await flushMicrotasks();
        await flushMicrotasks();

        expect(frames).toHaveLength(0);
    });

    it("sets active to false after dispose()", () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);
        mgr.dispose();
        expect(mgr.active).toBe(false);
    });

    it("is idempotent (safe to call dispose() multiple times)", () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);
        expect(() => {
            mgr.dispose();
            mgr.dispose();
        }).not.toThrow();
    });

    it("stops delivering frames after dispose() even if readers were active", async () => {
        // Create a stream that delivers chunks with a deliberate async gap.
        // We dispose mid-stream and verify no frames arrive after dispose.
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        mgr.onFrame(f => frames.push(f));

        // A "slow" stream: we create a custom async iterable that yields lazily.
        let _resolveFirst!: (value: IteratorResult<string>) => void;
        const firstChunkPromise = new Promise<IteratorResult<string>>(res => {
            _resolveFirst = res;
        });

        const slowExec = {
            createStream(): AsyncIterable<string> {
                return {
                    [Symbol.asyncIterator](): AsyncIterator<string> {
                        let step = 0;
                        return {
                            next(): Promise<IteratorResult<string>> {
                                if (step === 0) {
                                    step++;
                                    return firstChunkPromise;
                                }
                                return Promise.resolve({ value: "", done: true });
                            },
                        };
                    },
                };
            },
        };

        emitter.fire({ execution: slowExec });
        await flushMicrotasks();

        // Dispose BEFORE the first chunk is delivered.
        mgr.dispose();

        // Now deliver the chunk — the reader should be aborted.
        _resolveFirst({ value: osc7bel("777;notify;should-not-arrive"), done: false });
        await flushMicrotasks();
        await flushMicrotasks();

        expect(frames).toHaveLength(0);
    });
});

// ── onFrame callback ──────────────────────────────────────────────────────────

describe("onFrame callback management", () => {
    it("the unsubscribe function stops delivery", async () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        const unsub = mgr.onFrame(f => frames.push(f));
        unsub();

        emitter.fire({ execution: makeTerminalShellExecution([osc7bel("777;notify;x")]) });

        await flushMicrotasks();
        await flushMicrotasks();

        expect(frames).toHaveLength(0);
    });

    it("re-registering a callback replaces the previous one", async () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const first: OscFrame[] = [];
        const second: OscFrame[] = [];
        mgr.onFrame(f => first.push(f));
        mgr.onFrame(f => second.push(f)); // replaces first

        emitter.fire({ execution: makeTerminalShellExecution([osc7bel("777;preexec")]) });

        await flushMicrotasks();
        await flushMicrotasks();

        expect(first).toHaveLength(0);
        expect(second.length).toBeGreaterThanOrEqual(1);
    });
});

// ── Error resilience ─────────────────────────────────────────────────────────

describe("Error resilience (observer-not-owner: just stop reading)", () => {
    it("handles a stream that throws synchronously without crashing", async () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        mgr.onFrame(f => frames.push(f));

        // A stream whose async iterator throws on first next().
        const errorExec = {
            createStream(): AsyncIterable<string> {
                return {
                    [Symbol.asyncIterator](): AsyncIterator<string> {
                        return {
                            next(): Promise<IteratorResult<string>> {
                                return Promise.reject(new Error("terminal closed"));
                            },
                        };
                    },
                };
            },
        };

        expect(() => emitter.fire({ execution: errorExec })).not.toThrow();
        await flushMicrotasks();
        await flushMicrotasks();

        // No frames, no crash.
        expect(frames).toHaveLength(0);
        // Manager should still be active.
        expect(mgr.active).toBe(true);
    });

    it("continues to handle subsequent executions after a stream error", async () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        mgr.onFrame(f => frames.push(f));

        // First execution throws.
        const errorExec = {
            createStream(): AsyncIterable<string> {
                return {
                    [Symbol.asyncIterator](): AsyncIterator<string> {
                        return {
                            next: () => Promise.reject(new Error("bang")),
                        };
                    },
                };
            },
        };
        emitter.fire({ execution: errorExec });
        await flushMicrotasks();

        // Second execution succeeds.
        emitter.fire({ execution: makeTerminalShellExecution([osc7bel("9;2")]) });
        await flushMicrotasks();
        await flushMicrotasks();

        const osc9Frames = frames.filter(f => f.kind === "osc9");
        expect(osc9Frames).toHaveLength(1);
    });

    it("handles an execution whose createStream() throws synchronously", async () => {
        const emitter = makeShellExecEventEmitter();
        const mgr = new ReadStreamManager(emitter);

        const frames: OscFrame[] = [];
        mgr.onFrame(f => frames.push(f));

        const throwingExec = {
            createStream(): AsyncIterable<string> {
                throw new Error("createStream() failed");
            },
        };

        // This should not propagate to the emitter caller.
        expect(() => emitter.fire({ execution: throwingExec as never })).not.toThrow();
        await flushMicrotasks();

        expect(frames).toHaveLength(0);
        expect(mgr.active).toBe(true);
    });
});

// ── Integration: extension activate wires up the read-stream ──────────────────

describe("Extension wiring: activate() installs ReadStreamManager", () => {
    it("getReadStreamManager export exists", () => {
        // The full activate() wiring is tested in extension.test.ts.
        // Here we just verify the accessor export is present.
        // eslint-disable-next-line @typescript-eslint/no-require-imports
        const mod = require("../extension") as { getReadStreamManager?: unknown };
        expect(typeof mod.getReadStreamManager).toBe("function");
    });
});
