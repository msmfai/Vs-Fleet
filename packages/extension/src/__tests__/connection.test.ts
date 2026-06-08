/**
 * Unit tests for HubConnection and helpers (S8 EXTSKEL).
 *
 * All tests run WITHOUT a real VS Code runtime or a real Hub socket.
 * Network I/O is intercepted by mocking the `ws` module and Node's `net` module
 * so tests are deterministic, fast, and offline.
 */

import {
    HubConnection,
    heartbeatMessage,
    resolveEndpoint,
    resolveSessionId,
    ConnectionStatus,
} from "../connection";

// ── Mock WebSocket (ws npm package) ──────────────────────────────────────────

// We intercept `ws` before any connection is opened so we can script outcomes.
jest.mock("ws");

import WebSocket from "ws";
const MockWS = WebSocket as jest.MockedClass<typeof WebSocket>;

/**
 * Make a controllable fake WebSocket instance. The test can imperatively fire
 * the standard events (`open`, `error`, `close`, `message`) to drive the
 * connection state machine.
 */
function makeFakeWS() {
    const listeners: Record<string, Function[]> = {};
    const sent: string[] = [];

    const fake = {
        readyState: WebSocket.CONNECTING as number,
        on: jest.fn((event: string, cb: Function) => {
            (listeners[event] = listeners[event] || []).push(cb);
        }),
        send: jest.fn((data: string) => { sent.push(data); }),
        close: jest.fn(() => {
            fake.readyState = WebSocket.CLOSED;
        }),
        // Test helpers
        _emit(event: string, ...args: unknown[]) {
            (listeners[event] || []).forEach(cb => cb(...args));
        },
        _sent: sent,
    };
    return fake;
}

// ── Mock net module ───────────────────────────────────────────────────────────

jest.mock("net");

import * as net from "net";
const MockNet = net as jest.Mocked<typeof net>;

function makeFakeSocket() {
    const listeners: Record<string, Function[]> = {};
    const fake = {
        on: jest.fn((event: string, cb: Function) => {
            (listeners[event] = listeners[event] || []).push(cb);
        }),
        destroy: jest.fn(),
        _emit(event: string, ...args: unknown[]) {
            (listeners[event] || []).forEach(cb => cb(...args));
        },
    };
    return fake;
}

// Use fake timers to prevent real setInterval calls from leaking into Jest's
// open-handle tracker. All connection tests complete synchronously after
// faking timers; the heartbeat timer is just a setInterval that never fires
// in these unit tests (we control the clock).
beforeEach(() => jest.useFakeTimers());
afterEach(() => jest.useRealTimers());

// ── Helpers ───────────────────────────────────────────────────────────────────

function makeWsConn(endpoint = "ws://127.0.0.1:51777") {
    const fakeWs = makeFakeWS();
    MockWS.mockImplementationOnce(() => fakeWs as unknown as WebSocket);
    const conn = HubConnection.open(endpoint, "test-session", 10_000);
    return { conn, fakeWs };
}

// ── heartbeatMessage ──────────────────────────────────────────────────────────

describe("heartbeatMessage", () => {
    it("has the correct wire type (session.upsert)", () => {
        const msg = heartbeatMessage("sess-1") as Record<string, unknown>;
        expect(msg.type).toBe("session.upsert");
    });

    it("embeds the session_id", () => {
        const msg = heartbeatMessage("my-session-xyz") as {
            session: { session_id: string };
        };
        expect(msg.session.session_id).toBe("my-session-xyz");
    });

    it("has schema_version 1", () => {
        const msg = heartbeatMessage("s") as {
            session: { schema_version: number };
        };
        expect(msg.session.schema_version).toBe(1);
    });

    it("has rollup_state idle (no active runs at extension level)", () => {
        const msg = heartbeatMessage("s") as {
            session: { rollup_state: string };
        };
        expect(msg.session.rollup_state).toBe("idle");
    });

    it("serializes to valid JSON", () => {
        expect(() => JSON.stringify(heartbeatMessage("s"))).not.toThrow();
    });
});

// ── resolveEndpoint ───────────────────────────────────────────────────────────

describe("resolveEndpoint", () => {
    const savedEnv: Record<string, string | undefined> = {};
    beforeEach(() => {
        savedEnv.FLEET_HUB_SOCKET = process.env["FLEET_HUB_SOCKET"];
        savedEnv.FLEET_HUB_WS_URL = process.env["FLEET_HUB_WS_URL"];
        delete process.env["FLEET_HUB_SOCKET"];
        delete process.env["FLEET_HUB_WS_URL"];
    });
    afterEach(() => {
        if (savedEnv.FLEET_HUB_SOCKET === undefined) {
            delete process.env["FLEET_HUB_SOCKET"];
        } else {
            process.env["FLEET_HUB_SOCKET"] = savedEnv.FLEET_HUB_SOCKET;
        }
        if (savedEnv.FLEET_HUB_WS_URL === undefined) {
            delete process.env["FLEET_HUB_WS_URL"];
        } else {
            process.env["FLEET_HUB_WS_URL"] = savedEnv.FLEET_HUB_WS_URL;
        }
    });

    function makeConfig(overrides: Record<string, string> = {}) {
        const defaults: Record<string, string> = {
            "hubWsUrl": "ws://127.0.0.1:51777",
            "hubUnixSocket": "",
        };
        return {
            get<T>(key: string, defaultValue: T): T {
                if (key in overrides) return overrides[key] as unknown as T;
                if (key in defaults) return defaults[key] as unknown as T;
                return defaultValue;
            },
        };
    }

    it("uses the default WS URL when no env and no config override", () => {
        expect(resolveEndpoint(makeConfig())).toBe("ws://127.0.0.1:51777");
    });

    it("prefers FLEET_HUB_SOCKET env over everything", () => {
        process.env["FLEET_HUB_SOCKET"] = "/tmp/hub.sock";
        expect(resolveEndpoint(makeConfig())).toBe("unix:/tmp/hub.sock");
    });

    it("uses fleet.hubUnixSocket config when no FLEET_HUB_SOCKET env", () => {
        expect(resolveEndpoint(makeConfig({ hubUnixSocket: "/var/run/fleet.sock" }))).toBe(
            "unix:/var/run/fleet.sock"
        );
    });

    it("uses FLEET_HUB_WS_URL env when no socket override", () => {
        process.env["FLEET_HUB_WS_URL"] = "ws://10.0.0.1:9000";
        expect(resolveEndpoint(makeConfig())).toBe("ws://10.0.0.1:9000");
    });

    it("uses fleet.hubWsUrl config over the hardcoded default", () => {
        expect(
            resolveEndpoint(makeConfig({ hubWsUrl: "ws://127.0.0.1:12345" }))
        ).toBe("ws://127.0.0.1:12345");
    });

    it("FLEET_HUB_SOCKET takes priority over fleet.hubUnixSocket config", () => {
        process.env["FLEET_HUB_SOCKET"] = "/env.sock";
        expect(
            resolveEndpoint(makeConfig({ hubUnixSocket: "/cfg.sock" }))
        ).toBe("unix:/env.sock");
    });
});

// ── resolveSessionId ─────────────────────────────────────────────────────────

describe("resolveSessionId", () => {
    const savedEnv = process.env["FLEET_SESSION_ID"];
    afterEach(() => {
        if (savedEnv === undefined) {
            delete process.env["FLEET_SESSION_ID"];
        } else {
            process.env["FLEET_SESSION_ID"] = savedEnv;
        }
    });

    it("returns FLEET_SESSION_ID env when present", () => {
        process.env["FLEET_SESSION_ID"] = "injected-session-xyz";
        expect(resolveSessionId()).toBe("injected-session-xyz");
    });

    it("generates a pid-scoped fallback when env is absent", () => {
        delete process.env["FLEET_SESSION_ID"];
        const id = resolveSessionId();
        expect(id).toMatch(/^vscode-ext-\d+-\d+$/);
        expect(id).toContain(`${process.pid}`);
    });

    it("fallback is non-empty", () => {
        delete process.env["FLEET_SESSION_ID"];
        expect(resolveSessionId().length).toBeGreaterThan(0);
    });
});

// ── HubConnection (WebSocket path) ───────────────────────────────────────────

describe("HubConnection WebSocket", () => {
    beforeEach(() => {
        jest.clearAllMocks();
    });

    it("starts in connecting state", () => {
        const { conn, fakeWs } = makeWsConn();
        expect(conn.status).toBe("connecting");
        conn.dispose();
        void fakeWs; // suppress unused warning
    });

    it("transitions to connected when the socket opens", () => {
        const { conn, fakeWs } = makeWsConn();
        fakeWs.readyState = WebSocket.OPEN;

        const statuses: ConnectionStatus[] = [];
        conn.onStatusChange(ev => statuses.push(ev.status));

        fakeWs._emit("open");

        expect(statuses).toContain("connected");
        conn.dispose();
    });

    it("emits status: error on socket error", () => {
        const { conn, fakeWs } = makeWsConn();

        const statuses: ConnectionStatus[] = [];
        conn.onStatusChange(ev => statuses.push(ev.status));

        fakeWs._emit("error", new Error("ECONNREFUSED"));

        expect(statuses).toContain("error");
        conn.dispose();
    });

    it("emits status: disconnected on socket close", () => {
        const { conn, fakeWs } = makeWsConn();
        fakeWs.readyState = WebSocket.OPEN;

        const statuses: ConnectionStatus[] = [];
        conn.onStatusChange(ev => statuses.push(ev.status));

        fakeWs._emit("open");
        fakeWs._emit("close");

        expect(statuses).toContain("disconnected");
        conn.dispose();
    });

    it("sends subscribe on open", () => {
        const { conn, fakeWs } = makeWsConn();
        fakeWs.readyState = WebSocket.OPEN;

        fakeWs._emit("open");

        expect(fakeWs._sent.length).toBeGreaterThan(0);
        const msg = JSON.parse(fakeWs._sent[0]);
        expect(msg.type).toBe("subscribe");
        conn.dispose();
    });

    it("dispose() closes the socket cleanly", () => {
        const { conn, fakeWs } = makeWsConn();
        conn.dispose();
        expect(fakeWs.close).toHaveBeenCalled();
    });

    it("dispose() is idempotent (no double-close throws)", () => {
        const { conn } = makeWsConn();
        expect(() => {
            conn.dispose();
            conn.dispose();
            conn.dispose();
        }).not.toThrow();
    });

    it("onStatusChange unsubscribe removes the listener", () => {
        const { conn, fakeWs } = makeWsConn();
        fakeWs.readyState = WebSocket.OPEN;

        const calls: ConnectionStatus[] = [];
        const unsub = conn.onStatusChange(ev => calls.push(ev.status));
        unsub(); // unsubscribe before the event fires
        fakeWs._emit("open");

        expect(calls).toHaveLength(0);
        conn.dispose();
    });

    it("status detail carries the endpoint URL", () => {
        const url = "ws://127.0.0.1:51777";
        const { conn, fakeWs } = makeWsConn(url);
        const details: string[] = [];
        conn.onStatusChange(ev => details.push(ev.detail));
        fakeWs.readyState = WebSocket.OPEN;
        fakeWs._emit("open");
        expect(details.some(d => d.includes("51777"))).toBe(true);
        conn.dispose();
    });

    it("send() delivers JSON to the socket when connected", () => {
        const { conn, fakeWs } = makeWsConn();
        fakeWs.readyState = WebSocket.OPEN;
        fakeWs._emit("open");

        conn.send({ type: "command", command: "mute", session_id: "s1" });

        const last = fakeWs._sent[fakeWs._sent.length - 1];
        const msg = JSON.parse(last);
        expect(msg.type).toBe("command");
    });

    it("send() is no-op when socket is not OPEN", () => {
        const { conn, fakeWs } = makeWsConn();
        // readyState is still CONNECTING (not OPEN)
        fakeWs.readyState = WebSocket.CONNECTING;
        conn.send({ type: "anything" });
        // subscribe has NOT fired yet, so sent array is empty (no open event)
        const nonSubscribeSends = fakeWs._sent.filter(s => {
            try { return JSON.parse(s).type !== "subscribe"; }
            catch { return true; }
        });
        expect(nonSubscribeSends).toHaveLength(0);
        conn.dispose();
    });

    it("status changes after dispose are not emitted", () => {
        const { conn, fakeWs } = makeWsConn();
        fakeWs.readyState = WebSocket.OPEN;
        fakeWs._emit("open");

        const afterDispose: ConnectionStatus[] = [];
        conn.dispose();
        conn.onStatusChange(ev => afterDispose.push(ev.status));

        // Fire close AFTER dispose; listener should not fire.
        fakeWs._emit("close");
        expect(afterDispose).toHaveLength(0);
    });
});

// ── HubConnection (unix socket path) ─────────────────────────────────────────

describe("HubConnection unix socket", () => {
    beforeEach(() => {
        jest.clearAllMocks();
    });

    it("starts in connecting state for unix endpoint", () => {
        const fakeSocket = makeFakeSocket();
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        (MockNet.createConnection as jest.Mock).mockReturnValueOnce(fakeSocket as any);

        const fakeWs = makeFakeWS();
        MockWS.mockImplementationOnce(() => fakeWs as unknown as WebSocket);

        const conn = HubConnection.open("unix:/tmp/hub.sock", "sess", 10_000);
        expect(conn.status).toBe("connecting");
        conn.dispose();
    });

    it("emits error when the unix socket errors before WS upgrade", () => {
        const fakeSocket = makeFakeSocket();
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        (MockNet.createConnection as jest.Mock).mockReturnValueOnce(fakeSocket as any);

        const conn = HubConnection.open("unix:/tmp/hub.sock", "sess", 10_000);
        const statuses: ConnectionStatus[] = [];
        conn.onStatusChange(ev => statuses.push(ev.status));

        fakeSocket._emit("error", new Error("ENOENT"));
        expect(statuses).toContain("error");
        conn.dispose();
    });

    it("dispose() destroys the unix socket", () => {
        const fakeSocket = makeFakeSocket();
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        (MockNet.createConnection as jest.Mock).mockReturnValueOnce(fakeSocket as any);

        const conn = HubConnection.open("unix:/tmp/hub.sock", "sess", 10_000);
        conn.dispose();
        expect(fakeSocket.destroy).toHaveBeenCalled();
    });
});
