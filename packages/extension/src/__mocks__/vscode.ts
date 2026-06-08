/**
 * Minimal VS Code API mock for unit tests.
 *
 * Only the surface area used by EXTSKEL (S8) is mocked:
 * - window.createStatusBarItem
 * - workspace.getConfiguration
 * - commands.registerCommand
 * - ExtensionContext (subscriptions)
 * - StatusBarAlignment
 * - OutputChannel (window.createOutputChannel)
 * - Disposable
 *
 * NO proposed APIs are referenced here — consistent with D14/§3 invariant 4.
 */

import { EventEmitter } from "events";

// ── Disposable ────────────────────────────────────────────────────────────────

export class Disposable {
    readonly dispose: () => void;
    constructor(callOnDispose: () => void) {
        this.dispose = callOnDispose;
    }
    static from(...disposables: { dispose(): void }[]): Disposable {
        return new Disposable(() => disposables.forEach(d => d.dispose()));
    }
}

// ── StatusBarAlignment ────────────────────────────────────────────────────────

export enum StatusBarAlignment {
    Left = 1,
    Right = 2,
}

// ── StatusBarItem ─────────────────────────────────────────────────────────────

export interface StatusBarItem {
    text: string;
    tooltip: string | undefined;
    command: string | undefined;
    show(): void;
    hide(): void;
    dispose(): void;
}

function makeStatusBarItem(): StatusBarItem {
    return {
        text: "",
        tooltip: undefined,
        command: undefined,
        show: jest.fn(),
        hide: jest.fn(),
        dispose: jest.fn(),
    };
}

// ── OutputChannel ─────────────────────────────────────────────────────────────

export interface OutputChannel {
    appendLine(value: string): void;
    dispose(): void;
}

function makeOutputChannel(): OutputChannel {
    return {
        appendLine: jest.fn(),
        dispose: jest.fn(),
    };
}

// ── WorkspaceConfiguration ────────────────────────────────────────────────────

export interface WorkspaceConfiguration {
    get<T>(section: string): T | undefined;
    get<T>(section: string, defaultValue: T): T;
}

// Default configuration values matching the package.json contributes.
const defaultConfig: Record<string, unknown> = {
    "fleet.hubWsUrl": "ws://127.0.0.1:51777",
    "fleet.hubUnixSocket": "",
    "fleet.heartbeatIntervalMs": 10000,
};

let _configOverrides: Record<string, unknown> = {};

function makeWorkspaceConfiguration(section?: string): WorkspaceConfiguration {
    return {
        get<T>(key: string, defaultValue?: T): T | undefined {
            const fullKey = section ? `${section}.${key}` : key;
            // Check overrides first (most specific match wins):
            //   1. Full key with section (e.g. "fleet.hubUnixSocket")
            //   2. Short key without section (e.g. "hubUnixSocket")
            // Then fall back to built-in defaults, then the caller's defaultValue.
            if (fullKey in _configOverrides) {
                return _configOverrides[fullKey] as T;
            }
            if (key in _configOverrides) {
                return _configOverrides[key] as T;
            }
            if (fullKey in defaultConfig) {
                return defaultConfig[fullKey] as T;
            }
            if (key in defaultConfig) {
                return defaultConfig[key] as T;
            }
            return defaultValue;
        },
    };
}

// ── MarkdownString (only the constructor surface used as a description type) ──

export class MarkdownString {
    value: string;
    constructor(value = "") {
        this.value = value;
    }
}

// ── EnvironmentVariableMutatorType / mutator records ─────────────────────────

export enum EnvironmentVariableMutatorType {
    Replace = 1,
    Append = 2,
    Prepend = 3,
}

export interface EnvironmentVariableMutatorOptions {
    applyAtProcessCreation?: boolean;
    applyAtShellIntegration?: boolean;
}

export interface EnvironmentVariableMutator {
    readonly type: EnvironmentVariableMutatorType;
    readonly value: string;
    readonly options: EnvironmentVariableMutatorOptions;
}

/**
 * Faithful mock of `GlobalEnvironmentVariableCollection`.
 *
 * Models the documented invariants the ENVINJ code relies on:
 *   - "an extension can only make a single change to any one variable" — every
 *     replace/append/prepend OVERWRITES the prior mutator for that variable
 *     (one entry per variable, never duplicated).
 *   - `clear()` removes all mutators; `delete(v)` removes one.
 *   - `getScoped({ workspaceFolder })` returns an ISOLATED collection (a distinct
 *     mock), matching the real "does not impact objects for other scopes".
 *
 * The backing `Map` is exposed via test helpers so assertions can inspect what
 * was injected, what options were used, and that disposal emptied it.
 */
export class MockEnvironmentVariableCollection {
    persistent = true;
    description: string | MarkdownString | undefined = undefined;
    /** variable → mutator (one per variable, per the single-change rule). */
    readonly map = new Map<string, EnvironmentVariableMutator>();
    /** Records scopes requested via getScoped, for assertions. */
    readonly scopedCollections = new Map<string, MockEnvironmentVariableCollection>();
    /** Set true by clear(); lets tests assert clear() was actually called. */
    cleared = false;

    replace(variable: string, value: string, options: EnvironmentVariableMutatorOptions = {}): void {
        this.map.set(variable, {
            type: EnvironmentVariableMutatorType.Replace,
            value,
            options,
        });
    }

    append(variable: string, value: string, options: EnvironmentVariableMutatorOptions = {}): void {
        this.map.set(variable, {
            type: EnvironmentVariableMutatorType.Append,
            value,
            options,
        });
    }

    prepend(variable: string, value: string, options: EnvironmentVariableMutatorOptions = {}): void {
        this.map.set(variable, {
            type: EnvironmentVariableMutatorType.Prepend,
            value,
            options,
        });
    }

    get(variable: string): EnvironmentVariableMutator | undefined {
        return this.map.get(variable);
    }

    forEach(
        callback: (
            variable: string,
            mutator: EnvironmentVariableMutator,
            collection: MockEnvironmentVariableCollection
        ) => void
    ): void {
        this.map.forEach((mutator, variable) => callback(variable, mutator, this));
    }

    delete(variable: string): void {
        this.map.delete(variable);
    }

    clear(): void {
        this.map.clear();
        this.cleared = true;
    }

    getScoped(scope: { workspaceFolder?: { uri: { fsPath: string } } }): MockEnvironmentVariableCollection {
        const key = scope.workspaceFolder?.uri.fsPath ?? "__all__";
        let coll = this.scopedCollections.get(key);
        if (!coll) {
            coll = new MockEnvironmentVariableCollection();
            this.scopedCollections.set(key, coll);
        }
        return coll;
    }

    [Symbol.iterator](): Iterator<[string, EnvironmentVariableMutator]> {
        return this.map.entries();
    }
}

// ── ExtensionContext ──────────────────────────────────────────────────────────

export interface ExtensionContext {
    subscriptions: Array<{ dispose(): void }>;
    extensionUri: { fsPath: string };
    environmentVariableCollection: MockEnvironmentVariableCollection;
}

export function makeExtensionContext(): ExtensionContext {
    return {
        subscriptions: [],
        extensionUri: { fsPath: "/mock/extension" },
        environmentVariableCollection: new MockEnvironmentVariableCollection(),
    };
}

// ── Terminal shell execution (onDidStartTerminalShellExecution, stable ^1.93) ──

export interface TerminalShellExecution {
    /** The command line of the execution (informational). */
    commandLine?: { value: string };
    /** Returns an async iterable of raw terminal output chunks. */
    createStream(): AsyncIterable<string>;
}

export interface TerminalShellExecutionStartEvent {
    execution: TerminalShellExecution;
}

/**
 * Factory that returns a fresh `TerminalShellExecution` mock whose read-stream
 * is backed by the provided async iterator factory. Used in READSTREAM (S18)
 * tests to inject recorded OSC fixture data.
 */
export function makeTerminalShellExecution(
    chunks: string[]
): TerminalShellExecution {
    return {
        createStream(): AsyncIterable<string> {
            return {
                [Symbol.asyncIterator](): AsyncIterator<string> {
                    let idx = 0;
                    return {
                        next(): Promise<IteratorResult<string>> {
                            if (idx < chunks.length) {
                                return Promise.resolve({ value: chunks[idx++], done: false });
                            }
                            return Promise.resolve({ value: "", done: true });
                        },
                    };
                },
            };
        },
    };
}

/**
 * Creates a mock `window.onDidStartTerminalShellExecution` listener registry.
 * Listeners register via the returned `onDidStartTerminalShellExecution` mock.
 * Use `fire(event)` in tests to simulate a terminal shell execution starting.
 */
export function makeShellExecEventEmitter() {
    const listeners: Array<(e: TerminalShellExecutionStartEvent) => void> = [];

    function onDidStartTerminalShellExecution(
        listener: (e: TerminalShellExecutionStartEvent) => void
    ): Disposable {
        listeners.push(listener);
        return new Disposable(() => {
            const idx = listeners.indexOf(listener);
            if (idx >= 0) listeners.splice(idx, 1);
        });
    }

    function fire(event: TerminalShellExecutionStartEvent): void {
        for (const l of [...listeners]) {
            l(event);
        }
    }

    return { onDidStartTerminalShellExecution, fire };
}

// ── Top-level vscode namespace exports ───────────────────────────────────────

export const window = {
    createStatusBarItem: jest.fn(
        (_alignment?: StatusBarAlignment, _priority?: number): StatusBarItem =>
            makeStatusBarItem()
    ),
    createOutputChannel: jest.fn((_name: string): OutputChannel => makeOutputChannel()),
    showInformationMessage: jest.fn((_msg: string) => Promise.resolve()),
    showErrorMessage: jest.fn((_msg: string) => Promise.resolve()),
    showWarningMessage: jest.fn((_msg: string) => Promise.resolve()),
    onDidStartTerminalShellExecution: jest.fn(
        (_listener: (e: TerminalShellExecutionStartEvent) => void): Disposable =>
            new Disposable(() => {})
    ),
};

export const workspace = {
    getConfiguration: jest.fn(
        (section?: string): WorkspaceConfiguration => makeWorkspaceConfiguration(section)
    ),
};

export const commands = {
    registerCommand: jest.fn(
        (_command: string, _callback: (...args: unknown[]) => unknown): Disposable =>
            new Disposable(() => {})
    ),
};

// ── Test helpers (not part of the real vscode API) ───────────────────────────

/**
 * Override a configuration value for tests. Call `resetMockConfig()` in
 * `afterEach` to prevent cross-test contamination.
 */
export function setMockConfig(key: string, value: unknown): void {
    _configOverrides[key] = value;
}

/**
 * Reset all configuration overrides to defaults.
 */
export function resetMockConfig(): void {
    _configOverrides = {};
}

/**
 * Reset all jest.fn() call histories on the window/workspace/commands mocks.
 */
export function resetAllMocks(): void {
    jest.clearAllMocks();
    _configOverrides = {};
    // Re-bind the createStatusBarItem implementation after clearAllMocks wipes it.
    (window.createStatusBarItem as jest.Mock).mockImplementation(
        (_alignment?: StatusBarAlignment, _priority?: number): StatusBarItem =>
            makeStatusBarItem()
    );
    (window.createOutputChannel as jest.Mock).mockImplementation(
        (_name: string): OutputChannel => makeOutputChannel()
    );
    (workspace.getConfiguration as jest.Mock).mockImplementation(
        (section?: string): WorkspaceConfiguration => makeWorkspaceConfiguration(section)
    );
    (commands.registerCommand as jest.Mock).mockImplementation(
        (_command: string, _callback: (...args: unknown[]) => unknown): Disposable =>
            new Disposable(() => {})
    );
    (window.onDidStartTerminalShellExecution as jest.Mock).mockImplementation(
        (_listener: (e: TerminalShellExecutionStartEvent) => void): Disposable =>
            new Disposable(() => {})
    );
}
