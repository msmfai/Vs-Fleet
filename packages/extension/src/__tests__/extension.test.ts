/**
 * Unit tests for the extension activation logic (S8 EXTSKEL).
 *
 * The vscode module is replaced by the mock at src/__mocks__/vscode.ts (wired
 * up via jest.moduleNameMapper in package.json). We spy on HubConnection.open
 * so we can inspect its construction arguments and control the returned
 * connection object, without opening any real socket.
 */

import * as vscode from "vscode";
import {
    makeExtensionContext,
    resetAllMocks,
    setMockConfig,
} from "../__mocks__/vscode";

// Import connection module so we can spy on HubConnection.open.
import * as connectionModule from "../connection";
import { HubConnection } from "../connection";

// Import module under test.
import { activate, deactivate, getConnection, getEnvInjector } from "../extension";
import { FLEET_SESSION_ID_VAR, FLEET_HUB_SOCKET_VAR, FLEET_HUB_WS_URL_VAR } from "../envInject";

// ── Helpers ───────────────────────────────────────────────────────────────────

/** Minimal HubConnection fake — records open args, exposes event hooks. */
function makeFakeConnection() {
    const _listeners: Array<(ev: { status: string; detail: string }) => void> = [];
    let _status = "connecting";
    let _detail = "ws://127.0.0.1:51777";
    let _disposed = false;

    const fake = {
        get status() { return _status; },
        get detail() { return _detail; },
        onStatusChange: jest.fn((cb: (ev: { status: string; detail: string }) => void) => {
            _listeners.push(cb);
            return () => { /* unsubscribe */ };
        }),
        send: jest.fn(),
        dispose: jest.fn(() => { _disposed = true; }),
        // Test helpers
        _simulateStatus(status: string, detail = "") {
            _status = status;
            _detail = detail;
            _listeners.forEach(cb => cb({ status, detail }));
        },
        get _isDisposed() { return _disposed; },
    };
    return fake;
}

// ── Setup / teardown ──────────────────────────────────────────────────────────

let fakeConn: ReturnType<typeof makeFakeConnection>;
let openSpy: jest.SpyInstance;

beforeEach(() => {
    resetAllMocks();
    fakeConn = makeFakeConnection();
    // Spy on the static HubConnection.open without replacing the whole module.
    openSpy = jest.spyOn(HubConnection, "open").mockImplementation(
        () => fakeConn as unknown as HubConnection
    );
});

afterEach(() => {
    // Clean up any live connection.
    deactivate();
    openSpy.mockRestore();
    resetAllMocks();
});

// ── activate() ────────────────────────────────────────────────────────────────

describe("activate()", () => {
    it("calls HubConnection.open with the default WS endpoint", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        expect(openSpy).toHaveBeenCalledTimes(1);
        const [endpoint] = openSpy.mock.calls[0];
        expect(endpoint).toBe("ws://127.0.0.1:51777");
    });

    it("passes a non-empty session id to HubConnection.open", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        const [_endpoint, sessionId] = openSpy.mock.calls[0];
        expect(typeof sessionId).toBe("string");
        expect(sessionId.length).toBeGreaterThan(0);
    });

    it("uses FLEET_SESSION_ID env when set", () => {
        const saved = process.env["FLEET_SESSION_ID"];
        process.env["FLEET_SESSION_ID"] = "injected-abc";
        try {
            const ctx = makeExtensionContext();
            activate(ctx as unknown as vscode.ExtensionContext);
            const [_endpoint, sessionId] = openSpy.mock.calls[0];
            expect(sessionId).toBe("injected-abc");
        } finally {
            if (saved === undefined) delete process.env["FLEET_SESSION_ID"];
            else process.env["FLEET_SESSION_ID"] = saved;
        }
    });

    it("uses the configured unix socket endpoint when set", () => {
        setMockConfig("hubUnixSocket", "/var/run/fleet.sock");
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        const [endpoint] = openSpy.mock.calls[0];
        expect(endpoint).toBe("unix:/var/run/fleet.sock");
    });

    it("uses a custom WS URL from config", () => {
        setMockConfig("hubWsUrl", "ws://127.0.0.1:9999");
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        const [endpoint] = openSpy.mock.calls[0];
        expect(endpoint).toBe("ws://127.0.0.1:9999");
    });

    it("passes the configured heartbeat interval to HubConnection.open", () => {
        setMockConfig("heartbeatIntervalMs", 5000);
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        const [_endpoint, _sid, heartbeatMs] = openSpy.mock.calls[0];
        expect(heartbeatMs).toBe(5000);
    });

    it("creates a status bar item", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        expect(vscode.window.createStatusBarItem).toHaveBeenCalledTimes(1);
    });

    it("registers the fleet.showStatus command", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        expect(vscode.commands.registerCommand).toHaveBeenCalledWith(
            "fleet.showStatus",
            expect.any(Function)
        );
    });

    it("pushes disposables into context.subscriptions", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        // At minimum: the status bar item + the command disposable +
        // the connection cleanup disposable.
        expect(ctx.subscriptions.length).toBeGreaterThanOrEqual(2);
    });

    it("exposes the connection via getConnection()", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);
        expect(getConnection()).not.toBeNull();
    });

    it("FLEET_HUB_SOCKET env takes priority over config", () => {
        const saved = process.env["FLEET_HUB_SOCKET"];
        process.env["FLEET_HUB_SOCKET"] = "/env/hub.sock";
        setMockConfig("hubUnixSocket", "/cfg/hub.sock");
        try {
            const ctx = makeExtensionContext();
            activate(ctx as unknown as vscode.ExtensionContext);
            const [endpoint] = openSpy.mock.calls[0];
            expect(endpoint).toBe("unix:/env/hub.sock");
        } finally {
            if (saved === undefined) delete process.env["FLEET_HUB_SOCKET"];
            else process.env["FLEET_HUB_SOCKET"] = saved;
        }
    });
});

// ── deactivate() ─────────────────────────────────────────────────────────────

describe("deactivate()", () => {
    it("disposes the connection", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        deactivate();

        expect(fakeConn.dispose).toHaveBeenCalled();
    });

    it("nulls the connection reference (getConnection returns null)", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);
        deactivate();

        expect(getConnection()).toBeNull();
    });

    it("is safe to call before activate", () => {
        // Must not throw even if called with no active connection.
        expect(() => deactivate()).not.toThrow();
    });

    it("is idempotent (double-deactivate does not throw)", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);
        expect(() => {
            deactivate();
            deactivate();
        }).not.toThrow();
    });
});

// ── Status bar update from connection events ──────────────────────────────────

describe("status bar reflects connection state", () => {
    function makeBarItem() {
        return {
            text: "",
            tooltip: "",
            command: undefined as string | undefined,
            show: jest.fn(),
            hide: jest.fn(),
            dispose: jest.fn(),
        };
    }

    it("status bar text updates when connection becomes connected", () => {
        const ctx = makeExtensionContext();
        const barItem = makeBarItem();
        (vscode.window.createStatusBarItem as jest.Mock).mockReturnValueOnce(barItem);

        activate(ctx as unknown as vscode.ExtensionContext);

        // Simulate the connection becoming connected.
        fakeConn._simulateStatus("connected", "ws://127.0.0.1:51777");

        expect(barItem.text).toContain("Fleet");
        expect(barItem.text).toContain("circle-filled");
    });

    it("status bar shows warning icon on error", () => {
        const ctx = makeExtensionContext();
        const barItem = makeBarItem();
        (vscode.window.createStatusBarItem as jest.Mock).mockReturnValueOnce(barItem);

        activate(ctx as unknown as vscode.ExtensionContext);

        fakeConn._simulateStatus("error", "ECONNREFUSED");

        expect(barItem.text).toContain("warning");
    });

    it("status bar shows disconnected icon when socket closes", () => {
        const ctx = makeExtensionContext();
        const barItem = makeBarItem();
        (vscode.window.createStatusBarItem as jest.Mock).mockReturnValueOnce(barItem);

        activate(ctx as unknown as vscode.ExtensionContext);

        fakeConn._simulateStatus("disconnected", "closed");

        expect(barItem.text).toContain("circle-outline");
    });

    it("status bar tooltip includes connection detail", () => {
        const ctx = makeExtensionContext();
        const barItem = makeBarItem();
        (vscode.window.createStatusBarItem as jest.Mock).mockReturnValueOnce(barItem);

        activate(ctx as unknown as vscode.ExtensionContext);

        fakeConn._simulateStatus("error", "ECONNREFUSED 127.0.0.1:51777");

        expect(barItem.tooltip).toContain("ECONNREFUSED");
    });
});

// ── ENVINJ (S9): activate injects, deactivate clears ──────────────────────────

describe("env injection on activate/deactivate", () => {
    it("injects FLEET_SESSION_ID into the environmentVariableCollection on activate", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        const m = ctx.environmentVariableCollection.get(FLEET_SESSION_ID_VAR);
        expect(m).toBeDefined();
        expect(m!.value.length).toBeGreaterThan(0);
    });

    it("injects the FLEET_SESSION_ID matching the one passed to HubConnection.open", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        const [, sessionId] = openSpy.mock.calls[0];
        expect(ctx.environmentVariableCollection.get(FLEET_SESSION_ID_VAR)!.value).toBe(sessionId);
    });

    it("injects FLEET_HUB_WS_URL for the default WS endpoint (no socket var)", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        expect(ctx.environmentVariableCollection.get(FLEET_HUB_WS_URL_VAR)!.value).toBe(
            "ws://127.0.0.1:51777"
        );
        expect(ctx.environmentVariableCollection.get(FLEET_HUB_SOCKET_VAR)).toBeUndefined();
    });

    it("injects FLEET_HUB_SOCKET for a unix endpoint (no ws var)", () => {
        setMockConfig("hubUnixSocket", "/var/run/fleet.sock");
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        expect(ctx.environmentVariableCollection.get(FLEET_HUB_SOCKET_VAR)!.value).toBe(
            "/var/run/fleet.sock"
        );
        expect(ctx.environmentVariableCollection.get(FLEET_HUB_WS_URL_VAR)).toBeUndefined();
    });

    it("exposes the injector via getEnvInjector() after activate", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);
        expect(getEnvInjector()).not.toBeNull();
        expect(getEnvInjector()!.injected).toBe(true);
    });

    it("clears all injected env vars on deactivate (reversibility)", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);
        expect(ctx.environmentVariableCollection.map.size).toBeGreaterThan(0);

        deactivate();

        expect(ctx.environmentVariableCollection.map.size).toBe(0);
        expect(ctx.environmentVariableCollection.cleared).toBe(true);
        expect(getEnvInjector()).toBeNull();
    });

    it("clears injected env vars when the subscription disposer runs (uninstall/reload path)", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);
        expect(ctx.environmentVariableCollection.map.size).toBeGreaterThan(0);

        // Simulate VS Code disposing the registered subscriptions (deactivation).
        for (const sub of ctx.subscriptions) sub.dispose();

        expect(ctx.environmentVariableCollection.map.size).toBe(0);
    });
});

// ── fleet.showStatus command ──────────────────────────────────────────────────

describe("fleet.showStatus command", () => {
    it("calls showInformationMessage with the connection status", () => {
        const ctx = makeExtensionContext();
        activate(ctx as unknown as vscode.ExtensionContext);

        // Grab the registered callback.
        const registerCalls = (vscode.commands.registerCommand as jest.Mock).mock.calls;
        const showStatusCall = registerCalls.find(
            ([cmd]: [string]) => cmd === "fleet.showStatus"
        );
        expect(showStatusCall).toBeDefined();

        const callback = showStatusCall![1];
        callback();

        expect(vscode.window.showInformationMessage).toHaveBeenCalledTimes(1);
        const msg = (vscode.window.showInformationMessage as jest.Mock).mock
            .calls[0][0] as string;
        expect(msg).toMatch(/Fleet Hub/i);
    });
});

// ── Unused import suppression ─────────────────────────────────────────────────
// connectionModule is imported so the spy is set up on the real module instance.
void connectionModule;
