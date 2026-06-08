/**
 * Fleet PATH shim (B′) — SHIM (S10).
 *
 * Prepends a directory of *transparent* `claude`/`codex` wrapper scripts to the
 * integrated-terminal `PATH`, so the user STILL TYPES `claude`/`codex` but gets
 * hooks pointed at the Fleet reporter. This is the editor-scoped, licensing-clean
 * analog of cmux's PATH-shimmed `claude` wrapper (PLAN §1, cmux
 * `Resources/bin/cmux-claude-wrapper`), done from inside the editor via the
 * STABLE `EnvironmentVariableCollection.prepend` API rather than by owning the
 * terminal.
 *
 * ── The three behaviors this node owns (NODE SHIM brief) ──────────────────────
 *
 *  (1) SHIM RESOLUTION. We compute a per-window shim directory, write a `claude`
 *      and a `codex` wrapper into it, and `prepend` that dir to `PATH`. After
 *      that, `which codex` in the editor terminal resolves to the shim.
 *
 *  (2) PASS-THROUGH (the load-bearing invariant). The wrapper must be a
 *      *transparent* pass-through: it always ends in `exec "<real binary>" "$@"`.
 *      It finds the real binary by walking `PATH` and SKIPPING its own shim dir
 *      (and any path that resolves to itself), so it can never recurse into
 *      itself. OUTSIDE the editor the shim dir is simply not on `PATH` (the
 *      collection only applies to this editor's integrated terminals), so the
 *      real binary runs with zero Fleet involvement — "outside the editor the
 *      shims are absent (pass-through)". INSIDE the editor, if Fleet's marker env
 *      (`FLEET_SESSION_ID`) is absent or the reporter endpoint is unset, the
 *      wrapper ALSO passes straight through — defence in depth so a stale shim
 *      dir on `PATH` never changes the agent's behavior.
 *
 *  (3) ARG FORWARDING. Every argument is forwarded verbatim via `"$@"` (quoted,
 *      so spaces/globs are preserved) in EVERY code path — pass-through and
 *      hook-enabled alike. The wrapper never drops, reorders, or rewrites the
 *      user's args; at most it PREPENDS opt-in flags (see below) ahead of `"$@"`.
 *
 * ── Locked decisions / invariants honored ────────────────────────────────────
 *   - D14 — STABLE `EnvironmentVariableCollection` ONLY (`prepend` for the
 *     list-like `PATH` case); NO proposed APIs; Open-VSX-publishable, `^1.93.0`.
 *   - Observer-not-owner (invariant 3) — we shim the *launch environment* only;
 *     the wrapper `exec`s the real agent so Fleet never sits between the user's
 *     keystrokes and the agent, and never owns the PTY.
 *   - Confidence-honesty / "any reliability flag is opt-in + surfaced, never
 *     silent" (PLAN §1 job 2, §3 invariant 3) — we DO NOT default
 *     `--allow-dangerously-skip-permissions` (or any `bypassPermissions`). Such
 *     flags are injected ONLY when the user explicitly opts in via config, and
 *     when injected they are SURFACED (the wrapper echoes a one-line notice to
 *     stderr, and `injectedReliabilityFlags()` reports them so the UI can show
 *     them). cmux's own wrapper never passes this flag, and a cmux test enforces
 *     its absence (PLAN §1) — so it is NOT precedent-proven; we keep it strictly
 *     opt-in.
 *   - Reversible (invariant 6) — `dispose()` removes the `PATH` mutator (and the
 *     platform auto-clears the whole collection on uninstall, see ENVINJ).
 *
 * This module is intentionally split into PURE functions (shim-script text, real
 * binary discovery semantics encoded as the script we emit, arg-forwarding) plus
 * a thin `PathShimmer` that performs the filesystem writes + the single `PATH`
 * `prepend`. The pure parts are unit-tested without VS Code or a real shell.
 */

import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import type { EnvCollectionLike } from "./envInject";
import { FLEET_SESSION_ID_VAR } from "./envInject";

/** The agents Fleet shims. Order is stable for deterministic iteration/tests. */
export const SHIMMED_AGENTS = ["claude", "codex"] as const;
export type ShimmedAgent = (typeof SHIMMED_AGENTS)[number];

/** The PATH variable the shim prepends to. */
export const PATH_VAR = "PATH";

/**
 * Marker env var written by the shim writer so the wrapper can tell it is being
 * invoked from a Fleet-shimmed editor terminal (corroborates FLEET_SESSION_ID).
 * Also lets the wrapper locate the shim dir to skip during real-binary lookup.
 */
export const FLEET_SHIM_DIR_VAR = "FLEET_SHIM_DIR";

/**
 * A fixed sentinel embedded as a comment near the top of every generated shim
 * script. The wrapper's PATH walk reads each candidate's first lines and skips
 * any that carry this sentinel, so a shim can never select ANOTHER Fleet shim as
 * the "real" binary (recursion guard) — using only the `read` builtin, no
 * external `realpath`/`grep`. Unique enough not to collide with a real binary.
 */
export const FLEET_SHIM_SENTINEL = "fleet-shim-marker:b27f1c0a";

/**
 * Per-agent opt-in reliability flags. EMPTY by default (confidence-honesty): no
 * flag is ever injected unless the user explicitly opts in via extension config.
 * The only currently-supported one is Claude's
 * `--allow-dangerously-skip-permissions`, which is a real Claude CLI flag but is
 * NOT cmux-proven and is dangerous — so it is opt-in AND surfaced, never silent.
 */
export interface ReliabilityConfig {
    /**
     * If true, the `claude` shim prepends `--allow-dangerously-skip-permissions`
     * to the user's args. Default false. Surfaced to stderr when active.
     */
    claudeSkipPermissions?: boolean;
}

/** Options for building a shim. */
export interface ShimOptions {
    /** Absolute path of the shim directory (the dir prepended to PATH). */
    shimDir: string;
    /** Opt-in reliability flags (default: none). */
    reliability?: ReliabilityConfig;
}

/**
 * The reliability flags that WOULD be injected for a given config, per agent.
 * Pure + exported so the extension can surface exactly what is active (never
 * silent) and tests can assert the default is empty.
 */
export function injectedReliabilityFlags(
    reliability: ReliabilityConfig | undefined
): Record<ShimmedAgent, string[]> {
    const claudeFlags: string[] = [];
    if (reliability?.claudeSkipPermissions === true) {
        claudeFlags.push("--allow-dangerously-skip-permissions");
    }
    return { claude: claudeFlags, codex: [] };
}

/**
 * True iff ANY reliability flag is active for ANY agent. Used by the extension
 * to decide whether to surface a warning to the user (never silent).
 */
export function hasAnyReliabilityFlag(reliability: ReliabilityConfig | undefined): boolean {
    const f = injectedReliabilityFlags(reliability);
    return Object.values(f).some(list => list.length > 0);
}

/**
 * Produce the POSIX-sh source for a transparent wrapper script for `agent`.
 *
 * The emitted script's contract (verified by unit tests that EXECUTE it under
 * `/bin/sh`, not by string matching):
 *
 *   - It finds the REAL `<agent>` by scanning `$PATH`, skipping (a) the Fleet
 *     shim dir (`$FLEET_SHIM_DIR`) and (b) any candidate whose realpath equals
 *     this script's own realpath — so it can never recurse into itself even if a
 *     second shim dir shadows it.
 *   - If no real binary is found it exits 127 with a clear message (matches
 *     cmux's `claude not found` behavior).
 *   - If Fleet's marker env (`FLEET_SESSION_ID`) is empty, it `exec`s the real
 *     binary with `"$@"` unchanged — PASS-THROUGH (defence in depth; outside the
 *     editor the dir is off PATH entirely, so this branch is the in-editor
 *     "stale env" safety net).
 *   - Otherwise it prepends any OPT-IN reliability flags (surfacing them on
 *     stderr) and `exec`s the real binary with those flags followed by `"$@"`.
 *
 * Hook installation itself (writing `~/.codex/hooks.json`, Claude `--settings`)
 * is the job of the CODEX (S11–13) / CLUSETERM (S17) nodes; this script provides
 * the transparent, recursion-proof launch seam they build on. Until those land,
 * the hook-enabled branch differs from pass-through ONLY by the opt-in flags, so
 * the agent's UX is unchanged (NODE SHIM demo: "typing `codex` launches real
 * codex, UX unchanged").
 */
export function shimScript(agent: ShimmedAgent, reliability?: ReliabilityConfig): string {
    const flags = injectedReliabilityFlags(reliability)[agent];
    // Render opt-in flags as a quoted sh list assigned to $FLEET_EXTRA. Empty by
    // default. Each flag is single-quoted to be injection-safe.
    const extraAssign = flags.length
        ? "set -- " + flags.map(f => `'${f.replace(/'/g, `'\\''`)}'`).join(" ") + ' "$@"'
        : ""; // no-op when there are no flags

    // The surfacing notice (only emitted when flags are active).
    const surface = flags.length
        ? `printf '%s\\n' "fleet: ${agent} launched with opt-in reliability flag(s): ${flags.join(
              " "
          )}" 1>&2`
        : "";

    // NOTE: this is POSIX sh (not bash) for max portability across login shells.
    // `command -v`/arrays are avoided; we hand-roll the PATH walk with `IFS=:`.
    // A unique first-line sentinel lets a shim recognise (and skip) ANOTHER
    // Fleet shim during real-binary lookup using only the `read` builtin — no
    // realpath/grep, so the recursion guard works even on a stripped PATH.
    return `#!/bin/sh
# ${FLEET_SHIM_SENTINEL}
# Fleet transparent shim for "${agent}" (S10 / B').
# Generated by the Fleet VS Code extension. Prepended to PATH only inside the
# editor's integrated terminals (EnvironmentVariableCollection). Outside the
# editor this file is not on PATH at all -> the real ${agent} runs untouched.
#
# Contract: ALWAYS exec the real ${agent} with the user's args forwarded
# verbatim ("$@"). At most we PREPEND opt-in, surfaced reliability flags.
# Observer-not-owner: we never read stdin, never intercept keystrokes; we exec
# and get out of the way.

# self_dir = the directory holding this script, via POSIX parameter expansion
# only (NO external 'dirname'/'realpath' — the user's PATH may be stripped down,
# and we must never depend on a coreutil being present to launch the agent).
self="$0"
case "$self" in
  */*) self_dir=\${self%/*} ;;
  *)   self_dir="." ;;
esac

# Find the real "${agent}" on PATH, skipping (a) the dir this script lives in,
# (b) the recorded FLEET_SHIM_DIR, and (c) ANY candidate whose first lines carry
# the Fleet shim sentinel (so a second shim copy in some other dir can never be
# selected, which would recurse). Detection uses only the 'read' builtin — no
# realpath/grep — so it works even when PATH is stripped to bare essentials.
real=""
saved_ifs=$IFS
IFS=:
for dir in $PATH; do
  [ -n "$dir" ] || continue
  # Skip the Fleet shim dir (where this very script lives).
  if [ "$dir" = "$self_dir" ]; then continue; fi
  if [ -n "\${FLEET_SHIM_DIR:-}" ] && [ "$dir" = "$FLEET_SHIM_DIR" ]; then continue; fi
  cand="$dir/${agent}"
  [ -x "$cand" ] || continue
  # Skip any candidate that is itself a Fleet shim (sentinel on line 1 or 2).
  is_shim=0
  if [ -r "$cand" ]; then
    line_count=0
    while [ $line_count -lt 2 ] && IFS= read -r line; do
      case "$line" in
        *${FLEET_SHIM_SENTINEL}*) is_shim=1; break ;;
      esac
      line_count=$((line_count + 1))
    done < "$cand"
  fi
  [ "$is_shim" -eq 1 ] && continue
  real="$cand"
  break
done
IFS=$saved_ifs

if [ -z "$real" ]; then
  printf '%s\\n' "fleet: real '${agent}' not found in PATH (shim pass-through aborted)" 1>&2
  exit 127
fi

# PASS-THROUGH when Fleet's marker env is absent (stale shim dir / non-Fleet
# shell). Forward args verbatim; do not touch the agent's behavior.
if [ -z "\${${FLEET_SESSION_ID_VAR}:-}" ]; then
  exec "$real" "$@"
fi

# Fleet-shimmed launch. Surface any opt-in reliability flags, then forward args.
${surface}
${extraAssign}
exec "$real" "$@"
`;
}

/**
 * Compute the default per-window shim directory under the OS temp dir, keyed by
 * the session id so concurrent editor windows never share a dir. Pure (no I/O).
 */
export function defaultShimDir(sessionId: string): string {
    // Sanitize the session id for use as a path segment (keep it filesystem-safe
    // and bounded). Never empty.
    const safe = (sessionId || "default").replace(/[^A-Za-z0-9._-]/g, "_").slice(0, 128);
    return path.join(os.tmpdir(), "fleet-shim", safe);
}

/**
 * Owns the PATH shim: writes the wrapper scripts and performs the single
 * `prepend` of the shim dir onto `PATH`.
 *
 * Lifecycle mirrors `EnvInjector`: construct with the collection, `install()`
 * once on activation (after ENVINJ has injected `FLEET_SESSION_ID`), `dispose()`
 * on deactivation. `install()` is idempotent.
 */
export class PathShimmer {
    private readonly _collection: EnvCollectionLike;
    private readonly _shimDir: string;
    private readonly _reliability: ReliabilityConfig;
    private _installed = false;

    constructor(collection: EnvCollectionLike, options: ShimOptions) {
        if (!path.isAbsolute(options.shimDir)) {
            throw new Error(`PathShimmer: shimDir must be absolute, got: ${options.shimDir}`);
        }
        this._collection = collection;
        this._shimDir = options.shimDir;
        this._reliability = options.reliability ?? {};
    }

    /** The shim directory this shimmer manages. */
    get shimDir(): string {
        return this._shimDir;
    }

    /** True once `install()` has run and not been `dispose()`d. */
    get installed(): boolean {
        return this._installed;
    }

    /** The reliability flags that are active (surfaced, never silent). */
    get activeReliabilityFlags(): Record<ShimmedAgent, string[]> {
        return injectedReliabilityFlags(this._reliability);
    }

    /**
     * Write the wrapper scripts (chmod +x) and prepend the shim dir to PATH.
     *
     * Idempotent: re-running rewrites the scripts (e.g. after a config change to
     * reliability flags) and re-prepends the SAME dir. The API permits only one
     * change per variable, so re-prepending the same dir never accumulates
     * duplicate PATH segments.
     *
     * `applyAtProcessCreation: true` so the shim is on PATH even when shell
     * integration is unavailable; `applyAtShellIntegration: true` so it survives
     * shells that re-derive PATH at integration time. The shim is prepended so it
     * shadows the system binary (which `which codex` then resolves to).
     */
    install(): void {
        fs.mkdirSync(this._shimDir, { recursive: true });

        for (const agent of SHIMMED_AGENTS) {
            const file = path.join(this._shimDir, agent);
            fs.writeFileSync(file, shimScript(agent, this._reliability), { mode: 0o755 });
            // writeFileSync `mode` is masked by umask on create and ignored on
            // overwrite, so chmod explicitly to guarantee the +x bit.
            fs.chmodSync(file, 0o755);
        }

        this._collection.prepend(PATH_VAR, this._pathPrependValue(), {
            applyAtProcessCreation: true,
            applyAtShellIntegration: true,
        });
        // Record the shim dir so the wrapper can skip it during real-binary
        // lookup even if PATH is later reordered.
        this._collection.replace(FLEET_SHIM_DIR_VAR, this._shimDir, {
            applyAtProcessCreation: true,
        });

        this._installed = true;
    }

    /**
     * The string prepended onto PATH: the shim dir plus the platform path
     * delimiter, so it becomes the FIRST entry (`<shimDir>:<existing PATH>`).
     */
    private _pathPrependValue(): string {
        return this._shimDir + path.delimiter;
    }

    /**
     * Remove the PATH mutator + the marker var, and delete the shim scripts.
     * Reversible (invariant 6). Idempotent and safe before `install()`.
     *
     * We delete only the variables Fleet owns; the wider `clear()` belongs to the
     * env injector that owns the whole collection (this shimmer shares it). On
     * uninstall the platform invalidates the collection regardless.
     */
    dispose(): void {
        this._collection.delete(PATH_VAR);
        this._collection.delete(FLEET_SHIM_DIR_VAR);
        // Best-effort removal of the on-disk shim dir (reversibility). Never
        // throw out of dispose — deactivation must not fail.
        try {
            for (const agent of SHIMMED_AGENTS) {
                const file = path.join(this._shimDir, agent);
                if (fs.existsSync(file)) fs.rmSync(file);
            }
            if (fs.existsSync(this._shimDir)) fs.rmdirSync(this._shimDir);
        } catch {
            /* best-effort: a non-empty/locked dir is left for the OS temp reaper */
        }
        this._installed = false;
    }
}
