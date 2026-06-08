/**
 * Unit tests for ENVINJ (S9) — env injection via EnvironmentVariableCollection.
 *
 * The vscode module is replaced by the mock at src/__mocks__/vscode.ts. We
 * drive `EnvInjector` against the faithful `MockEnvironmentVariableCollection`,
 * which models the documented API invariants (single-change-per-variable,
 * clear/delete, getScoped isolation).
 *
 * These tests assert the build-relevant contract:
 *   - the RIGHT vars (FLEET_SESSION_ID + exactly one reporter endpoint) are
 *     injected with the RIGHT options (applyAtProcessCreation),
 *   - injection is idempotent (no accumulation),
 *   - endpoint unix↔ws switching never leaves a stale endpoint var,
 *   - dispose() is fully reversible and idempotent (invariant 6),
 *   - persistence + description are set (reload survival + UI legibility).
 */

import {
    MockEnvironmentVariableCollection,
    MarkdownString,
    EnvironmentVariableMutatorType,
} from "../__mocks__/vscode";
import {
    EnvInjector,
    endpointToTargets,
    FLEET_SESSION_ID_VAR,
    FLEET_HUB_SOCKET_VAR,
    FLEET_HUB_WS_URL_VAR,
    FLEET_ENV_VARS,
} from "../envInject";

// ── endpointToTargets() (pure) ────────────────────────────────────────────────

describe("endpointToTargets()", () => {
    it("maps a unix: endpoint to a hubSocketPath", () => {
        expect(endpointToTargets("unix:/var/run/fleet.sock")).toEqual({
            hubSocketPath: "/var/run/fleet.sock",
        });
    });

    it("maps a ws:// endpoint to a hubWsUrl", () => {
        expect(endpointToTargets("ws://127.0.0.1:51777")).toEqual({
            hubWsUrl: "ws://127.0.0.1:51777",
        });
    });

    it("treats a non-unix endpoint as a ws url (no socket path leaks)", () => {
        const t = endpointToTargets("ws://example:9999");
        expect(t.hubSocketPath).toBeUndefined();
        expect(t.hubWsUrl).toBe("ws://example:9999");
    });
});

// ── construction ──────────────────────────────────────────────────────────────

describe("EnvInjector construction", () => {
    it("marks the collection persistent (survives window reload)", () => {
        const coll = new MockEnvironmentVariableCollection();
        coll.persistent = false; // simulate a non-default starting state
        new EnvInjector(coll);
        expect(coll.persistent).toBe(true);
    });

    it("sets a human-readable description for the env-changes UI", () => {
        const coll = new MockEnvironmentVariableCollection();
        new EnvInjector(coll);
        expect(typeof coll.description === "string" || coll.description instanceof MarkdownString).toBe(true);
        expect(String(coll.description)).toContain("Fleet");
    });

    it("starts un-injected", () => {
        const inj = new EnvInjector(new MockEnvironmentVariableCollection());
        expect(inj.injected).toBe(false);
    });
});

// ── inject(): session id ──────────────────────────────────────────────────────

describe("inject() — FLEET_SESSION_ID", () => {
    it("injects FLEET_SESSION_ID via replace with the given value", () => {
        const coll = new MockEnvironmentVariableCollection();
        new EnvInjector(coll).inject({ sessionId: "win-42", hubWsUrl: "ws://h" });

        const m = coll.get(FLEET_SESSION_ID_VAR);
        expect(m).toBeDefined();
        expect(m!.value).toBe("win-42");
        expect(m!.type).toBe(EnvironmentVariableMutatorType.Replace);
    });

    it("injects with applyAtProcessCreation:true (works without shell integration)", () => {
        const coll = new MockEnvironmentVariableCollection();
        new EnvInjector(coll).inject({ sessionId: "win-1", hubWsUrl: "ws://h" });
        expect(coll.get(FLEET_SESSION_ID_VAR)!.options.applyAtProcessCreation).toBe(true);
    });

    it("flips injected to true", () => {
        const inj = new EnvInjector(new MockEnvironmentVariableCollection());
        inj.inject({ sessionId: "x", hubWsUrl: "ws://h" });
        expect(inj.injected).toBe(true);
    });
});

// ── inject(): reporter endpoint (exactly one var) ─────────────────────────────

describe("inject() — reporter endpoint", () => {
    it("unix endpoint → FLEET_HUB_SOCKET set, FLEET_HUB_WS_URL absent", () => {
        const coll = new MockEnvironmentVariableCollection();
        new EnvInjector(coll).inject({ sessionId: "s", hubSocketPath: "/run/f.sock" });

        expect(coll.get(FLEET_HUB_SOCKET_VAR)!.value).toBe("/run/f.sock");
        expect(coll.get(FLEET_HUB_WS_URL_VAR)).toBeUndefined();
    });

    it("ws endpoint → FLEET_HUB_WS_URL set, FLEET_HUB_SOCKET absent", () => {
        const coll = new MockEnvironmentVariableCollection();
        new EnvInjector(coll).inject({ sessionId: "s", hubWsUrl: "ws://127.0.0.1:51777" });

        expect(coll.get(FLEET_HUB_WS_URL_VAR)!.value).toBe("ws://127.0.0.1:51777");
        expect(coll.get(FLEET_HUB_SOCKET_VAR)).toBeUndefined();
    });

    it("no endpoint → neither endpoint var is set (only session id)", () => {
        const coll = new MockEnvironmentVariableCollection();
        new EnvInjector(coll).inject({ sessionId: "only-session" });

        expect(coll.get(FLEET_SESSION_ID_VAR)!.value).toBe("only-session");
        expect(coll.get(FLEET_HUB_SOCKET_VAR)).toBeUndefined();
        expect(coll.get(FLEET_HUB_WS_URL_VAR)).toBeUndefined();
    });

    it("switching ws→unix removes the stale ws var", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "s", hubWsUrl: "ws://h" });
        expect(coll.get(FLEET_HUB_WS_URL_VAR)).toBeDefined();

        inj.inject({ sessionId: "s", hubSocketPath: "/run/f.sock" });
        expect(coll.get(FLEET_HUB_WS_URL_VAR)).toBeUndefined();
        expect(coll.get(FLEET_HUB_SOCKET_VAR)!.value).toBe("/run/f.sock");
    });

    it("switching unix→ws removes the stale socket var", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "s", hubSocketPath: "/run/f.sock" });
        expect(coll.get(FLEET_HUB_SOCKET_VAR)).toBeDefined();

        inj.inject({ sessionId: "s", hubWsUrl: "ws://h" });
        expect(coll.get(FLEET_HUB_SOCKET_VAR)).toBeUndefined();
        expect(coll.get(FLEET_HUB_WS_URL_VAR)!.value).toBe("ws://h");
    });
});

// ── idempotency (no accumulation) ─────────────────────────────────────────────

describe("inject() idempotency", () => {
    it("re-injecting the same targets leaves exactly one mutator per variable", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "win-7", hubSocketPath: "/s" });
        inj.inject({ sessionId: "win-7", hubSocketPath: "/s" });
        inj.inject({ sessionId: "win-7", hubSocketPath: "/s" });

        // Only the two relevant vars; no duplicates (Map keyed by var name).
        expect(coll.map.size).toBe(2);
        expect(coll.get(FLEET_SESSION_ID_VAR)!.value).toBe("win-7");
        expect(coll.get(FLEET_HUB_SOCKET_VAR)!.value).toBe("/s");
    });

    it("re-injecting a new session id overwrites the old value", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "old", hubWsUrl: "ws://h" });
        inj.inject({ sessionId: "new", hubWsUrl: "ws://h" });
        expect(coll.get(FLEET_SESSION_ID_VAR)!.value).toBe("new");
        expect(coll.map.size).toBe(2);
    });
});

// ── dispose(): reversibility (invariant 6) ────────────────────────────────────

describe("dispose() — reversibility", () => {
    it("removes every Fleet var after injection", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "s", hubSocketPath: "/s" });
        expect(coll.map.size).toBeGreaterThan(0);

        inj.dispose();

        for (const v of FLEET_ENV_VARS) {
            expect(coll.get(v)).toBeUndefined();
        }
        expect(coll.map.size).toBe(0);
    });

    it("calls clear() on the collection (full reset)", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "s", hubWsUrl: "ws://h" });
        inj.dispose();
        expect(coll.cleared).toBe(true);
    });

    it("flips injected back to false", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "s", hubWsUrl: "ws://h" });
        inj.dispose();
        expect(inj.injected).toBe(false);
    });

    it("is safe to call before inject() (no throw, empty collection)", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        expect(() => inj.dispose()).not.toThrow();
        expect(coll.map.size).toBe(0);
    });

    it("is idempotent (double-dispose does not throw)", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "s", hubWsUrl: "ws://h" });
        expect(() => {
            inj.dispose();
            inj.dispose();
        }).not.toThrow();
        expect(coll.map.size).toBe(0);
    });

    it("can re-inject cleanly after dispose (round-trip)", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "s1", hubWsUrl: "ws://h" });
        inj.dispose();
        inj.inject({ sessionId: "s2", hubSocketPath: "/s" });

        expect(inj.injected).toBe(true);
        expect(coll.get(FLEET_SESSION_ID_VAR)!.value).toBe("s2");
        expect(coll.get(FLEET_HUB_SOCKET_VAR)!.value).toBe("/s");
        expect(coll.get(FLEET_HUB_WS_URL_VAR)).toBeUndefined();
    });
});

// ── scoping (build-time re-verify finding (2): workspace-scoped, not per-terminal)

describe("collection scoping (documented as workspace-scoped, not per-terminal)", () => {
    it("getScoped returns an isolated collection that does not affect the global one", () => {
        const global = new MockEnvironmentVariableCollection();
        new EnvInjector(global).inject({ sessionId: "win", hubWsUrl: "ws://h" });

        const scoped = global.getScoped({
            workspaceFolder: { uri: { fsPath: "/proj" } },
        });
        // The scoped collection is empty/distinct — injecting globally does NOT
        // leak into a scoped collection, matching the real API's isolation.
        expect(scoped).not.toBe(global);
        expect(scoped.get(FLEET_SESSION_ID_VAR)).toBeUndefined();
        // Global injection is intact (every terminal in the window sees it).
        expect(global.get(FLEET_SESSION_ID_VAR)!.value).toBe("win");
    });
});
