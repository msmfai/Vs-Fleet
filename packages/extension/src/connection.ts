/**
 * Fleet Hub connection (EXTSKEL S8).
 *
 * The extension connects to the local Fleet Hub via:
 *   (a) Unix-socket fast path (`FLEET_HUB_SOCKET` env or `fleet.hubUnixSocket`
 *       config) on macOS/Linux — consistent with D7.
 *   (b) WebSocket fallback (`fleet.hubWsUrl`, default ws://127.0.0.1:51777).
 *
 * The connection is read/write:
 *   - On open, sends `subscribe` so the Hub begins streaming deltas.
 *   - Periodically sends a heartbeat `session.upsert` carrying the editor's
 *     window identity (FLEET_SESSION_ID injected by ENVINJ, S9).
 *   - Fires `onStatusChange` callbacks so the status bar reflects the state.
 *
 * Design constraints honored:
 *   - D6 — JSON wire format: all frames are JSON text.
 *   - D7 — WebSocket everywhere + unix fast path on unix.
 *   - D14 — NO proposed APIs, no enabledApiProposals.
 *   - Observer-not-owner: the extension only reports presence; it never drives
 *     an agent.
 *   - Reversibility: `dispose()` closes the socket cleanly.
 */

import * as net from "net";
import WebSocket from "ws";

// ── Connection status ─────────────────────────────────────────────────────────

export type ConnectionStatus =
    | "connecting"
    | "connected"
    | "disconnected"
    | "error";

export interface ConnectionStatusEvent {
    status: ConnectionStatus;
    /** Human-readable detail (error message, endpoint, …). */
    detail: string;
}

// ── HubConnection ─────────────────────────────────────────────────────────────

export type StatusChangeCallback = (event: ConnectionStatusEvent) => void;

/**
 * A live (or reconnecting) connection to the Fleet Hub.
 *
 * Consumers obtain one via `HubConnection.open()` and dispose it with
 * `dispose()` when the extension deactivates.
 */
export class HubConnection {
    private _status: ConnectionStatus = "connecting";
    private _detail = "";
    private _ws: WebSocket | null = null;
    private _unixSocket: net.Socket | null = null;
    private _heartbeatTimer: ReturnType<typeof setInterval> | null = null;
    private _listeners: StatusChangeCallback[] = [];
    private _disposed = false;
    private _sessionId: string;
    private _heartbeatIntervalMs: number;

    private constructor(sessionId: string, heartbeatIntervalMs: number) {
        this._sessionId = sessionId;
        this._heartbeatIntervalMs = heartbeatIntervalMs;
    }

    /** Current connection status. */
    get status(): ConnectionStatus {
        return this._status;
    }

    /** Human-readable detail. */
    get detail(): string {
        return this._detail;
    }

    /** Register a listener for status changes. Returns an unsubscribe function. */
    onStatusChange(cb: StatusChangeCallback): () => void {
        this._listeners.push(cb);
        return () => {
            this._listeners = this._listeners.filter(l => l !== cb);
        };
    }

    /** Send a raw JSON message to the Hub. No-op if not connected. */
    send(msg: object): void {
        const json = JSON.stringify(msg);
        if (this._ws && this._ws.readyState === WebSocket.OPEN) {
            this._ws.send(json);
        }
    }

    /** Close the connection and stop heartbeats. Idempotent. */
    dispose(): void {
        if (this._disposed) return;
        this._disposed = true;
        this._stopHeartbeat();
        if (this._ws) {
            try { this._ws.close(); } catch { /* best-effort */ }
            this._ws = null;
        }
        if (this._unixSocket) {
            try { this._unixSocket.destroy(); } catch { /* best-effort */ }
            this._unixSocket = null;
        }
        this._emit({ status: "disconnected", detail: "disposed" });
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    private _emit(ev: ConnectionStatusEvent): void {
        this._status = ev.status;
        this._detail = ev.detail;
        for (const l of this._listeners) {
            try { l(ev); } catch { /* listener errors must not kill the connection */ }
        }
    }

    private _startHeartbeat(): void {
        this._stopHeartbeat();
        if (this._disposed) return;
        this._heartbeatTimer = setInterval(() => {
            if (this._disposed) {
                this._stopHeartbeat();
                return;
            }
            // Heartbeat = a minimal session.upsert so the Hub knows the editor is
            // alive. Full session objects (with runs) are not the extension's
            // responsibility at this layer — that is ENVINJ/S9.
            this.send(heartbeatMessage(this._sessionId));
        }, this._heartbeatIntervalMs);
    }

    private _stopHeartbeat(): void {
        if (this._heartbeatTimer !== null) {
            clearInterval(this._heartbeatTimer);
            this._heartbeatTimer = null;
        }
    }

    // ── Factory ───────────────────────────────────────────────────────────────

    /**
     * Open a connection to the Hub.
     *
     * @param endpoint  Either a `ws://…` URL or `unix:/path/to/hub.sock`.
     * @param sessionId The editor window's Fleet session id.
     * @param heartbeatIntervalMs Heartbeat cadence.
     * @returns A HubConnection instance (connection attempt begins immediately).
     */
    static open(
        endpoint: string,
        sessionId: string,
        heartbeatIntervalMs: number
    ): HubConnection {
        const conn = new HubConnection(sessionId, heartbeatIntervalMs);
        if (endpoint.startsWith("unix:")) {
            conn._connectUnix(endpoint.slice("unix:".length));
        } else {
            conn._connectWs(endpoint);
        }
        return conn;
    }

    private _connectWs(url: string): void {
        this._emit({ status: "connecting", detail: url });
        let ws: WebSocket;
        try {
            ws = new WebSocket(url);
        } catch (err) {
            this._emit({
                status: "error",
                detail: `WebSocket construction failed: ${err}`,
            });
            return;
        }
        this._ws = ws;

        ws.on("open", () => {
            if (this._disposed) { ws.close(); return; }
            this._emit({ status: "connected", detail: url });
            // Subscribe so the Hub starts streaming deltas.
            this.send({ type: "subscribe" });
            this._startHeartbeat();
        });

        ws.on("message", (_data) => {
            // S8: the extension does not yet consume Hub events. Future slices
            // (ENVINJ S9, READSTREAM S18) will process the delta stream here.
        });

        ws.on("error", (err) => {
            if (this._disposed) return;
            this._emit({ status: "error", detail: String(err) });
        });

        ws.on("close", () => {
            if (this._disposed) return;
            this._stopHeartbeat();
            this._ws = null;
            this._emit({ status: "disconnected", detail: `closed: ${url}` });
        });
    }

    /**
     * Connect over a unix-domain socket using WebSocket framing (D7 fast path).
     *
     * This mirrors the Rust `UnixConnector` in `fleet-reporter/src/transport.rs`:
     * a TCP-less unix stream upgraded with a WS handshake so both transports
     * speak the same JSON-framed protocol.
     */
    private _connectUnix(socketPath: string): void {
        this._emit({ status: "connecting", detail: `unix:${socketPath}` });
        const socket = net.createConnection(socketPath);
        this._unixSocket = socket;

        socket.on("connect", () => {
            if (this._disposed) { socket.destroy(); return; }
            // Upgrade the raw unix socket to a WebSocket stream using the `ws`
            // package's duplex-stream constructor — mirrors the Rust approach.
            // The server (fleet-hub serve_ws_connection) expects a WS handshake.
            const ws = new WebSocket(`ws://localhost/`, {
                // eslint-disable-next-line @typescript-eslint/no-explicit-any
                createConnection: () => socket as any,
            });
            this._ws = ws;

            ws.on("open", () => {
                if (this._disposed) { ws.close(); return; }
                this._emit({ status: "connected", detail: `unix:${socketPath}` });
                this.send({ type: "subscribe" });
                this._startHeartbeat();
            });

            ws.on("message", (_data) => {
                // S8: no delta consumption yet.
            });

            ws.on("error", (err) => {
                if (this._disposed) return;
                this._emit({ status: "error", detail: String(err) });
            });

            ws.on("close", () => {
                if (this._disposed) return;
                this._stopHeartbeat();
                this._ws = null;
                this._unixSocket = null;
                this._emit({
                    status: "disconnected",
                    detail: `closed: unix:${socketPath}`,
                });
            });
        });

        socket.on("error", (err) => {
            if (this._disposed) return;
            this._unixSocket = null;
            this._emit({ status: "error", detail: String(err) });
        });
    }
}

// ── Wire helpers ──────────────────────────────────────────────────────────────

/**
 * Build a minimal heartbeat message. The extension sends this to the Hub to
 * signal that the editor window is alive. Full session objects with runs are
 * assembled by the reporter framework (ENVINJ S9); the S8 heartbeat is
 * intentionally minimal — a subscribe-then-heartbeat pattern that keeps the
 * hub connection alive without requiring the full reporter machinery.
 *
 * Wire shape mirrors ClientMessage::SessionUpsert (fleet-hub/src/wire.rs):
 * `{ "type": "session.upsert", "session": { ... } }`
 */
export function heartbeatMessage(sessionId: string): object {
    return {
        type: "session.upsert",
        session: {
            schema_version: 1,
            session_id: sessionId,
            title: "VS Code editor",
            location: {
                kind: "local",
                label: "editor",
                glyph: "laptop",
            },
            server: {
                kind: "local",
            },
            runs: [],
            rollup_state: "idle",
            rollup_urgency: null,
            muted: false,
            soloed: false,
            unread: false,
            tags: [],
            updated_at: new Date().toISOString(),
        },
    };
}

/**
 * Resolve the Hub endpoint from VS Code configuration + environment variables.
 *
 * Priority (highest first):
 * 1. `FLEET_HUB_SOCKET` env var — unix socket path.
 * 2. `fleet.hubUnixSocket` VS Code config — unix socket path.
 * 3. `FLEET_HUB_WS_URL` env var — WebSocket URL.
 * 4. `fleet.hubWsUrl` VS Code config — WebSocket URL (default ws://127.0.0.1:51777).
 */
export function resolveEndpoint(config: {
    get<T>(key: string, defaultValue: T): T;
}): string {
    // Env vars override VS Code config (shell environment → highest priority).
    const envSocket = process.env["FLEET_HUB_SOCKET"];
    if (envSocket) return `unix:${envSocket}`;

    const cfgSocket = config.get<string>("hubUnixSocket", "");
    if (cfgSocket) return `unix:${cfgSocket}`;

    const envWs = process.env["FLEET_HUB_WS_URL"];
    if (envWs) return envWs;

    return config.get<string>("hubWsUrl", "ws://127.0.0.1:51777");
}

/**
 * Resolve the Fleet session id for this editor window.
 *
 * The ENVINJ node (S9) will inject `FLEET_SESSION_ID` via
 * EnvironmentVariableCollection so every integrated-terminal shell inherits it.
 * At S8 we read it from the process environment if available, or generate a
 * stable-per-process UUID fallback.
 */
export function resolveSessionId(): string {
    const env = process.env["FLEET_SESSION_ID"];
    if (env) return env;
    // Deterministic per-process fallback: a combination of PID + startup time.
    // In production (S9+) this is replaced by the injected session id.
    return `vscode-ext-${process.pid}-${Date.now()}`;
}
