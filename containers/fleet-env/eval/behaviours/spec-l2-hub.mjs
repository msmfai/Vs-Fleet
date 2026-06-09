// L2 Hub-state behaviours (SPEC area 21-hub-state.md) — the deterministic E2E slice
// that the eval harness CAN drive through real containers.
//
// THE DETERMINISTIC PATH (the only Hub path this harness can drive E2E):
//   Each env runs the real Fleet reporter `--serve`, listening on the in-container
//   unix socket /tmp/fleet-reporter.sock. The reporter registers exactly ONE Hub
//   session, titled by FLEET_SERVER_ID (== env.id), and phones it home to the
//   host-side Hub (ws://HOST:51777). Every claude hook frame we inject on that
//   socket is routed to the ClaudeAdapter, which owns one state machine PER claude
//   `session_id` and mints a distinct Fleet run-id per session_id
//   (`claude:<session_id>:run-N`) — i.e. each distinct injected `session_id`
//   becomes a separate RUN under the env's single Hub session. The env session's
//   bracketed state in `fleet ls --once` is therefore the live ROLLUP across those
//   runs. This is exactly the channel `agent.waitingState` proves; here we use it
//   to pin the run-upsert → recompute-rollup path and the rollup PRECEDENCE
//   contract end-to-end through the real reporter + Hub + CLI.
//
//   Claude hook → reporter run-state map (crates/fleet-reporter/src/claude.rs):
//     UserPromptSubmit | PreToolUse                → working  (a live run)
//     Stop (plain)                                 → idle
//     Stop (task_complete:true, !stop_hook_active) → done
//     SessionEnd                                   → dead
//     PreToolUse with NO following Stop, after the S16 debounce tick → waiting
//                                                    (inferred; the only state that pings)
//
// WHAT IS LEFT TODO (and why) — see the spec file's status lines:
//   * The session/run merge edges, the added-vs-updated Event distinction, seq/epoch
//     reclaim, and the snapshot/Lagged atomicity (L2.HUB.001-002, 004-009, 016,
//     031-038) need a RAW WS client to the Hub (`rawWs`) to inject protocol-level
//     `session.upsert`/`run.upsert`/`*.remove` deltas with stamps AND a second
//     `subscribe` socket to read `Event::type_name`. The eval harness has neither —
//     `fleet ls --once` only renders the merged rollup, it cannot observe per-event
//     broadcasts or distinguish added/updated. These stay Rust-unit-tested.
//   * Persist replay + dead-reap grace + session TTL/GC (L2.HUB.017-030) need
//     hubRestart / hubGc / persist — restarting the real Hub binary or driving
//     `HubState::gc(now,…)` with an injected clock. The harness does not own the
//     Hub's lifecycle or clock.
//   * Error/Dead precedence (L2.HUB.011 error leg, 013) need an `error` run, which
//     the claude adapter does not emit from any injectable frame, and a stable
//     `dead` run (SessionEnd terminates the series) — too risky to assert green.
//   * no-network split (L2.HUB.042) needs the no-network scenario AND a live bridge
//     to prove "editor drivable while Hub unreachable"; left to that scenario.
//
// All implemented behaviours here gate on the host-side Hub + `fleet` CLI exactly
// like agent.claudeRuns/agent.waitingState: absent ⇒ clean SKIP, never a hard fail.
// They do NOT require claude auth (pure socket injection), so they are deterministic.

import { execSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ─── Host-side Hub query (replicates agentInput.mjs's private helpers; that file
// must not be edited, and these are the proven idioms) ─────────────────────────
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

// Send one framed claude hook line to the env's REAL reporter --serve socket. This
// is byte-for-byte the channel `fleet init`'s installed hooks write, and the same
// injection agent.waitingState uses. `nc -N -U` closes after the write so the
// reporter sees a complete framed line.
function sendFrame(env, obj) {
  return env.exec(
    `printf 'claude %s\\n' '${JSON.stringify(obj)}' | timeout 2 nc -N -U /tmp/fleet-reporter.sock 2>/dev/null || true`,
  );
}

// Shared Hub-availability gate: CLI present + the env's session already registered
// (the reporter registers it on boot, runs-empty, at [idle]). Returns either
// { skip } (caller returns it verbatim) or { startLine, startState }.
async function hubGate(env, sessionTitle) {
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
  const boot = await pollHub(sessionTitle, () => true, { ms: 15000, every: 1000 });
  if (!boot.ok) {
    return {
      skip: {
        pass: false,
        skipped: `Hub session "${sessionTitle}" not found (Hub down or reporter not phoned home)`,
        detail: "skipped: env's session is not registered on the Hub",
        evidence: { sessionTitle, cli },
      },
    };
  }
  return { startLine: boot.line, startState: stateOf(boot.line) };
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ── L2.HUB.003 — a run.upsert on a known session adds the run and re-rolls ──────
  // Deterministic socket variant: a single injected UserPromptSubmit creates one
  // working RUN under the env's (idle) Hub session; the session rollup must flip
  // idle → working. Then a plain Stop resolves the run so the env is left clean.
  {
    id: "hub.runUpsertRecomputesRollup",
    specId: "L2.HUB.003",
    title: "run.upsert flips the env Hub session rollup idle → working",
    tags: ["hub", "agent"],
    isolation: "shared",
    needs: [],
    rationale: `
WHAT: Drives the run-upsert → session-rollup-recompute path end-to-end through the
real stack, deterministically and without claude auth. The env boots with its Hub
session registered runs-empty at [idle]. We inject ONE claude \`UserPromptSubmit\`
frame on the env's real reporter \`--serve\` socket (/tmp/fleet-reporter.sock) under a
fresh \`session_id\`. The ClaudeAdapter mints a new run (\`claude:<sid>:run-N\`) in state
\`working\` and the reporter sends a \`run.upsert\` to the Hub, which records the run AND
recomputes the session rollup. We assert the env session's bracketed state in
\`fleet ls --once\` flips idle → working. We then inject a plain \`Stop\` (→ idle) so the
run resolves and the env is left as we found it.

WHY THIS IS THE EXPECTED OUTCOME: per the merge contract, a run delta both records
the run and re-rolls its session in one apply (MergeEngine::upsert_run emits the run
event AND a session.updated, calling recompute_rollups). With exactly one run present
and that run \`working\`, the rollup max over {working} is \`working\` — so the env
session, previously idle (no runs), must read \`[working]\`. UserPromptSubmit→working
is the canonical S15 activity mapping; injecting it on the socket exercises the SAME
adapter → reporter → WS phone-home → Hub merge → CLI render path a real prompt would,
but deterministically (no model call, no auth, no timing flake).

WHY IT MATTERS: this is the single most fundamental Hub transition — "an agent run
appeared and the session is now active". If a refactor breaks run.upsert's
recompute, or the reporter stops emitting the run on UserPromptSubmit, or the CLI
stops rendering the rolled-up state, every higher-level attention signal (working/
waiting badges, the rail) silently dies. The Rust units pin upsert_run's recompute in
isolation; this is the only E2E guard that the whole socket→Hub→CLI rollup pipeline
actually lights a session up on first activity. SKIPs cleanly (never fails) when the
Hub/CLI is absent or the session never registered — those are environmental.`,
    async run(env) {
      const sessionTitle = env.id;
      const gate = await hubGate(env, sessionTitle);
      if (gate.skip) return gate.skip;
      const startState = gate.startState; // expected: idle (runs-empty boot session)

      const sid = `hub003-${env.id}-${Date.now()}`;
      sendFrame(env, {
        hook_event_name: "UserPromptSubmit",
        session_id: sid,
        cwd: "/home/coder/project",
      });

      const working = await pollHub(
        sessionTitle,
        (_l, st) => st === "working",
        { ms: 30000, every: 750 },
      );

      // Always resolve the injected run so the env session returns to idle.
      sendFrame(env, {
        hook_event_name: "Stop",
        session_id: sid,
        cwd: "/home/coder/project",
        stop_hook_active: false,
      });

      return {
        pass: working.ok,
        detail: working.ok
          ? `env Hub session "${sessionTitle}": ${startState}→working after one injected run.upsert (UserPromptSubmit)`
          : `rollup did not reach "working" after run.upsert (states seen: ${JSON.stringify([...working.seen])})`,
        evidence: { sessionTitle, startState, statesSeen: [...new Set(working.seen)], finalLine: working.line || gate.startLine },
      };
    },
  },

  // ── L2.HUB.012 — Done is kept DISTINCT from Idle in the rollup (D9) ─────────────
  // Two-step, single env: a Stop+task_complete run rolls to [done]; a plain Stop run
  // rolls to [idle]. The two produce DIFFERENT bracketed tokens — never collapsed.
  {
    id: "hub.doneDistinctFromIdle",
    specId: "L2.HUB.012",
    title: "Done renders distinct from Idle on the env Hub session rollup (D9)",
    tags: ["hub", "agent"],
    isolation: "shared",
    needs: [],
    rationale: `
WHAT: Verifies D9 (Done ≠ Idle) end-to-end through the real reporter + Hub + CLI,
deterministically. Under one fresh injected \`session_id\` we drive a turn to
completion two ways and read the env session's bracketed rollup token each time:
(1) \`UserPromptSubmit\` then \`Stop{task_complete:true}\` → the run lands \`done\`, so the
single-run rollup renders \`[done]\`; (2) a second \`UserPromptSubmit\` then a PLAIN
\`Stop\` → the run lands \`idle\`, so the rollup renders \`[idle]\`. We assert the two
observed tokens are literally different strings ("done" vs "idle"), i.e. the wire
state is never collapsed. (We re-poll for \`done\` first, then for \`idle\`, on the SAME
env session; both are reached off \`fleet ls --once\`.)

WHY THIS IS THE EXPECTED OUTCOME: the claude adapter maps a \`Stop\` carrying a
completion marker (task_complete / subtype success, and NOT stop_hook_active) to
\`State::Done\`, and a bare \`Stop\` to \`State::Idle\` (claude.rs). State has distinct
kebab wire tokens "done" vs "idle" (state_wire_tokens) and Done ranks strictly above
Idle in the rollup (done_distinct_and_ranks_above_idle), so a one-run session reads
\`[done]\` then \`[idle]\` across the two turns. "Task complete" must be visually
distinguishable from "awaiting the next prompt" — that is the entire point of D9.

WHY IT MATTERS: if Done were ever folded into Idle (a tempting simplification — both
mean "not currently working"), users would lose the "this finished" vs "this is
waiting for me to drive it again" distinction that the inbox is built on. This is the
only E2E guard that the done/idle SPLIT survives the full Stop-frame → adapter →
rollup → CLI render path; the Rust units pin the tokens and the rank in isolation.
SKIPs cleanly when the Hub/CLI is absent.`,
    async run(env) {
      const sessionTitle = env.id;
      const gate = await hubGate(env, sessionTitle);
      if (gate.skip) return gate.skip;

      const sid = `hub012-${env.id}-${Date.now()}`;
      const cwd = "/home/coder/project";

      // Step 1 — drive a turn to DONE (Stop carries a completion marker).
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: sid, cwd });
      await sleep(400);
      sendFrame(env, {
        hook_event_name: "Stop",
        session_id: sid,
        cwd,
        task_complete: true,
        stop_hook_active: false,
      });
      const doneR = await pollHub(sessionTitle, (_l, st) => st === "done", { ms: 30000, every: 750 });

      // Step 2 — drive a fresh turn to IDLE (a plain Stop, no completion marker).
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: sid, cwd });
      await sleep(400);
      sendFrame(env, { hook_event_name: "Stop", session_id: sid, cwd, stop_hook_active: false });
      const idleR = await pollHub(sessionTitle, (_l, st) => st === "idle", { ms: 30000, every: 750 });

      const doneTok = stateOf(doneR.line);
      const idleTok = stateOf(idleR.line);
      const distinct = doneR.ok && idleR.ok && doneTok === "done" && idleTok === "idle" && doneTok !== idleTok;

      return {
        pass: distinct,
        detail: distinct
          ? `env Hub session "${sessionTitle}" rendered "[done]" then "[idle]" — distinct rollup tokens (D9 holds)`
          : `done/idle not both observed as distinct tokens (done="${doneTok}", idle="${idleTok}"; doneSeen=${JSON.stringify([...doneR.seen])}, idleSeen=${JSON.stringify([...idleR.seen])})`,
        evidence: {
          sessionTitle,
          doneToken: doneTok,
          idleToken: idleTok,
          doneStatesSeen: [...new Set(doneR.seen)],
          idleStatesSeen: [...new Set(idleR.seen)],
        },
      };
    },
  },

  // ── L2.HUB.010 — Waiting wins the rollup over every other state ─────────────────
  // A concurrent working run (one session_id) plus an inferred-waiting run (a
  // PreToolUse-without-Stop on a second session_id) must roll the env session up to
  // [waiting] — the only state that pings — regardless of the live working run.
  {
    id: "hub.waitingWinsRollup",
    specId: "L2.HUB.010",
    title: "Waiting dominates the env Hub session rollup over a concurrent working run",
    tags: ["hub", "agent"],
    isolation: "shared",
    needs: [],
    rationale: `
WHAT: Pins the ping-precedence invariant (Waiting outranks every other state) end-to-
end through the real reporter + Hub + CLI, deterministically. Under the env's single
Hub session we create TWO concurrent runs via the reporter socket: run A is \`working\`
(a \`UserPromptSubmit\` under session_id "work-…" with no Stop, so it stays live), and
run B is an inferred \`waiting\` (a \`PreToolUse\`-without-\`Stop\` under session_id
"wait-…" — the S16 infer adapter arms on it and, after serve's debounce tick fires
with no follow-up, emits \`waiting\`). With both runs present the session rollup must
escalate to \`[waiting]\`. We assert \`fleet ls --once\` shows \`[waiting]\` for the env
session, then resolve BOTH runs (a Stop to each session_id) so the env is left clean.

WHY THIS IS THE EXPECTED OUTCOME: \`fleet_protocol::rollup::state_rank\` puts
Waiting(5) strictly above Working(3) (waiting_beats_working), and Waiting is the ONLY
state for which State::pings() is true (only_waiting_pings) — so a session with a live
working run AND a blocked-on-approval run must badge as the blocked one. If the louder
"there's a busy run" signal masked the quieter "a run is blocked waiting for you", the
user would never be told an agent is stuck. The waiting run here is the genuinely
INFERRED path (PreToolUse-without-Stop, Confidence::Inferred), the heuristic Fleet's
whole ping rests on, surfaced through serve's real debounce TICK — not a unit stub.

WHY IT MATTERS: this is THE attention-precedence guarantee. A refactor that reorders
state_rank, breaks the S16 debounce tick, or lets a concurrent working run win the
rollup would silently swallow approval pings while a working run is also live — the
single worst Fleet failure (the agent is blocked and nobody is told). The Rust units
pin the rank and the infer machine separately; this is the only E2E guard that a
CONCURRENT working+waiting session rolls up to waiting across the full socket → both
adapters → Hub → CLI path. SKIPs cleanly when the Hub/CLI is absent.`,
    async run(env) {
      const sessionTitle = env.id;
      const gate = await hubGate(env, sessionTitle);
      if (gate.skip) return gate.skip;

      const cwd = "/home/coder/project";
      const workSid = `hub010-work-${env.id}-${Date.now()}`;
      const waitSid = `hub010-wait-${env.id}-${Date.now()}`;

      // Run A: a live WORKING run (UserPromptSubmit, no Stop → stays working).
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: workSid, cwd });
      // Run B: a PreToolUse-without-Stop → the S16 infer adapter emits waiting after
      // its debounce tick.
      sendFrame(env, {
        hook_event_name: "PreToolUse",
        session_id: waitSid,
        tool_name: "Bash",
        tool_use_id: "toolu_hub010",
        cwd,
      });

      // The rollup must escalate to waiting despite the concurrent working run.
      const waited = await pollHub(sessionTitle, (_l, st) => st === "waiting", { ms: 45000, every: 1000 });

      // Resolve BOTH runs so the env session is left clean.
      sendFrame(env, { hook_event_name: "Stop", session_id: waitSid, cwd, stop_hook_active: false });
      sendFrame(env, { hook_event_name: "Stop", session_id: workSid, cwd, stop_hook_active: false });

      return {
        pass: waited.ok,
        detail: waited.ok
          ? `env Hub session "${sessionTitle}" rolled up to "[waiting]" with a concurrent working run present (waiting dominates; states: ${[...waited.seen].join("->")})`
          : `"waiting" did not dominate the rollup within budget (states seen: ${JSON.stringify([...waited.seen])})`,
        evidence: {
          sessionTitle,
          startState: gate.startState,
          statesSeen: [...new Set(waited.seen)],
          finalLine: waited.line || gate.startLine,
          workRun: workSid,
          waitRun: waitSid,
        },
      };
    },
  },
];
