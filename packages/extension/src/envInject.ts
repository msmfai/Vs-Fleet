/**
 * Fleet env injection — ENVINJ (S9).
 *
 * Injects Fleet's per-window identity into every shell launched in the editor's
 * integrated terminal, using the STABLE `EnvironmentVariableCollection` API.
 * This is the sanctioned, licensing-clean analog of cmux's env injection
 * (PLAN §1): cmux *owns* the terminals it spawns and injects `CMUX_SURFACE_ID` /
 * `CMUX_SOCKET_PATH`; Fleet refuses to own the terminal (§4.1/§4.5, invariant 3
 * observer-not-owner), so it injects the same kind of identity from *inside* the
 * editor without spawning anything.
 *
 * Two variables are injected:
 *   - `FLEET_SESSION_ID`   — the per-window Fleet session id, so a `claude`/
 *     `codex` run started in this window can be correlated back to the editor
 *     window (run↔editor correlation; also fixes focus/jump mapping, §12.2).
 *   - `FLEET_REPORTER_SOCKET` — the per-window **reporter** socket path, where
 *     `fleet-reporter --serve` listens for this window's hooks. The shim reads it
 *     to point Claude/Codex hooks at the reporter (consumed by SHIM/S10 + the
 *     Codex/Claude hooks). This is the reporter socket, NOT the Hub socket — the
 *     terminals never talk to the Hub directly.
 *
 * ── Locked decisions / invariants honored ────────────────────────────────────
 *   - D14 — STABLE `EnvironmentVariableCollection` ONLY; NO proposed APIs; the
 *     extension stays Open-VSX-publishable, engine `^1.93.0`.
 *   - Observer-not-owner (invariant 3) — we only shim the *launch environment*;
 *     no keystrokes are intercepted and no agent is launched through Fleet.
 *   - Reversible (invariant 6) — every mutator we add can be removed by
 *     `dispose()` (which calls `clear()`), and the platform also auto-clears the
 *     collection on uninstall (see BUILD-TIME RE-VERIFY below).
 *
 * ── BUILD-TIME RE-VERIFY (PLAN §6, against @types/vscode index.d.ts ^1.93) ────
 *
 * Findings encoded as comments per the brief, grounded in the authoritative API
 * doc-comments (not memory):
 *
 * (1) DISPOSE / UNINSTALL behavior — CONFIRMED auto-clearing.
 *     The `EnvironmentVariableCollection.persistent` doc says verbatim: "The
 *     collection will be invalidated when the extension is uninstalled or when
 *     the collection is cleared." So on uninstall the platform drops our
 *     mutators; we do NOT have to manually wipe them on the way out. This
 *     resolves the historical "env not cleared on uninstall" worry (vscode
 *     #234384, closed completed Dec 2024 — PLAN §6). We STILL call `clear()` in
 *     `dispose()` for an in-session disable/reload (where uninstall semantics do
 *     not apply) and so a re-activate starts from a known-clean collection.
 *     Because the platform auto-clears on uninstall, the "relaunch terminal to
 *     drop stale env" affordance is NOT needed (PLAN §6) — already-open
 *     terminals keep the old env until relaunched, which is the platform's
 *     documented behavior, not a Fleet bug.
 *
 * (2) SCOPING — workspace-scoped, NOT per-terminal.
 *     `context.environmentVariableCollection` is a
 *     `GlobalEnvironmentVariableCollection`; `getScoped({ workspaceFolder })`
 *     narrows to a workspace folder, and `EnvironmentVariableScope` exposes ONLY
 *     `workspaceFolder` — there is NO per-terminal scope (vscode #138109, closed
 *     by-design — PLAN §6). Consequence: identity injection is coarser than
 *     cmux's per-surface scheme — EVERY integrated-terminal shell in this window
 *     inherits the SAME `FLEET_SESSION_ID`. That is correct for Fleet's model:
 *     `FLEET_SESSION_ID` is the *window's* id, and per-run identity is assigned
 *     downstream by the reporter from the run's durable id (Codex `thread.id` /
 *     Claude `session_id`), not from the env var. We therefore inject at the
 *     GLOBAL collection by default (applies to all of this window's terminals);
 *     `getScoped` is available if a future slice needs per-folder values.
 *
 * (3) MUTATION SEMANTICS — `replace` is correct for identity (single-value).
 *     The API enforces "an extension can only make a single change to any one
 *     variable" — replace/append/prepend overwrite each other for a given
 *     variable. Identity vars are whole values (not list-like PATH segments), so
 *     we use `replace`. (SHIM/S10 will `prepend` PATH, the list-like case.) We
 *     set `applyAtProcessCreation: true` (the doc default for replace) so the
 *     value is present even when shell-integration is unavailable — robustness
 *     the OSC read-stream path (S18) does not require.
 */

/** The window-identity env var injected into every integrated-terminal shell. */
export const FLEET_SESSION_ID_VAR = "FLEET_SESSION_ID";

/**
 * The **reporter** socket path env var injected into every integrated-terminal
 * shell. This is the per-window socket where `fleet-reporter --serve` listens —
 * NOT the Hub socket. The shim reads it to point Claude/Codex hooks at the
 * reporter (re-exported from `./paths`, the single source of truth shared with
 * the Rust side's `FLEET_REPORTER_SOCKET`).
 */
export { FLEET_REPORTER_SOCKET_VAR } from "./paths";
import { FLEET_REPORTER_SOCKET_VAR } from "./paths";

/**
 * The set of env vars Fleet owns in the collection. Used by `dispose()` to
 * remove exactly what we added (defence-in-depth alongside `clear()`), and by
 * tests to assert the reversible contract.
 */
export const FLEET_ENV_VARS: readonly string[] = [
    FLEET_SESSION_ID_VAR,
    FLEET_REPORTER_SOCKET_VAR,
] as const;

/**
 * Minimal structural type for the part of `EnvironmentVariableCollection` we
 * use. Declaring it locally (rather than importing the full vscode interface)
 * keeps the unit under test trivially mockable and documents the exact surface
 * we depend on — all of it STABLE (^1.93), none proposed.
 */
export interface EnvCollectionLike {
    persistent: boolean;
    // `description` is `string | MarkdownString | undefined` in the real API; we
    // only ever assign a plain string. Typed loosely so both the real
    // `GlobalEnvironmentVariableCollection` and a test double are assignable
    // without dragging in the full `MarkdownString` interface.
    description: unknown;
    replace(
        variable: string,
        value: string,
        options?: { applyAtProcessCreation?: boolean; applyAtShellIntegration?: boolean }
    ): void;
    // `prepend` is the list-like mutator (used by SHIM/S10 for the PATH var).
    // ENVINJ itself only uses `replace`, but the shared collection needs the
    // method on the structural type so PathShimmer can be typed against it too.
    prepend(
        variable: string,
        value: string,
        options?: { applyAtProcessCreation?: boolean; applyAtShellIntegration?: boolean }
    ): void;
    delete(variable: string): void;
    clear(): void;
}

/**
 * What gets injected into the integrated-terminal environment.
 */
export interface InjectionTargets {
    /** The per-window Fleet session id. */
    sessionId: string;
    /**
     * The per-window **reporter** socket path — where `fleet-reporter --serve`
     * listens for this window's Claude/Codex hooks. The shim reads this to point
     * the agent's hooks at the reporter. (Distinct from the Hub socket, which the
     * terminals never talk to — only the reporter and the extension connect to
     * the Hub.)
     */
    reporterSocket: string;
}

/**
 * Owns Fleet's mutations to the integrated-terminal environment.
 *
 * Construct with the extension's `environmentVariableCollection`, call
 * `inject()` once on activation, and `dispose()` on deactivation. `inject()` is
 * idempotent — re-injecting (e.g. after a session-id change) overwrites prior
 * values for the same variables, never accumulating.
 */
export class EnvInjector {
    private readonly _collection: EnvCollectionLike;
    private _injected = false;

    constructor(collection: EnvCollectionLike) {
        this._collection = collection;
        // Persist across window reloads so a reloaded window's terminals still
        // carry identity without waiting for re-activation. (Default is true;
        // set explicitly to make the intent — and the reversible contract —
        // legible.)
        this._collection.persistent = true;
        this._collection.description =
            "Fleet — injects FLEET_SESSION_ID + reporter endpoint into integrated-terminal shells";
    }

    /** True once `inject()` has run and not been `dispose()`d. */
    get injected(): boolean {
        return this._injected;
    }

    /**
     * Inject Fleet identity into the (workspace-scoped) integrated-terminal env.
     *
     * Idempotent: `replace` overwrites any prior Fleet mutator for the same
     * variable, so calling this twice (or after a session-id change) leaves the
     * collection with exactly the latest values — never duplicates (the API
     * permits only one change per variable regardless).
     *
     * We `replace` (not `prepend`/`append`): these are whole-value identity
     * vars, not list-like PATH segments. `applyAtProcessCreation: true` ensures
     * the value is present even without shell integration.
     */
    inject(targets: InjectionTargets): void {
        const opts = { applyAtProcessCreation: true };

        this._collection.replace(FLEET_SESSION_ID_VAR, targets.sessionId, opts);
        this._collection.replace(FLEET_REPORTER_SOCKET_VAR, targets.reporterSocket, opts);

        this._injected = true;
    }

    /**
     * Remove every Fleet mutator (reversibility, invariant 6).
     *
     * Called on deactivation (window close / disable / reload). We `clear()` the
     * whole collection — Fleet is the sole owner of this extension's collection,
     * so clearing it removes exactly our vars and nothing else (the collection
     * is per-extension). The per-var `delete()` calls are belt-and-suspenders in
     * case a future slice shares the collection. On UNINSTALL the platform
     * invalidates the collection for us regardless (see BUILD-TIME RE-VERIFY (1)).
     *
     * Idempotent and safe to call before `inject()`.
     */
    dispose(): void {
        for (const v of FLEET_ENV_VARS) {
            this._collection.delete(v);
        }
        this._collection.clear();
        this._injected = false;
    }
}
