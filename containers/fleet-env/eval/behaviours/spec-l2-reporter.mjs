// L2 Reporter behaviours (SPEC area 22-reporter.md) — end-to-end through the REAL
// in-env reporter `--serve` unix socket (/tmp/fleet-reporter.sock) to the host Hub.
//
// These do NOT touch the DESKTOP multiplexer or any host supervisor — they drive
// synthetic claude/codex hook FRAMES straight at the env's reporter socket (the same
// path the in-container claude shim uses: `printf 'claude %s\n' '<json>' | nc -U`),
// then assert the resulting Hub session state via the `fleet ls --once` CLI. This is
// the determinism-over-realism trigger the SPEC blesses (cf. agent.waitingState /
// L2.RPT.070): a controlled frame exercises the SAME socket → S15/S16 adapters → WS
// phone-home → Hub registry → CLI render pipeline a real run would, with none of the
// flakiness of driving a live claude into a particular state.
//
// Hub-dependent behaviours SKIP cleanly (never hard-fail) when the `fleet` CLI is
// absent or the env's session never registered — those are environmental, not
// regressions (identical gate to behaviours/agentInput.mjs). The socket itself lives
// in every booted env, so `needs:[]` (env.exec / docker exec is always available; it
// is NOT a bridge cap, so it must NOT be listed in `needs:` or the runner would SKIP).
//
// One new file; existing behaviour files are untouched. See _contract.mjs for shape.

import { execSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ─── Host-side Hub query (replicated from behaviours/agentInput.mjs) ────────────
// The reporter inside each env phones home to ws://HOST:51777 and registers a
// session titled by FLEET_SERVER_ID (== env.id). We read the live snapshot off the
// host with the `fleet` CLI; `fleet ls --once` prints one row per session:
//   [<state>]<unread> <title>  (<n> run[s])[ <urgency>]
// state ∈ {working,waiting,idle,done,error,dead}.

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = join(HERE, "..", "..", "..", "..");
const CLI_CANDIDATES = [
  process.env.FLEET_CLI,
  join(REPO_ROOT, "target", "debug", "fleet"),
  join(REPO_ROOT, "target", "release", "fleet"),
].filter(Boolean);

function fleetCli() {
  for (const p of CLI_CANDIDATES) if (p && (p === "fleet" || existsSync(p))) return p;
  return null;
}

function hubSnapshot() {
  const cli = fleetCli();
  if (!cli) return null;
  try {
    return execSync(`${JSON.stringify(cli)} ls --once`, {
      encoding: "utf8",
      timeout: 8000,
      stdio: ["ignore", "pipe", "ignore"],
    });
  } catch {
    return null;
  }
}

function sessionLineFor(snapshot, sessionTitle) {
  if (!snapshot) return null;
  for (const raw of snapshot.split("\n")) {
    const line = raw.trim();
    if (!line.startsWith("[")) continue;
    if (line.includes(sessionTitle)) return line;
  }
  return null;
}

function stateOf(line) {
  if (!line) return null;
  const m = line.match(/^\[(\w+)\]/);
  return m ? m[1] : null;
}

// Run count from a session row, e.g. "(2 runs)" → 2, "(1 run)" → 1, none → 0.
function runCountOf(line) {
  if (!line) return 0;
  const m = line.match(/\((\d+) runs?\)/);
  return m ? Number(m[1]) : 0;
}

async function pollHub(sessionTitle, match, { ms = 30000, every = 1000 } = {}) {
  const t0 = Date.now();
  const seen = [];
  let lastLine = null;
  while (Date.now() - t0 < ms) {
    const snap = hubSnapshot();
    const line = sessionLineFor(snap, sessionTitle);
    if (line) {
      lastLine = line;
      const st = stateOf(line);
      if (st && seen[seen.length - 1] !== st) seen.push(st);
      if (match(line, st)) return { ok: true, seen, line };
    }
    await sleep(every);
  }
  return { ok: false, seen, line: lastLine };
}

// Standard Hub-availability gate shared by every Hub-dependent behaviour here.
// Returns { skip } (a BehaviourResult to return) or { cli, boot } when ready.
async function hubGate(env) {
  const cli = fleetCli();
  if (!cli) {
    return {
      skip: {
        pass: false,
        skipped: "Hub `fleet` CLI not found (target/debug/fleet) — start the Hub (see test.sh)",
        detail: "skipped: no fleet CLI to query the Hub",
      },
    };
  }
  const boot = await pollHub(env.id, () => true, { ms: 15000, every: 1000 });
  if (!boot.ok) {
    return {
      skip: {
        pass: false,
        skipped: `Hub session "${env.id}" not found (Hub down or reporter not phoned home)`,
        detail: "skipped: env's session is not registered on the Hub",
        evidence: { sessionTitle: env.id, cli },
      },
    };
  }
  return { cli, boot };
}

// Send one framed claude/codex hook payload to the env's REAL reporter socket. This
// is byte-for-byte the relay the in-container shim uses (`printf '<tag> %s\n' '<json>'
// | nc -U <sock>`), so it travels the exact production path. `|| true` keeps a flaky
// nc from failing the exec.
function sendFrame(env, obj, tag = "claude") {
  return env.exec(
    `printf '${tag} %s\\n' '${JSON.stringify(obj)}' | timeout 2 nc -N -U /tmp/fleet-reporter.sock 2>/dev/null || true`,
  );
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ── L2.RPT.064 — the reporter socket is owner-only (mode 0600) ────────────────
  {
    id: "reporter.socketMode0600",
    specId: "L2.RPT.064",
    title: "Reporter --serve socket is restricted to owner-only (0600)",
    tags: ["reporter", "security"],
    // env.exec (docker exec) is always available — it is NOT a bridge cap, so we do
    // NOT list it in needs (that would make the runner SKIP every env).
    needs: [],
    rationale: `
WHAT: Stats the live reporter unix socket inside a booted env
(/tmp/fleet-reporter.sock, bound by \`fleet-reporter --serve\`) and asserts its
permission bits are exactly 0600 (owner read/write only), via
\`env.exec("stat -c '%a' /tmp/fleet-reporter.sock")\`. It first confirms the socket
file exists and is a socket, then reads its mode.

WHY THIS IS THE EXPECTED OUTCOME: \`serve_unix\` calls \`restrict_socket_perms\` to
chmod the socket to 0o600 immediately after binding. A hook frame written to this
socket can mutate THIS window's reported agent state (working/waiting/idle/done),
so the socket is a local trust boundary: only the socket's owner (the coder user
running the agent + reporter) may connect. 0600 means no other local user can open
it to inject spoofed frames. Any looser mode (group/other access) would let a
co-tenant on the box impersonate the agent's lifecycle to the Hub.

WHY IT MATTERS: This is defence-in-depth on the one writable surface of the reporter.
The Rust sets 0o600, but nothing else asserts the LIVE socket actually ends up
restricted (a refactor of \`restrict_socket_perms\`, an umask change in the
entrypoint, or a bind that races the chmod could silently widen it). If this goes
red, a future reader knows the reporter's local trust boundary leaked — spoofable
agent state — not that the agent or Hub regressed. We assert via a real container
stat so it reflects the bound socket, not the Rust constant.`,
    async run(env) {
      // Confirm the socket exists and is actually a socket before reading its mode.
      const sock = "/tmp/fleet-reporter.sock";
      const kind = env.exec(`test -S ${sock} && echo socket || echo missing`);
      if (kind !== "socket") {
        return {
          pass: false,
          skipped: `reporter socket ${sock} not present (is --serve running in this env?)`,
          detail: `skipped: ${sock} is not a bound unix socket (saw "${kind || "<empty>"}")`,
          evidence: { sock, kind },
        };
      }
      // GNU stat (busybox/coreutils both honour -c '%a' for the octal mode).
      const mode = env.exec(`stat -c '%a' ${sock} 2>/dev/null || stat -f '%Lp' ${sock}`);
      const pass = mode === "600";
      return {
        pass,
        detail: pass
          ? `${sock} is mode 0600 (owner-only) as restrict_socket_perms requires`
          : `${sock} mode is "${mode}", expected "600" (owner-only) — socket trust boundary widened`,
        evidence: { sock, mode },
      };
    },
  },

  // ── L2.RPT.010/011 — UserPromptSubmit→working then Stop→idle on the Hub ───────
  {
    id: "reporter.promptWorkingThenStopIdle",
    specId: "L2.RPT.011",
    title: "claude UserPromptSubmit drives the Hub session working, Stop drives it idle",
    tags: ["reporter", "hub", "agent"],
    needs: [],
    rationale: `
WHAT: Drives the canonical S15 turn lifecycle end-to-end through the real reporter
socket and asserts the Hub session reflects it. It sends a synthetic
\`claude {hook_event_name:"UserPromptSubmit", session_id:<unique>}\` frame to the
env's /tmp/fleet-reporter.sock, polls the host Hub (\`fleet ls --once\`) until the
env's session row reads \`[working]\`, then sends a real turn-boundary
\`Stop\` (stop_hook_active:false, no completion marker) and polls until the row reads
\`[idle]\`. The Stop is sent regardless (it is also the cleanup) so the session is left
quiescent.

WHY THIS IS THE EXPECTED OUTCOME: A UserPromptSubmit is the canonical activity signal
— the ClaudeAdapter maps it to State::Working (Inferred), anchored on the verbatim
session_id, and the reporter phones an UpsertRun(Working) home, which the CLI renders
\`[working]\`. A bare Stop (no completion marker, not fired from inside a Stop hook) is
THE turn-boundary completion signal for the native UI; absent a marker the adapter is
conservatively Idle (D9: never over-claim Done), so the row settles to \`[idle]\`. This
is the proven, deterministic counterpart to agent.waitingState's PreToolUse path —
the same socket, adapters, phone-home and render, but exercising the working/idle spine
instead of the inferred-waiting one.

WHY IT MATTERS: working→idle is the backbone of agent observability; if it regresses,
every Fleet user's session list goes stale or wrong (an active agent shows idle, or a
finished one is stuck working). The Rust unit test
\`process_claude_prompt_then_stop_drives_working_then_idle\` proves the adapter
mapping in isolation; THIS guards the full socket→Hub→CLI path the unit test cannot
reach. Determinism over realism: we inject the exact frames rather than driving a live
claude (whose timing/version would make catching the brief working window flaky). Hub
absence SKIPs cleanly — environmental, not a regression.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;
      const startState = stateOf(gate.boot.line);
      const sid = `rpt-pi-${env.id}`;

      // UserPromptSubmit → the adapter must mint a Working run for this session.
      sendFrame(env, {
        hook_event_name: "UserPromptSubmit",
        session_id: sid,
        cwd: "/home/coder/project",
      });
      const working = await pollHub(env.id, (_l, st) => st === "working", {
        ms: 30000,
        every: 1000,
      });

      // Bare Stop (real turn boundary, no marker) → Idle. Always sent (also cleanup).
      sendFrame(env, {
        hook_event_name: "Stop",
        session_id: sid,
        cwd: "/home/coder/project",
        stop_hook_active: false,
      });
      const idle = await pollHub(env.id, (_l, st) => st === "idle", {
        ms: 30000,
        every: 1000,
      });

      const pass = working.ok && idle.ok;
      return {
        pass,
        detail: pass
          ? `Hub session "${env.id}": UserPromptSubmit→[working] then Stop→[idle]` +
            ` (states: ${[...working.seen, ...idle.seen].join("->")})`
          : `working/idle not both observed (working=${working.ok}, idle=${idle.ok};` +
            ` states seen: ${JSON.stringify([...new Set([...working.seen, ...idle.seen])])})`,
        evidence: {
          sessionTitle: env.id,
          sid,
          startState,
          statesSeen: [...new Set([...working.seen, ...idle.seen])],
          finalLine: idle.line || working.line || gate.boot.line,
        },
      };
    },
  },

  // ── L2.RPT.074 — two distinct sessions on one socket → ≥2 distinct runs ───────
  {
    id: "reporter.twoSessionsTwoRuns",
    specId: "L2.RPT.074",
    title: "Two distinct claude sessions on one socket keep distinct runs on the Hub",
    tags: ["reporter", "hub", "agent"],
    needs: [],
    rationale: `
WHAT: Verifies the reporter multiplexes independent claude invocations under one
session shell without cross-contaminating their runs. It sends two UserPromptSubmit
frames for two DISTINCT session_ids (…-a and …-b) to the same env reporter socket,
then polls the host Hub until the env's session row reports at least 2 runs
(\`(2 runs)\` / \`(N runs)\` with N≥2). It then resolves the first session with a Stop
and asserts the run count does NOT drop — the second run is untouched — before
resolving the second (cleanup) so the session is left quiescent.

WHY THIS IS THE EXPECTED OUTCOME: One reporter \`--serve\` hosts many claude sessions
arriving on many short-lived nc connections; the shared Receiver keys adapter state per
session_id and mints a distinct Fleet run-id per session
(\`claude:<session>:run-<n>\`). Two different session_ids must therefore become two
distinct runs aggregated under the one env session, so the rolled-up row shows ≥2 runs.
Resolving session A to idle must leave session B's run independently intact — the rollup
counts both runs regardless of either's state — proving no cross-session state bleed.

WHY IT MATTERS: Concurrent agents are a first-class Fleet scenario (split panes, sub-tasks,
parallel windows). If session keying regressed so a second session reused the first's run
(or clobbered its state), users would see one agent mislabelled with another's lifecycle,
or a lost run. The Rust test \`process_two_sessions_get_distinct_runs\` proves distinct
run-ids at the adapter; THIS is the end-to-end guard that the two runs actually surface and
stay independent on the Hub. Determinism over realism: injected frames, not two live claudes.
Hub absence SKIPs cleanly.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;
      const baseRuns = runCountOf(gate.boot.line);
      const sidA = `rpt-2s-${env.id}-a`;
      const sidB = `rpt-2s-${env.id}-b`;

      // Two distinct sessions on the one socket → two distinct runs.
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: sidA, cwd: "/home/coder/project" });
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: sidB, cwd: "/home/coder/project" });

      // Poll until the rolled-up row reports at least baseRuns+2 runs (both arrived).
      const want = baseRuns + 2;
      const both = await pollHub(env.id, (line) => runCountOf(line) >= want, {
        ms: 30000,
        every: 1000,
      });
      const runsAfterBoth = runCountOf(both.line || gate.boot.line);

      // Resolve A → idle; B's run must remain (count must not fall below want).
      sendFrame(env, { hook_event_name: "Stop", session_id: sidA, cwd: "/home/coder/project", stop_hook_active: false });
      const stillBoth = await pollHub(env.id, (line) => runCountOf(line) >= want, {
        ms: 15000,
        every: 1000,
      });
      const runsAfterResolveA = runCountOf(stillBoth.line || both.line || gate.boot.line);

      // Cleanup: resolve B too so the session is left quiescent.
      sendFrame(env, { hook_event_name: "Stop", session_id: sidB, cwd: "/home/coder/project", stop_hook_active: false });

      const pass = both.ok && runsAfterResolveA >= want;
      return {
        pass,
        detail: pass
          ? `Hub session "${env.id}": two sessions → ${runsAfterBoth} runs; resolving A left ${runsAfterResolveA} (≥${want}) — independent runs`
          : `two distinct runs not observed/kept (baseRuns=${baseRuns}, want≥${want},` +
            ` afterBoth=${runsAfterBoth}, afterResolveA=${runsAfterResolveA})`,
        evidence: {
          sessionTitle: env.id,
          sidA, sidB,
          baseRuns, want, runsAfterBoth, runsAfterResolveA,
          finalLine: stillBoth.line || both.line || gate.boot.line,
        },
      };
    },
  },
];
