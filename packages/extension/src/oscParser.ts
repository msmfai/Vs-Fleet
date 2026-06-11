/**
 * Fleet OSC byte-stream parser — READSTREAM (S18).
 *
 * Parses OSC (Operating System Command) escape sequences from a raw terminal
 * read-stream to recover shell integration and agent lifecycle events that the
 * terminal renderer may have dropped before they could be processed by the
 * extension host.
 *
 * ── WHY THIS EXISTS ────────────────────────────────────────────────────────────
 *
 * The VS Code terminal renderer can drop OSC frames (e.g. OSC 9/777 shell
 * integration sequences) under certain timing conditions — the renderer and the
 * extension host are in separate processes and the data races. engineering spec §1 job 3:
 * "recover dropped OSC 9/777 via the stable shell-integration read-stream".
 * OSC 99 is included for future-proofing (vscode #294247).
 *
 * ── STABLE API ONLY ────────────────────────────────────────────────────────────
 *
 * This module is PURE (no I/O, no vscode imports). The caller (`readStream.ts`)
 * subscribes to `onDidStartTerminalShellExecution` and passes chunks from
 * `execution.createStream().read()` into `OscParser`. No `onDidWriteTerminalData`
 * (permanently proposed — engineering spec §1 / D14). Engine ^1.93.0, Open-VSX-publishable.
 *
 * ── OSC SEQUENCE GRAMMAR ───────────────────────────────────────────────────────
 *
 * An OSC sequence has the form:
 *
 *   ESC ]  <params> ST
 *
 * Where:
 *   ESC  = 0x1B (or C1 byte 0x9D for 8-bit, which we also handle)
 *   ]    = 0x5D  (only used after ESC 0x1B form)
 *   ST   = String Terminator: ESC \ (0x1B 0x5C) or BEL (0x07) or 0x9C (8-bit)
 *
 * <params> is the OSC parameter string, e.g. "9;message" or "777;preexec;shell".
 *
 * ── OSC CODES OF INTEREST ──────────────────────────────────────────────────────
 *
 *   OSC 9   — ConEmu / VS Code shell integration notifications
 *             param format: "9;N;payload" or "9;N" where N is a subcommand
 *             Subcommands relevant to Fleet:
 *               9;1   = PreCmdExec (shell about to execute a command)
 *               9;2   = CmdStart (command execution started)
 *               9;3   = CmdEnd  (command finished, optionally with exit code)
 *               9;4   = CwdChange (current working directory changed)
 *
 *   OSC 777 — iTerm2 / VS Code shell integration (most common for activity)
 *             param format: "777;action;payload" or "777;action"
 *             Actions relevant to Fleet:
 *               notify        = shell notification / agent turn complete
 *               preexec       = shell about to execute
 *               precmd        = prompt about to appear
 *               postexec      = command just finished
 *               workdir       = working directory change
 *
 *   OSC 99  — future VS Code shell integration (vscode #294247); treated as an
 *             opaque payload delivered to the caller for forward-compatibility.
 *             (This issues the "future-proof for OSC 99" requirement from engineering spec §1.)
 *
 * ── ERROR HANDLING ─────────────────────────────────────────────────────────────
 *
 * Malformed frames are silently skipped (never panic, never mis-state — engineering spec §2
 * G2 criterion: "schema-drift fuzz → degrades gracefully"). Each frame is parsed
 * independently; a bad frame does not corrupt subsequent parsing.
 *
 * ── STREAMING / FRAGMENTATION ──────────────────────────────────────────────────
 *
 * The parser is INCREMENTAL: chunks may be split anywhere including inside an
 * escape sequence. Callers push raw string chunks via `push()` and read parsed
 * frames via the returned array or the registered callback. State is preserved
 * across push() calls.
 */

// ── OSC frame types ────────────────────────────────────────────────────────────

/** A fully parsed OSC 9 shell integration frame. */
export interface Osc9Frame {
    kind: "osc9";
    /** OSC 9 subcommand number (e.g. 1 = PreCmdExec, 2 = CmdStart, 3 = CmdEnd). */
    subcommand: number;
    /** Raw payload string after the subcommand (may be empty). */
    payload: string;
    /** Full raw parameter string (e.g. "9;2;/home/user"). */
    raw: string;
}

/** A fully parsed OSC 777 shell integration frame. */
export interface Osc777Frame {
    kind: "osc777";
    /** Action string (e.g. "notify", "preexec", "precmd", "postexec", "workdir"). */
    action: string;
    /** Raw payload string after the action (may be empty). */
    payload: string;
    /** Full raw parameter string (e.g. "777;notify;message"). */
    raw: string;
}

/** A fully parsed OSC 99 frame (future-proof, opaque payload). */
export interface Osc99Frame {
    kind: "osc99";
    /** Full raw parameter string (e.g. "99;payload"). */
    raw: string;
}

/** Union of all OSC frame types Fleet recovers from the read-stream. */
export type OscFrame = Osc9Frame | Osc777Frame | Osc99Frame;

// ── Constants ─────────────────────────────────────────────────────────────────

/** ESC character (0x1B). */
const ESC = "\x1B";
/** BEL character (0x07) — can terminate an OSC sequence. */
const BEL = "\x07";
/** 8-bit C1 OSC introducer (0x9D). */
const C1_OSC = "\x9D";
/** 8-bit String Terminator (0x9C). */
const C1_ST = "\x9C";

/** Maximum OSC parameter string length. Frames exceeding this are dropped. */
const MAX_OSC_PARAM_LEN = 4096;

// ── Parser state ──────────────────────────────────────────────────────────────

const enum State {
    /** Normal text passthrough — not inside any escape sequence. */
    Normal,
    /** Saw ESC (0x1B); waiting for the next byte. */
    Esc,
    /** Inside an OSC sequence: ESC ] … or 0x9D … */
    OscParam,
    /** Saw ESC while inside OscParam — may be ESC \ (ST). */
    OscEscInParam,
}

// ── Public API ────────────────────────────────────────────────────────────────

/**
 * Incremental, streaming OSC parser.
 *
 * Usage:
 *   const parser = new OscParser();
 *   parser.onFrame(frame => { ... });   // optional: register a callback
 *   parser.push(chunk1);                // feed raw terminal text
 *   parser.push(chunk2);
 *   parser.reset();                     // reset if the terminal session ends
 *
 * All parsing is synchronous; callbacks fire inline during `push()`.
 */
export class OscParser {
    private _state: State = State.Normal;
    /** Accumulates the raw OSC parameter bytes between the introducer and the ST. */
    private _paramBuf = "";
    /** Optional per-frame callback. */
    private _onFrame: ((frame: OscFrame) => void) | null = null;

    /**
     * Register a callback invoked for each fully-parsed OSC frame. Only one
     * callback is supported (overwriting replaces the previous one). Returns an
     * unsubscribe function.
     */
    onFrame(cb: (frame: OscFrame) => void): () => void {
        this._onFrame = cb;
        return () => {
            if (this._onFrame === cb) this._onFrame = null;
        };
    }

    /**
     * Feed a raw chunk of text from the terminal read-stream. The chunk may be
     * split anywhere — including in the middle of an escape sequence — and the
     * parser preserves state across calls. Any fully-terminated OSC sequences
     * inside the chunk are emitted immediately (or at the end of the chunk if the
     * terminator arrives).
     *
     * Returns an array of all OSC frames fully parsed from this chunk (in order).
     * Callers can use the return value OR the `onFrame` callback; both are fed.
     */
    push(chunk: string): OscFrame[] {
        const frames: OscFrame[] = [];

        for (let i = 0; i < chunk.length; i++) {
            const ch = chunk[i];

            switch (this._state) {
                case State.Normal:
                    if (ch === ESC) {
                        this._state = State.Esc;
                    } else if (ch === C1_OSC) {
                        // 8-bit C1 OSC introducer — enter OSC param directly.
                        this._paramBuf = "";
                        this._state = State.OscParam;
                    }
                    // All other characters are normal text — we don't buffer them.
                    break;

                case State.Esc:
                    if (ch === "]") {
                        // ESC ] — start of 7-bit OSC sequence.
                        this._paramBuf = "";
                        this._state = State.OscParam;
                    } else {
                        // Not an OSC introducer — abandon this escape and
                        // re-process ch as Normal (it might be another ESC).
                        this._state = State.Normal;
                        if (ch === ESC) {
                            this._state = State.Esc;
                        } else if (ch === C1_OSC) {
                            this._paramBuf = "";
                            this._state = State.OscParam;
                        }
                    }
                    break;

                case State.OscParam:
                    if (ch === BEL || ch === C1_ST) {
                        // BEL or 8-bit ST terminates the OSC.
                        const frame = _parseOscParam(this._paramBuf);
                        if (frame) {
                            frames.push(frame);
                            this._onFrame?.(frame);
                        }
                        this._paramBuf = "";
                        this._state = State.Normal;
                    } else if (ch === ESC) {
                        // Could be ESC \ (two-byte ST).
                        this._state = State.OscEscInParam;
                    } else {
                        // Accumulate — but guard against runaway sequences.
                        if (this._paramBuf.length < MAX_OSC_PARAM_LEN) {
                            this._paramBuf += ch;
                        } else {
                            // Overflow: silently drop this frame.
                            this._paramBuf = "";
                            this._state = State.Normal;
                        }
                    }
                    break;

                case State.OscEscInParam:
                    if (ch === "\\") {
                        // ESC \ = String Terminator.
                        const frame = _parseOscParam(this._paramBuf);
                        if (frame) {
                            frames.push(frame);
                            this._onFrame?.(frame);
                        }
                        this._paramBuf = "";
                        this._state = State.Normal;
                    } else {
                        // Not a valid ST — the ESC was spurious. Keep the ESC in
                        // the param (some OSC implementations do embed raw ESC) and
                        // re-process ch.
                        this._paramBuf += ESC;
                        this._state = State.OscParam;
                        // Re-process the current character without advancing i.
                        i--;
                    }
                    break;
            }
        }

        return frames;
    }

    /**
     * Reset all parser state. Call when the terminal session ends so stale
     * buffered sequences from the previous session cannot contaminate a new one.
     */
    reset(): void {
        this._state = State.Normal;
        this._paramBuf = "";
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/**
 * Parse the raw OSC parameter string (the content between the OSC introducer
 * and the String Terminator) into a typed `OscFrame`.
 *
 * Returns `null` for any frame whose OSC code is not 9, 777, or 99, and for
 * any frame whose format does not match our expected structure (malformed →
 * silently dropped rather than mis-stated).
 */
function _parseOscParam(param: string): OscFrame | null {
    // An OSC param starts with a numeric code followed by ; (or is just the code).
    const semi = param.indexOf(";");
    const codeStr = semi < 0 ? param : param.slice(0, semi);
    const code = parseInt(codeStr, 10);

    if (!Number.isFinite(code)) return null;

    // The rest of the param after the code (empty string if no semicolon).
    const rest = semi < 0 ? "" : param.slice(semi + 1);

    switch (code) {
        case 9:
            return _parseOsc9(rest, param);
        case 777:
            return _parseOsc777(rest, param);
        case 99:
            return { kind: "osc99", raw: param };
        default:
            return null; // Not a Fleet-relevant OSC code.
    }
}

/**
 * Parse the payload portion of an OSC 9 frame.
 *
 * Expected format: "<subcommand>" or "<subcommand>;<payload>"
 * E.g. "2" → { subcommand: 2, payload: "" }
 *      "3;0" → { subcommand: 3, payload: "0" }
 *
 * Returns null for malformed inputs (no valid subcommand number).
 */
function _parseOsc9(rest: string, raw: string): Osc9Frame | null {
    const semi = rest.indexOf(";");
    const subStr = semi < 0 ? rest : rest.slice(0, semi);
    const subcommand = parseInt(subStr, 10);

    if (!Number.isFinite(subcommand)) return null;

    const payload = semi < 0 ? "" : rest.slice(semi + 1);
    return { kind: "osc9", subcommand, payload, raw };
}

/**
 * Parse the payload portion of an OSC 777 frame.
 *
 * Expected format: "<action>" or "<action>;<payload>"
 * E.g. "notify;agent done" → { action: "notify", payload: "agent done" }
 *      "preexec" → { action: "preexec", payload: "" }
 *
 * Returns null if the action is empty (malformed).
 */
function _parseOsc777(rest: string, raw: string): Osc777Frame | null {
    if (!rest) return null; // OSC 777 with no action is malformed.

    const semi = rest.indexOf(";");
    const action = semi < 0 ? rest : rest.slice(0, semi);
    const payload = semi < 0 ? "" : rest.slice(semi + 1);

    if (!action) return null;

    return { kind: "osc777", action, payload, raw };
}
