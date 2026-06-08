/**
 * Unit tests for SHIM (S10) — the transparent PATH-shim `claude`/`codex`
 * wrappers (B′).
 *
 * The NODE SHIM brief requires unit tests for: shim resolution + pass-through +
 * arg forwarding. We test these THREE ways:
 *
 *  1. PURE logic (`injectedReliabilityFlags`, `defaultShimDir`, `shimScript`
 *     shape) — no I/O.
 *  2. EXECUTING the generated wrapper under `/bin/sh` against fake `claude`/
 *     `codex` binaries on a synthetic PATH — proves the resolution / pass-through
 *     / arg-forwarding / recursion-guard / surfacing CONTRACT at runtime (not by
 *     string-matching the script). Skipped automatically on win32.
 *  3. `PathShimmer` against the faithful `MockEnvironmentVariableCollection`
 *     (the same mock ENVINJ uses) + a real temp dir — proves the PATH `prepend`,
 *     the marker var, idempotency, and reversible `dispose()`.
 */

import { execFileSync } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";

import { MockEnvironmentVariableCollection } from "../__mocks__/vscode";
import { FLEET_SESSION_ID_VAR } from "../envInject";
import {
    PathShimmer,
    SHIMMED_AGENTS,
    PATH_VAR,
    FLEET_SHIM_DIR_VAR,
    shimScript,
    defaultShimDir,
    injectedReliabilityFlags,
    hasAnyReliabilityFlag,
    claudeHooksSettings,
    hookRelayCommand,
    CLAUDE_HOOKS_FILE,
    type ShimmedAgent,
} from "../shim";

const onWindows = process.platform === "win32";
// `describe.skip`-style guard: shell-exec tests only run on POSIX.
const describePosix = onWindows ? describe.skip : describe;

// ── temp-dir bookkeeping ──────────────────────────────────────────────────────

const tmpDirs: string[] = [];
function makeTmpDir(prefix = "fleet-shim-test-"): string {
    const d = fs.mkdtempSync(path.join(os.tmpdir(), prefix));
    tmpDirs.push(d);
    return d;
}
afterAll(() => {
    for (const d of tmpDirs) {
        try {
            fs.rmSync(d, { recursive: true, force: true });
        } catch {
            /* best-effort cleanup */
        }
    }
});

/**
 * Create a fake executable `name` in `dir` that, when run, prints a stable
 * marker plus its forwarded argv (one per line) so tests can assert exact
 * arg forwarding. Returns the file path.
 */
function makeFakeBinary(dir: string, name: string, marker = `REAL_${name.toUpperCase()}`): string {
    fs.mkdirSync(dir, { recursive: true });
    const file = path.join(dir, name);
    fs.writeFileSync(
        file,
        `#!/bin/sh\nprintf '%s\\n' "${marker}"\nfor a in "$@"; do printf 'ARG:%s\\n' "$a"; done\n`,
        { mode: 0o755 }
    );
    fs.chmodSync(file, 0o755);
    return file;
}

/** Write a shim script for `agent` into `shimDir` and chmod +x. */
function writeShim(
    shimDir: string,
    agent: ShimmedAgent,
    reliability?: Parameters<typeof shimScript>[1]
): string {
    fs.mkdirSync(shimDir, { recursive: true });
    const file = path.join(shimDir, agent);
    fs.writeFileSync(file, shimScript(agent, reliability), { mode: 0o755 });
    fs.chmodSync(file, 0o755);
    return file;
}

/**
 * Run `shimPath` under /bin/sh with the given PATH + extra env + args, and
 * return { stdout, stderr, status }.
 */
function runShim(
    shimPath: string,
    opts: { pathDirs: string[]; env?: Record<string, string>; args?: string[]; shimDir?: string }
): { stdout: string; stderr: string; status: number } {
    const env: Record<string, string> = {
        // Start from a minimal, deterministic env — do NOT inherit the test
        // runner's PATH (it might contain a real `claude`/`codex`).
        PATH: opts.pathDirs.join(path.delimiter),
        ...(opts.shimDir ? { [FLEET_SHIM_DIR_VAR]: opts.shimDir } : {}),
        ...(opts.env ?? {}),
    };
    // Capture stderr separately via a temp file so we get it on success too
    // (execFileSync only surfaces stderr on its thrown error object).
    const errFile = path.join(makeTmpDir("fleet-shim-stderr-"), "stderr");
    try {
        const fd = fs.openSync(errFile, "w");
        let stdout: string;
        try {
            stdout = execFileSync("/bin/sh", [shimPath, ...(opts.args ?? [])], {
                env,
                encoding: "utf8",
                stdio: ["ignore", "pipe", fd],
            });
        } finally {
            fs.closeSync(fd);
        }
        const stderr = fs.readFileSync(errFile, "utf8");
        return { stdout, stderr, status: 0 };
    } catch (err: unknown) {
        const e = err as { stdout?: Buffer | string; stderr?: Buffer | string; status?: number };
        // stderr was redirected to errFile (fd), so read it back here too.
        let stderr = "";
        try {
            stderr = fs.readFileSync(errFile, "utf8");
        } catch {
            stderr = e.stderr ? String(e.stderr) : "";
        }
        return {
            stdout: e.stdout ? String(e.stdout) : "",
            stderr,
            status: typeof e.status === "number" ? e.status : 1,
        };
    }
}

// ── PURE: injectedReliabilityFlags / hasAnyReliabilityFlag ────────────────────

describe("injectedReliabilityFlags() — confidence-honesty default", () => {
    it("injects NO flags by default (never silent bypassPermissions)", () => {
        const f = injectedReliabilityFlags(undefined);
        expect(f.claude).toEqual([]);
        expect(f.codex).toEqual([]);
        expect(hasAnyReliabilityFlag(undefined)).toBe(false);
    });

    it("injects NO flags for an empty config", () => {
        expect(injectedReliabilityFlags({})).toEqual({ claude: [], codex: [] });
        expect(hasAnyReliabilityFlag({})).toBe(false);
    });

    it("does NOT default --allow-dangerously-skip-permissions even when other config present", () => {
        // Only the explicit opt-in flips it on.
        expect(injectedReliabilityFlags({ claudeSkipPermissions: false }).claude).toEqual([]);
    });

    it("adds --allow-dangerously-skip-permissions ONLY on explicit opt-in", () => {
        const f = injectedReliabilityFlags({ claudeSkipPermissions: true });
        expect(f.claude).toEqual(["--allow-dangerously-skip-permissions"]);
        expect(f.codex).toEqual([]); // never leaks to codex
        expect(hasAnyReliabilityFlag({ claudeSkipPermissions: true })).toBe(true);
    });
});

// ── PURE: defaultShimDir ──────────────────────────────────────────────────────

describe("defaultShimDir()", () => {
    it("is absolute and namespaced under the OS temp dir", () => {
        const d = defaultShimDir("win-42");
        expect(path.isAbsolute(d)).toBe(true);
        expect(d.startsWith(os.tmpdir())).toBe(true);
        expect(d).toContain("fleet-shim");
    });

    it("is distinct per session id (concurrent windows never share a dir)", () => {
        expect(defaultShimDir("a")).not.toBe(defaultShimDir("b"));
    });

    it("sanitizes unsafe session ids and never yields an empty segment", () => {
        const d = defaultShimDir("../../etc/passwd has spaces");
        // No path traversal survives; the last segment is sanitized.
        expect(path.basename(d)).not.toContain("/");
        expect(path.basename(d)).not.toContain(" ");
        expect(path.basename(defaultShimDir("")).length).toBeGreaterThan(0);
    });
});

// ── PURE: shimScript shape ────────────────────────────────────────────────────

describe("shimScript() shape", () => {
    it.each(SHIMMED_AGENTS)("emits a /bin/sh script that execs the real %s with \"$@\"", agent => {
        const s = shimScript(agent);
        expect(s.startsWith("#!/bin/sh")).toBe(true);
        // Always forwards args verbatim and execs the resolved real binary.
        expect(s).toContain('exec "$real" "$@"');
    });

    it("does not embed --allow-dangerously-skip-permissions by default", () => {
        for (const agent of SHIMMED_AGENTS) {
            expect(shimScript(agent)).not.toContain("--allow-dangerously-skip-permissions");
        }
    });

    it("embeds the opt-in flag ONLY when explicitly enabled (and only for claude)", () => {
        expect(shimScript("claude", { claudeSkipPermissions: true })).toContain(
            "--allow-dangerously-skip-permissions"
        );
        expect(shimScript("codex", { claudeSkipPermissions: true })).not.toContain(
            "--allow-dangerously-skip-permissions"
        );
    });
});

// ── RUNTIME: shim resolution + pass-through + arg forwarding ───────────────────

describePosix("generated wrapper — runtime contract (executed under /bin/sh)", () => {
    it.each(SHIMMED_AGENTS)("resolves and execs the real %s found on PATH", agent => {
        const shimDir = makeTmpDir();
        const realDir = makeTmpDir();
        makeFakeBinary(realDir, agent);
        const shim = writeShim(shimDir, agent);

        const r = runShim(shim, {
            pathDirs: [shimDir, realDir],
            shimDir,
            env: { [FLEET_SESSION_ID_VAR]: "win-1" },
        });
        expect(r.status).toBe(0);
        expect(r.stdout).toContain(`REAL_${agent.toUpperCase()}`);
    });

    it.each(SHIMMED_AGENTS)("forwards args verbatim to the real %s", agent => {
        const shimDir = makeTmpDir();
        const realDir = makeTmpDir();
        makeFakeBinary(realDir, agent);
        const shim = writeShim(shimDir, agent);

        const args = ["--resume", "my session", "-p", "hello --world", "a b c"];
        const r = runShim(shim, {
            pathDirs: [shimDir, realDir],
            shimDir,
            env: { [FLEET_SESSION_ID_VAR]: "win-1" },
            args,
        });
        expect(r.status).toBe(0);
        // Each arg is forwarded EXACTLY once, in order, spaces preserved.
        const forwarded = r.stdout
            .split("\n")
            .filter(l => l.startsWith("ARG:"))
            .map(l => l.slice("ARG:".length));
        expect(forwarded).toEqual(args);
    });

    it.each(SHIMMED_AGENTS)("passes through unchanged when FLEET_SESSION_ID is absent (%s)", agent => {
        const shimDir = makeTmpDir();
        const realDir = makeTmpDir();
        makeFakeBinary(realDir, agent);
        const shim = writeShim(shimDir, agent);

        // No FLEET_SESSION_ID -> pure pass-through. Args still forwarded verbatim.
        const args = ["chat", "--model", "gpt"];
        const r = runShim(shim, { pathDirs: [shimDir, realDir], shimDir, args });
        expect(r.status).toBe(0);
        expect(r.stdout).toContain(`REAL_${agent.toUpperCase()}`);
        const forwarded = r.stdout
            .split("\n")
            .filter(l => l.startsWith("ARG:"))
            .map(l => l.slice("ARG:".length));
        expect(forwarded).toEqual(args);
        // Pass-through path emits NO surfacing notice.
        expect(r.stderr).toBe("");
    });

    it("never recurses into itself: skips the shim dir and finds the real binary after it on PATH", () => {
        // Two shim dirs both on PATH BEFORE the real dir. A naive wrapper that
        // just took the next `claude` on PATH would pick the OTHER shim and loop.
        const shimDirA = makeTmpDir();
        const shimDirB = makeTmpDir();
        const realDir = makeTmpDir();
        makeFakeBinary(realDir, "claude");
        const shimA = writeShim(shimDirA, "claude");
        writeShim(shimDirB, "claude"); // a second shim copy shadowing the real one

        const r = runShim(shimA, {
            // shimA, then shimB (another shim!), then the real dir.
            pathDirs: [shimDirA, shimDirB, realDir],
            shimDir: shimDirA,
            env: { [FLEET_SESSION_ID_VAR]: "win-1" },
        });
        // Recursion guard (realpath self-skip) means we still reach REAL_CLAUDE.
        expect(r.status).toBe(0);
        expect(r.stdout).toContain("REAL_CLAUDE");
    });

    it("exits 127 with a clear message when no real binary exists", () => {
        const shimDir = makeTmpDir();
        const shim = writeShim(shimDir, "codex");
        // PATH contains ONLY the shim dir -> no real codex anywhere.
        const r = runShim(shim, {
            pathDirs: [shimDir],
            shimDir,
            env: { [FLEET_SESSION_ID_VAR]: "win-1" },
        });
        expect(r.status).toBe(127);
        expect(r.stderr).toContain("not found");
    });

    it("does NOT pass --allow-dangerously-skip-permissions by default (real claude sees only user args)", () => {
        const shimDir = makeTmpDir();
        const realDir = makeTmpDir();
        makeFakeBinary(realDir, "claude");
        const shim = writeShim(shimDir, "claude"); // default reliability

        const r = runShim(shim, {
            pathDirs: [shimDir, realDir],
            shimDir,
            env: { [FLEET_SESSION_ID_VAR]: "win-1" },
            args: ["hello"],
        });
        const forwarded = r.stdout
            .split("\n")
            .filter(l => l.startsWith("ARG:"))
            .map(l => l.slice("ARG:".length));
        expect(forwarded).toEqual(["hello"]); // no injected flag
        expect(r.stderr).toBe(""); // nothing surfaced
    });

    it("prepends the opt-in flag AND surfaces it on stderr when enabled (never silent)", () => {
        const shimDir = makeTmpDir();
        const realDir = makeTmpDir();
        makeFakeBinary(realDir, "claude");
        const shim = writeShim(shimDir, "claude", { claudeSkipPermissions: true });

        const r = runShim(shim, {
            pathDirs: [shimDir, realDir],
            shimDir,
            env: { [FLEET_SESSION_ID_VAR]: "win-1" },
            args: ["hello"],
        });
        const forwarded = r.stdout
            .split("\n")
            .filter(l => l.startsWith("ARG:"))
            .map(l => l.slice("ARG:".length));
        // Flag is PREPENDED, user args preserved after it.
        expect(forwarded).toEqual(["--allow-dangerously-skip-permissions", "hello"]);
        // Surfaced (never silent).
        expect(r.stderr).toContain("opt-in reliability flag");
        expect(r.stderr).toContain("--allow-dangerously-skip-permissions");
    });
});

// ── PathShimmer (against the mock collection + a real temp dir) ────────────────

describe("PathShimmer.install()", () => {
    it("rejects a non-absolute shimDir", () => {
        const coll = new MockEnvironmentVariableCollection();
        expect(() => new PathShimmer(coll, { shimDir: "relative/dir" })).toThrow(/absolute/);
    });

    it("writes an executable wrapper for every shimmed agent", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        new PathShimmer(coll, { shimDir }).install();

        for (const agent of SHIMMED_AGENTS) {
            const file = path.join(shimDir, agent);
            expect(fs.existsSync(file)).toBe(true);
            if (!onWindows) {
                // +x bit set.
                expect(fs.statSync(file).mode & 0o111).not.toBe(0);
            }
            expect(fs.readFileSync(file, "utf8").startsWith("#!/bin/sh")).toBe(true);
        }
    });

    it("prepends the shim dir (with delimiter) to PATH so it shadows the system binary", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        new PathShimmer(coll, { shimDir }).install();

        const m = coll.get(PATH_VAR);
        expect(m).toBeDefined();
        expect(m!.value).toBe(shimDir + path.delimiter);
        // prepend (list-like), not replace.
        expect(m!.type).toBe(3 /* EnvironmentVariableMutatorType.Prepend */);
        expect(m!.options.applyAtProcessCreation).toBe(true);
        expect(m!.options.applyAtShellIntegration).toBe(true);
    });

    it("records FLEET_SHIM_DIR so the wrapper can skip itself during lookup", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        new PathShimmer(coll, { shimDir }).install();
        expect(coll.get(FLEET_SHIM_DIR_VAR)!.value).toBe(shimDir);
    });

    it("flips installed -> true and exposes the shim dir", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        const s = new PathShimmer(coll, { shimDir });
        expect(s.installed).toBe(false);
        s.install();
        expect(s.installed).toBe(true);
        expect(s.shimDir).toBe(shimDir);
    });

    it("is idempotent: re-install never accumulates duplicate PATH segments", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        const s = new PathShimmer(coll, { shimDir });
        s.install();
        s.install();
        s.install();
        // One mutator per variable (the single-change-per-variable API rule).
        expect(coll.get(PATH_VAR)!.value).toBe(shimDir + path.delimiter);
        // PATH + FLEET_SHIM_DIR only.
        expect(coll.map.size).toBe(2);
    });

    it("re-install picks up changed reliability config in the rewritten script", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        // First install: no flags.
        new PathShimmer(coll, { shimDir }).install();
        expect(fs.readFileSync(path.join(shimDir, "claude"), "utf8")).not.toContain(
            "--allow-dangerously-skip-permissions"
        );
        // Re-install with opt-in: the on-disk script now carries the flag.
        new PathShimmer(coll, {
            shimDir,
            reliability: { claudeSkipPermissions: true },
        }).install();
        expect(fs.readFileSync(path.join(shimDir, "claude"), "utf8")).toContain(
            "--allow-dangerously-skip-permissions"
        );
    });

    it("surfaces active reliability flags via activeReliabilityFlags (never silent)", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        const off = new PathShimmer(coll, { shimDir });
        expect(off.activeReliabilityFlags).toEqual({ claude: [], codex: [] });

        const on = new PathShimmer(new MockEnvironmentVariableCollection(), {
            shimDir: makeTmpDir(),
            reliability: { claudeSkipPermissions: true },
        });
        expect(on.activeReliabilityFlags.claude).toEqual([
            "--allow-dangerously-skip-permissions",
        ]);
    });
});

describe("PathShimmer.dispose() — reversibility (invariant 6)", () => {
    it("removes the PATH + FLEET_SHIM_DIR mutators", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        const s = new PathShimmer(coll, { shimDir });
        s.install();
        expect(coll.get(PATH_VAR)).toBeDefined();

        s.dispose();
        expect(coll.get(PATH_VAR)).toBeUndefined();
        expect(coll.get(FLEET_SHIM_DIR_VAR)).toBeUndefined();
        expect(s.installed).toBe(false);
    });

    it("deletes the on-disk shim scripts", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        const s = new PathShimmer(coll, { shimDir });
        s.install();
        for (const agent of SHIMMED_AGENTS) {
            expect(fs.existsSync(path.join(shimDir, agent))).toBe(true);
        }
        s.dispose();
        for (const agent of SHIMMED_AGENTS) {
            expect(fs.existsSync(path.join(shimDir, agent))).toBe(false);
        }
    });

    it("is safe to call before install() and idempotent (double-dispose)", () => {
        const coll = new MockEnvironmentVariableCollection();
        const s = new PathShimmer(coll, { shimDir: makeTmpDir() });
        expect(() => {
            s.dispose();
            s.dispose();
        }).not.toThrow();
        expect(s.installed).toBe(false);
    });

    it("can re-install cleanly after dispose (round-trip)", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        const s = new PathShimmer(coll, { shimDir });
        s.install();
        s.dispose();
        s.install();
        expect(s.installed).toBe(true);
        expect(coll.get(PATH_VAR)!.value).toBe(shimDir + path.delimiter);
        expect(fs.existsSync(path.join(shimDir, "claude"))).toBe(true);
    });
});

// ── Claude hook installation (the load-bearing step-4 wiring) ─────────────────

const SOCK = "/run/user/1000/fleet/reporter-win.sock";

describe("hookRelayCommand() — framing matches the reporter --serve receiver", () => {
    it("emits a `claude `-tagged, newline-framed nc pipeline to the socket", () => {
        const cmd = hookRelayCommand(SOCK);
        expect(cmd).toContain("printf 'claude %s\\n'");
        expect(cmd).toContain("tr -d '\\r\\n'"); // strip embedded newlines → one line
        expect(cmd).toContain(`nc -U ${SOCK}`);
        expect(cmd).toContain("|| true"); // observer-not-owner: never break claude
    });
});

describe("claudeHooksSettings() — the --settings document", () => {
    const doc = claudeHooksSettings(SOCK) as {
        hooks: Record<string, Array<{ matcher?: string; hooks: Array<{ type: string; command: string }> }>>;
    };

    it("covers exactly the Fleet-consumed lifecycle events", () => {
        expect(Object.keys(doc.hooks).sort()).toEqual(
            ["PreToolUse", "SessionEnd", "SessionStart", "Stop", "UserPromptSubmit"].sort()
        );
    });

    it("uses the array-of-matcher-groups shape Claude expects", () => {
        for (const [, groups] of Object.entries(doc.hooks)) {
            expect(Array.isArray(groups)).toBe(true);
            expect(groups[0].hooks[0].type).toBe("command");
            expect(groups[0].hooks[0].command).toContain(`nc -U ${SOCK}`);
        }
    });

    it("gives PreToolUse a '*' matcher (fires for every tool); lifecycle events take none", () => {
        expect(doc.hooks.PreToolUse[0].matcher).toBe("*");
        expect(doc.hooks.Stop[0].matcher).toBeUndefined();
        expect(doc.hooks.UserPromptSubmit[0].matcher).toBeUndefined();
    });

    it("serializes to valid JSON a Claude --settings file can load", () => {
        const json = JSON.stringify(claudeHooksSettings(SOCK));
        expect(() => JSON.parse(json)).not.toThrow();
    });
});

describe("PathShimmer.install() with a reporter socket — writes hooks + wires --settings", () => {
    it("writes fleet-hooks.json into the shim dir pointing at the socket", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        const s = new PathShimmer(coll, { shimDir, reporterSocket: SOCK });
        s.install();

        const hooksFile = path.join(shimDir, CLAUDE_HOOKS_FILE);
        expect(s.claudeHooksFile).toBe(hooksFile);
        expect(fs.existsSync(hooksFile)).toBe(true);
        const parsed = JSON.parse(fs.readFileSync(hooksFile, "utf8"));
        expect(parsed.hooks.Stop[0].hooks[0].command).toContain(`nc -U ${SOCK}`);
    });

    it("does NOT write a hooks file when no reporter socket is configured (pass-through)", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        const s = new PathShimmer(coll, { shimDir });
        s.install();
        expect(s.claudeHooksFile).toBeUndefined();
        expect(fs.existsSync(path.join(shimDir, CLAUDE_HOOKS_FILE))).toBe(false);
    });

    it("removes the hooks file on dispose (reversibility)", () => {
        const coll = new MockEnvironmentVariableCollection();
        const shimDir = makeTmpDir();
        const s = new PathShimmer(coll, { shimDir, reporterSocket: SOCK });
        s.install();
        const hooksFile = path.join(shimDir, CLAUDE_HOOKS_FILE);
        expect(fs.existsSync(hooksFile)).toBe(true);
        s.dispose();
        expect(fs.existsSync(hooksFile)).toBe(false);
    });
});

describePosix("generated claude wrapper with hooks — executed under /bin/sh", () => {
    it("prepends --settings <hooksFile> ahead of the user's args (Fleet branch)", () => {
        const shimDir = makeTmpDir();
        const realDir = makeTmpDir();
        makeFakeBinary(realDir, "claude");
        const hooksFile = path.join(shimDir, CLAUDE_HOOKS_FILE);
        // Write the claude wrapper WITH a hooks file (3rd arg).
        fs.mkdirSync(shimDir, { recursive: true });
        const shimPath = path.join(shimDir, "claude");
        fs.writeFileSync(shimPath, shimScript("claude", undefined, hooksFile), { mode: 0o755 });
        fs.chmodSync(shimPath, 0o755);

        const out = runShim(shimPath, {
            pathDirs: [shimDir, realDir],
            shimDir,
            env: { [FLEET_SESSION_ID_VAR]: "win-1" },
            args: ["-p", "hello"],
        });

        expect(out.status).toBe(0);
        expect(out.stdout).toContain("REAL_CLAUDE");
        // The exact forwarded argv: --settings <hooksFile> then the user's args.
        const argLines = out.stdout.split("\n").filter(l => l.startsWith("ARG:")).map(l => l.slice(4));
        expect(argLines).toEqual(["--settings", hooksFile, "-p", "hello"]);
    });

    it("does NOT add --settings in pass-through (no FLEET_SESSION_ID)", () => {
        const shimDir = makeTmpDir();
        const realDir = makeTmpDir();
        makeFakeBinary(realDir, "claude");
        const hooksFile = path.join(shimDir, CLAUDE_HOOKS_FILE);
        fs.mkdirSync(shimDir, { recursive: true });
        const shimPath = path.join(shimDir, "claude");
        fs.writeFileSync(shimPath, shimScript("claude", undefined, hooksFile), { mode: 0o755 });
        fs.chmodSync(shimPath, 0o755);

        const out = runShim(shimPath, {
            pathDirs: [shimDir, realDir],
            shimDir,
            // No FLEET_SESSION_ID → pure pass-through.
            args: ["-p", "hello"],
        });
        const argLines = out.stdout.split("\n").filter(l => l.startsWith("ARG:")).map(l => l.slice(4));
        expect(argLines).toEqual(["-p", "hello"]); // verbatim, no --settings
    });
});
