/**
 * Unit tests for ENVINJ (S9) — env injection via EnvironmentVariableCollection.
 *
 * The vscode module is replaced by the mock at src/__mocks__/vscode.ts. We
 * drive `EnvInjector` against the faithful `MockEnvironmentVariableCollection`,
 * which models the documented API invariants (single-change-per-variable,
 * clear/delete, getScoped isolation).
 *
 * These tests assert the build-relevant contract:
 *   - the RIGHT vars (FLEET_SESSION_ID + FLEET_REPORTER_SOCKET) are injected with
 *     the RIGHT options (applyAtProcessCreation),
 *   - injection is idempotent (no accumulation),
 *   - re-injection overwrites values rather than accumulating,
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
    FLEET_SESSION_ID_VAR,
    FLEET_REPORTER_SOCKET_VAR,
    FLEET_ENV_VARS,
} from "../envInject";

const SOCK = "/run/user/1000/fleet/reporter-win.sock";

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
        new EnvInjector(coll).inject({ sessionId: "win-42", reporterSocket: SOCK });

        const m = coll.get(FLEET_SESSION_ID_VAR);
        expect(m).toBeDefined();
        expect(m!.value).toBe("win-42");
        expect(m!.type).toBe(EnvironmentVariableMutatorType.Replace);
    });

    it("injects with applyAtProcessCreation:true (works without shell integration)", () => {
        const coll = new MockEnvironmentVariableCollection();
        new EnvInjector(coll).inject({ sessionId: "win-1", reporterSocket: SOCK });
        expect(coll.get(FLEET_SESSION_ID_VAR)!.options.applyAtProcessCreation).toBe(true);
    });

    it("flips injected to true", () => {
        const inj = new EnvInjector(new MockEnvironmentVariableCollection());
        inj.inject({ sessionId: "x", reporterSocket: SOCK });
        expect(inj.injected).toBe(true);
    });
});

// ── inject(): reporter socket ─────────────────────────────────────────────────

describe("inject() — FLEET_REPORTER_SOCKET", () => {
    it("injects the per-window reporter socket via replace", () => {
        const coll = new MockEnvironmentVariableCollection();
        new EnvInjector(coll).inject({ sessionId: "s", reporterSocket: SOCK });

        const m = coll.get(FLEET_REPORTER_SOCKET_VAR);
        expect(m!.value).toBe(SOCK);
        expect(m!.type).toBe(EnvironmentVariableMutatorType.Replace);
        expect(m!.options.applyAtProcessCreation).toBe(true);
    });

    it("injects exactly the two Fleet vars (session id + reporter socket)", () => {
        const coll = new MockEnvironmentVariableCollection();
        new EnvInjector(coll).inject({ sessionId: "s", reporterSocket: SOCK });
        expect(coll.map.size).toBe(2);
        expect(coll.get(FLEET_SESSION_ID_VAR)!.value).toBe("s");
        expect(coll.get(FLEET_REPORTER_SOCKET_VAR)!.value).toBe(SOCK);
    });
});

// ── idempotency (no accumulation) ─────────────────────────────────────────────

describe("inject() idempotency", () => {
    it("re-injecting the same targets leaves exactly one mutator per variable", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "win-7", reporterSocket: SOCK });
        inj.inject({ sessionId: "win-7", reporterSocket: SOCK });
        inj.inject({ sessionId: "win-7", reporterSocket: SOCK });

        expect(coll.map.size).toBe(2);
        expect(coll.get(FLEET_SESSION_ID_VAR)!.value).toBe("win-7");
        expect(coll.get(FLEET_REPORTER_SOCKET_VAR)!.value).toBe(SOCK);
    });

    it("re-injecting a new session id / socket overwrites the old values", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "old", reporterSocket: "/run/a.sock" });
        inj.inject({ sessionId: "new", reporterSocket: "/run/b.sock" });
        expect(coll.get(FLEET_SESSION_ID_VAR)!.value).toBe("new");
        expect(coll.get(FLEET_REPORTER_SOCKET_VAR)!.value).toBe("/run/b.sock");
        expect(coll.map.size).toBe(2);
    });
});

// ── dispose(): reversibility (invariant 6) ────────────────────────────────────

describe("dispose() — reversibility", () => {
    it("removes every Fleet var after injection", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "s", reporterSocket: SOCK });
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
        inj.inject({ sessionId: "s", reporterSocket: SOCK });
        inj.dispose();
        expect(coll.cleared).toBe(true);
    });

    it("flips injected back to false", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "s", reporterSocket: SOCK });
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
        inj.inject({ sessionId: "s", reporterSocket: SOCK });
        expect(() => {
            inj.dispose();
            inj.dispose();
        }).not.toThrow();
        expect(coll.map.size).toBe(0);
    });

    it("can re-inject cleanly after dispose (round-trip)", () => {
        const coll = new MockEnvironmentVariableCollection();
        const inj = new EnvInjector(coll);
        inj.inject({ sessionId: "s1", reporterSocket: "/run/a.sock" });
        inj.dispose();
        inj.inject({ sessionId: "s2", reporterSocket: "/run/b.sock" });

        expect(inj.injected).toBe(true);
        expect(coll.get(FLEET_SESSION_ID_VAR)!.value).toBe("s2");
        expect(coll.get(FLEET_REPORTER_SOCKET_VAR)!.value).toBe("/run/b.sock");
    });
});

// ── scoping (build-time re-verify finding (2): workspace-scoped, not per-terminal)

describe("collection scoping (documented as workspace-scoped, not per-terminal)", () => {
    it("getScoped returns an isolated collection that does not affect the global one", () => {
        const global = new MockEnvironmentVariableCollection();
        new EnvInjector(global).inject({ sessionId: "win", reporterSocket: SOCK });

        const scoped = global.getScoped({
            workspaceFolder: { uri: { fsPath: "/proj" } },
        });
        expect(scoped).not.toBe(global);
        expect(scoped.get(FLEET_SESSION_ID_VAR)).toBeUndefined();
        expect(global.get(FLEET_SESSION_ID_VAR)!.value).toBe("win");
    });
});
