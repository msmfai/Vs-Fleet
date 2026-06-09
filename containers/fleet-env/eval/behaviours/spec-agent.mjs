// SPEC area 19 — Agent in env (claude lifecycle → Hub). Deterministic implementations.
//
// These behaviours implement the SPEC/19-agent-in-env.md entries that are testable
// purely by injecting CONTROLLED hook frames into the env's REAL reporter `--serve`
// Unix socket (/tmp/fleet-reporter.sock) and asserting the resulting Hub state via
// the host-side `fleet ls --once` CLI — exactly the path the proven `agent.waitingState`
// behaviour (in agentInput.mjs) already exercises end-to-end. None of them run a real
// `claude`, so they need NO container auth (`env.claudeAuthed`) — only the Hub gate.
//
// WHY frame injection rather than a real claude (per SPEC §"Determinism over realism"):
// the S15/S16 Claude adapters' state mappings are environment-independent, but a real
// headless claude run is flaky (version/permission-mode dependent). Injecting the exact
// `claude <json>` frame travels the SAME socket → S15+S16 adapters → Hub plumbing →
// `fleet ls` render that a real run would, so each is a true end-to-end wire test of one
// mapping, deterministic in timing.
//
// The frame→state mappings asserted here are the ones baked into the reporter's serve
// adapter (crates/fleet-reporter/src/claude_infer.rs, which `--serve` drives):
//   UserPromptSubmit | PreToolUse        → working   (urgency none)
//   Stop (bare)                          → idle
//   Stop (reason:"completed")            → done
//   SessionEnd                           → dead
//   SubagentStop                         → liveness only (stays working)
//   PreToolUse-without-Stop + debounce   → waiting + [approval] urgency
//   run minted once per NEW session_id   (run_counter only increments on first sighting)
//
// Every behaviour carries ONLY the Hub-availability gate (no claude, no auth). Each
// resolves any pending inference (sends a final Stop) so the env is left non-blocked.
//
// We reuse the proven helpers from agentInput.mjs (fleetCli / pollHub / sessionLineFor /
// stateOf) verbatim rather than re-deriving the Hub-query plumbing.

import { execSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ─── Host-side Hub query (replicated from behaviours/agentInput.mjs) ────────────
// `fleet ls --once` prints one row per session: "[<state>]<unread> <title>  (<N>
// run[s])  [<urgency>]". The env's session title == FLEET_SERVER_ID == env.id.

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

// Parse the "(N runs)" count off a session row; null when no count is present.
function runCountOf(line) {
  if (!line) return null;
  const m = line.match(/\((\d+)\s+runs?\)/);
  return m ? Number(m[1]) : null;
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

// The Hub gate every entry shares: a reachable `fleet` CLI + the env's session row
// already registered (the reporter phones home at boot). Returns either a `skip`
// result object or `{ ok:true, boot }`.
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
  return { ok: true, boot };
}

// Send one controlled `claude <json>` frame to the env's REAL reporter --serve socket.
// The `claude ` tag selects the S15+S16 Claude adapters. Fire-and-forget (bounded
// timeout so a slow socket never hangs the harness).
function sendFrame(env, obj) {
  return env.exec(
    `printf 'claude %s\\n' '${JSON.stringify(obj)}' | timeout 2 nc -N -U /tmp/fleet-reporter.sock 2>/dev/null || true`,
  );
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ── L1.AGENT.002 — UserPromptSubmit → working ────────────────────────────────
  {
    id: "agent.workingState",
    specId: "L1.AGENT.002",
    title: "UserPromptSubmit drives the Hub session idle→working",
    tags: ["agent", "hub"],
    isolation: "fresh",
    needs: [],
    rationale: `
WHAT: Injects ONE controlled \`claude {hook_event_name:"UserPromptSubmit"}\` frame to the
env's REAL reporter \`--serve\` socket (/tmp/fleet-reporter.sock) under a fresh session_id,
and asserts the env's Hub session row transitions to \`working\`. Resolves with a \`Stop\`
afterward so the session is left quiescent.

WHY THIS IS THE EXPECTED OUTCOME: the S15/S16 Claude adapter maps both UserPromptSubmit
and PreToolUse to State::Working with no urgency (confirmed in
crates/fleet-reporter/src/claude_infer.rs: enter_working on those hooks). A bare prompt
submission is the canonical "the agent started a turn" signal, so a single such frame must
surface the session as \`working\` once the UpsertRun reaches the Hub. We inject the exact
wire frame rather than running a real claude because the mapping is environment-independent
and the injected frame travels the identical socket → adapter → Hub → CLI path a real run
would (SPEC §"Determinism over realism").

WHY IT MATTERS: this pins the working edge at the socket→Hub→CLI boundary — the Rust unit
tests cover the pure machine, but only this exercises the live wire. A refactor that drops
the UserPromptSubmit→working mapping (or the UpsertRun forwarding) would silently hide
active agents from the rail; a future reader seeing this red should suspect the working
edge or the serve→Hub forwarding, not claude itself. No real claude → no auth gate; only
the Hub-availability gate applies (skips cleanly when the Hub is absent).`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;
      const startState = stateOf(gate.boot.line);

      const sid = `work-${env.id}`;
      sendFrame(env, {
        hook_event_name: "UserPromptSubmit",
        session_id: sid,
        cwd: "/home/coder/project",
      });

      const working = await pollHub(env.id, (_l, st) => st === "working", { ms: 20000, every: 750 });

      // Leave the session clean: a bare Stop settles the run back to idle.
      sendFrame(env, { hook_event_name: "Stop", session_id: sid, cwd: "/home/coder/project", stop_hook_active: false });

      return {
        pass: working.ok,
        detail: working.ok
          ? `Hub session "${env.id}" reached "working" after UserPromptSubmit (states: ${working.seen.join("->")})`
          : `"working" not observed within budget (states seen: ${JSON.stringify(working.seen)})`,
        evidence: { sessionTitle: env.id, startState, statesSeen: [...new Set(working.seen)], finalLine: working.line || gate.boot.line },
      };
    },
  },

  // ── L1.AGENT.003 — Stop (bare) → idle ────────────────────────────────────────
  {
    id: "agent.stopIdle",
    specId: "L1.AGENT.003",
    title: "A bare Stop settles the run to idle (turn finished)",
    tags: ["agent", "hub"],
    isolation: "fresh",
    needs: [],
    rationale: `
WHAT: Drives a session to \`working\` (injected UserPromptSubmit) then injects a BARE
\`claude {hook_event_name:"Stop", stop_hook_active:false}\` (no completion marker), and
asserts the Hub session transitions working→idle.

WHY THIS IS THE EXPECTED OUTCOME: \`Stop\` is THE completion signal; a bare Stop (no
\`reason:"completed"\`/\`subtype:"success"\`/\`task_complete\` marker, and not fired from
within a stop-hook continuation) means the turn ended and Claude is awaiting the next
prompt — conservatively \`idle\`, never \`done\` (D9). The adapter computes exactly this:
\`next = if turn_complete_done && !stop_hook_active { Done } else { Idle }\` (claude_infer.rs).
We first drive \`working\` so the working→idle EDGE is observed, not just a static idle.

WHY IT MATTERS: misclassifying Stop leaves runs stuck \`working\` forever (false "agent
busy") or over-claims \`done\`. This guards the idle-vs-continuation distinction at the
wire, the inverse of the working edge. Deterministic injection; no real claude → no auth
gate, only the Hub gate.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;

      const sid = `idle-${env.id}`;
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: sid, cwd: "/home/coder/project" });
      const working = await pollHub(env.id, (_l, st) => st === "working", { ms: 20000, every: 750 });

      sendFrame(env, { hook_event_name: "Stop", session_id: sid, cwd: "/home/coder/project", stop_hook_active: false });
      const idle = await pollHub(env.id, (_l, st) => st === "idle", { ms: 20000, every: 750 });

      const pass = working.ok && idle.ok;
      return {
        pass,
        detail: pass
          ? `Hub session "${env.id}": working→idle on a bare Stop`
          : `expected working→idle on Stop (working=${working.ok}, idle=${idle.ok}; states: ${JSON.stringify([...working.seen, ...idle.seen])})`,
        evidence: { sessionTitle: env.id, sawWorking: working.ok, sawIdle: idle.ok, statesSeen: [...new Set([...working.seen, ...idle.seen])], finalLine: idle.line || working.line },
      };
    },
  },

  // ── L1.AGENT.004 — Stop with completion marker → done ────────────────────────
  {
    id: "agent.stopDone",
    specId: "L1.AGENT.004",
    title: "A Stop carrying a completion marker settles to done (not idle)",
    tags: ["agent", "hub"],
    isolation: "fresh",
    needs: [],
    rationale: `
WHAT: Drives a session to \`working\` then injects a \`claude {hook_event_name:"Stop",
reason:"completed", stop_hook_active:false}\` and asserts the Hub session transitions
working→done (NOT idle).

WHY THIS IS THE EXPECTED OUTCOME: the adapter promotes a Stop to \`done\` exactly when a
task-completion marker is present and it is not a stop-hook continuation —
\`turn_complete_done && !stop_hook_active\` — where \`turn_complete_done\` is set by
\`reason:"completed"\`/\`reason:"done"\`/\`subtype:"success"|"completed"\`/\`task_complete:true\`
(claude.rs from_raw / claude_infer.rs apply). \`reason:"completed"\` is therefore expected to
yield \`done\`. A bare Stop (covered by agent.stopIdle / L1.AGENT.003) conservatively yields
\`idle\`; this is the distinguishing positive case.

WHY IT MATTERS: \`done\` (a finished, dismissible task) vs \`idle\` (turn paused awaiting a
prompt) are treated differently by the rollup/urgency UI (done ranks above idle; see
fleet-cli render). Collapsing the two would either over-claim completion or hide finished
work. Pins D9 at the socket boundary. Deterministic injection; Hub gate only.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;

      const sid = `done-${env.id}`;
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: sid, cwd: "/home/coder/project" });
      const working = await pollHub(env.id, (_l, st) => st === "working", { ms: 20000, every: 750 });

      sendFrame(env, { hook_event_name: "Stop", session_id: sid, cwd: "/home/coder/project", stop_hook_active: false, reason: "completed" });
      const done = await pollHub(env.id, (_l, st) => st === "done", { ms: 20000, every: 750 });

      const pass = working.ok && done.ok;
      return {
        pass,
        detail: pass
          ? `Hub session "${env.id}": working→done on a Stop with reason:"completed"`
          : `expected working→done on a completion Stop (working=${working.ok}, done=${done.ok}; states: ${JSON.stringify([...working.seen, ...done.seen])})`,
        evidence: { sessionTitle: env.id, sawWorking: working.ok, sawDone: done.ok, statesSeen: [...new Set([...working.seen, ...done.seen])], finalLine: done.line || working.line },
      };
    },
  },

  // ── L1.AGENT.006 — activity before the debounce cancels the pending waiting ──
  {
    id: "agent.waitingCancelled",
    specId: "L1.AGENT.006",
    title: "A Stop before the debounce cancels the pending inferred waiting",
    tags: ["agent", "hub"],
    isolation: "fresh",
    needs: [],
    rationale: `
WHAT: Injects a \`PreToolUse\` (arming the S16 inference) then, within the debounce window
(< DEFAULT_DEBOUNCE_MS = 1.5s), injects a \`Stop\` for the SAME session BEFORE the serve
tick can fire. Asserts the Hub session NEVER shows \`waiting\` (a short poll returns
not-ok) and instead settles to \`idle\`.

WHY THIS IS THE EXPECTED OUTCOME: the inference is a DEBOUNCE — a PreToolUse arms it, and
ANY follow-up activity before the window elapses cancels the arm (claude_infer.rs:
\`clear_inference()\` on Stop/UserPromptSubmit/PreToolUse; only \`tick()\` past the window
calls \`fire_inference\`). A tool that was approved/ran quickly therefore must NOT raise a
false \`waiting\`. Sending the Stop promptly (no inter-frame sleep) keeps us inside the
window; we then assert \`waiting\` is absent over a short budget and that the session is
\`idle\`. This is the exact inverse of agent.waitingState (L1.AGENT.005).

WHY IT MATTERS: a false \`waiting\` would ping the user for nothing — eroding trust in
Fleet's core approval signal. This guards "any later activity cancels the arm" at the live
wire (the negative half of the inference contract). Note the assertion is conservative: it
proves \`waiting\` did not appear AND idle did, so a flaky tick that fired anyway would be
caught. Hub gate only; no claude/auth.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;

      const sid = `cancel-${env.id}`;
      // Arm then immediately cancel — no sleep between, to stay inside the <1.5s window.
      sendFrame(env, { hook_event_name: "PreToolUse", session_id: sid, tool_name: "Bash", tool_use_id: "toolu_cancel", cwd: "/home/coder/project" });
      sendFrame(env, { hook_event_name: "Stop", session_id: sid, cwd: "/home/coder/project", stop_hook_active: false });

      // `waiting` must NOT appear (the arm was cancelled before the tick).
      const waited = await pollHub(env.id, (_l, st) => st === "waiting", { ms: 5000, every: 500 });
      // The session must settle to idle (the cancelling Stop).
      const idle = await pollHub(env.id, (_l, st) => st === "idle", { ms: 10000, every: 750 });

      const pass = !waited.ok && idle.ok;
      return {
        pass,
        detail: pass
          ? `Hub session "${env.id}" never showed "waiting"; settled idle (the pre-debounce Stop cancelled the arm)`
          : `expected NO waiting + idle (sawWaiting=${waited.ok}, sawIdle=${idle.ok}; states: ${JSON.stringify([...waited.seen, ...idle.seen])})`,
        evidence: { sessionTitle: env.id, sawWaiting: waited.ok, sawIdle: idle.ok, statesSeen: [...new Set([...waited.seen, ...idle.seen])], finalLine: idle.line || waited.line },
      };
    },
  },

  // ── L1.AGENT.007 — repeat PreToolUse re-arms; a single waiting raised ────────
  {
    id: "agent.waitingCoalesced",
    specId: "L1.AGENT.007",
    title: "Repeat PreToolUse re-arms the debounce; exactly one waiting on the session",
    tags: ["agent", "hub"],
    isolation: "fresh",
    needs: [],
    rationale: `
WHAT: Injects a \`PreToolUse\` (session \`rearm-<env.id>\`), lets it reach \`waiting\`, then
injects a SECOND \`PreToolUse\` for the SAME session (a new tool dispatch while still
blocked) and re-checks. Asserts the session is still \`waiting\` and the run count under the
env's row did NOT increase beyond what the first frame minted (it coalesces to a single
run). Resolves with a Stop.

WHY THIS IS THE EXPECTED OUTCOME: a real agent fires multiple PreToolUse on one blocked
turn. The adapter mints exactly ONE run per session_id (run_counter increments only on the
first sighting of a session_id — claude.rs/claude_infer.rs ingest), and a second PreToolUse
on that same session re-arms the debounce rather than stacking a second waiting run
(\`clear_inference\` then \`enter_working\` then re-arm). So the count stays at one run for
this session and the rollup stays \`waiting\`. We measure the env-row run count BEFORE
injecting (baseline) and assert the post-second-frame count did not exceed baseline+1.

WHY IT MATTERS: without coalescing, each tool dispatch on a blocked turn would multiply the
approval ping — spamming the user. Guards against waiting-run duplication under repeated
tool dispatch at the wire. Hub gate only; deterministic injection.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;
      const baseRuns = runCountOf(gate.boot.line) ?? 0;

      const sid = `rearm-${env.id}`;
      // First PreToolUse — let the debounce fire → waiting.
      sendFrame(env, { hook_event_name: "PreToolUse", session_id: sid, tool_name: "Bash", tool_use_id: "toolu_rearm1", cwd: "/home/coder/project" });
      const firstWait = await pollHub(env.id, (_l, st) => st === "waiting", { ms: 30000, every: 1000 });

      // Second PreToolUse on the SAME session — re-arms, must not stack a second run.
      sendFrame(env, { hook_event_name: "PreToolUse", session_id: sid, tool_name: "Bash", tool_use_id: "toolu_rearm2", cwd: "/home/coder/project" });
      const stillWait = await pollHub(env.id, (_l, st) => st === "waiting", { ms: 30000, every: 1000 });

      const endRuns = runCountOf(stillWait.line || firstWait.line) ?? baseRuns;

      // Resolve cleanly.
      sendFrame(env, { hook_event_name: "Stop", session_id: sid, cwd: "/home/coder/project", stop_hook_active: false });

      // Coalesced == the second PreToolUse added no extra run (delta ≤ 1 over baseline).
      const coalesced = endRuns <= baseRuns + 1;
      const pass = firstWait.ok && stillWait.ok && coalesced;
      return {
        pass,
        detail: pass
          ? `Hub session "${env.id}": one waiting coalesced across two PreToolUse (runs ${baseRuns}→${endRuns}, ≤ +1)`
          : `coalescing failed (firstWait=${firstWait.ok}, stillWait=${stillWait.ok}, runs ${baseRuns}→${endRuns})`,
        evidence: { sessionTitle: env.id, baseRuns, endRuns, sawWaiting: firstWait.ok, stillWaiting: stillWait.ok, finalLine: stillWait.line || firstWait.line },
      };
    },
  },

  // ── L1.AGENT.010 — SessionEnd → dead ─────────────────────────────────────────
  {
    id: "agent.sessionEndDead",
    specId: "L1.AGENT.010",
    title: "SessionEnd marks the run dead (one-shot exit)",
    tags: ["agent", "hub"],
    isolation: "fresh",
    needs: [],
    rationale: `
WHAT: Drives a session to \`working\` (injected UserPromptSubmit) then injects a
\`claude {hook_event_name:"SessionEnd"}\` for that session and asserts the Hub session
transitions to \`dead\`.

WHY THIS IS THE EXPECTED OUTCOME: \`SessionEnd\` is the authoritative session-closed signal
— the adapter maps it to State::Dead with Confidence::High (the only High in this adapter;
claude_infer.rs SessionEnd arm). \`claude -p\` is one-shot: it fires SessionEnd on exit, which
is precisely why \`agent.claudeRuns\` (L1.AGENT.001) accepts \`dead\` as a legitimate terminal
state. We drive \`working\` first so the working→dead edge is observed.

WHY IT MATTERS: without the SessionEnd→dead mapping, a finished one-shot run would look
stuck (\`working\`) forever. This pins the terminal-exit edge at the wire and underpins
agent.claudeRuns' acceptance of \`dead\` as terminal. Deterministic injection; Hub gate only.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;

      const sid = `end-${env.id}`;
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: sid, cwd: "/home/coder/project" });
      const working = await pollHub(env.id, (_l, st) => st === "working", { ms: 20000, every: 750 });

      sendFrame(env, { hook_event_name: "SessionEnd", session_id: sid, cwd: "/home/coder/project" });
      const dead = await pollHub(env.id, (_l, st) => st === "dead", { ms: 20000, every: 750 });

      const pass = working.ok && dead.ok;
      return {
        pass,
        detail: pass
          ? `Hub session "${env.id}": working→dead on SessionEnd`
          : `expected working→dead on SessionEnd (working=${working.ok}, dead=${dead.ok}; states: ${JSON.stringify([...working.seen, ...dead.seen])})`,
        evidence: { sessionTitle: env.id, sawWorking: working.ok, sawDead: dead.ok, statesSeen: [...new Set([...working.seen, ...dead.seen])], finalLine: dead.line || working.line },
      };
    },
  },

  // ── L1.AGENT.011 — multi-turn: one run, cycles working↔idle ──────────────────
  {
    id: "agent.multiTurnOneRun",
    specId: "L1.AGENT.011",
    title: "Multi-turn on one session_id keeps ONE run cycling working↔idle",
    tags: ["agent", "hub"],
    isolation: "fresh",
    needs: [],
    rationale: `
WHAT: For ONE session_id (\`mt-<env.id>\`), injects the sequence UserPromptSubmit → Stop →
UserPromptSubmit → Stop (two turns). Asserts the Hub \`seen\` array shows the working↔idle
CYCLE (working, then idle, then working again, then idle), ends \`idle\`, and the run count
under the env's row did not grow by more than one across the whole sequence (the two turns
share a SINGLE run).

WHY THIS IS THE EXPECTED OUTCOME: the Claude \`session_id\` is the run's durable identity
(native_id, D4); a run is minted once per NEW session_id (run_counter increments only on
first sighting — claude_infer.rs ingest). Multiple turns on the same session_id therefore
must NOT spawn phantom runs — they re-use the one run, which cycles
idle→working→idle→working→idle as each prompt starts and each Stop ends a turn. We assert
both the state cycle (via pollHub's recorded \`seen\`) and the run-count stability (baseline
measured before the sequence; delta ≤ 1).

WHY IT MATTERS: durable identity is what lets Fleet track a real multi-turn conversation as
ONE session rather than a swarm of phantom runs. A regression that re-derived identity per
turn would split one conversation into many. Guards run-count stability across an
interactive multi-turn at the wire. Hub gate only; deterministic injection.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;
      const baseRuns = runCountOf(gate.boot.line) ?? 0;

      const sid = `mt-${env.id}`;
      // Turn 1.
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: sid, cwd: "/home/coder/project" });
      const w1 = await pollHub(env.id, (_l, st) => st === "working", { ms: 20000, every: 600 });
      sendFrame(env, { hook_event_name: "Stop", session_id: sid, cwd: "/home/coder/project", stop_hook_active: false });
      const i1 = await pollHub(env.id, (_l, st) => st === "idle", { ms: 20000, every: 600 });
      // Turn 2 (same session_id).
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: sid, cwd: "/home/coder/project" });
      const w2 = await pollHub(env.id, (_l, st) => st === "working", { ms: 20000, every: 600 });
      sendFrame(env, { hook_event_name: "Stop", session_id: sid, cwd: "/home/coder/project", stop_hook_active: false });
      const i2 = await pollHub(env.id, (_l, st) => st === "idle", { ms: 20000, every: 600 });

      const endRuns = runCountOf(i2.line || w2.line) ?? baseRuns;
      const cycled = w1.ok && i1.ok && w2.ok && i2.ok;
      const oneRun = endRuns <= baseRuns + 1;
      const pass = cycled && oneRun;
      return {
        pass,
        detail: pass
          ? `Hub session "${env.id}": two turns cycled working↔idle on ONE run (runs ${baseRuns}→${endRuns}, ≤ +1)`
          : `multi-turn failed (cycle w1=${w1.ok},i1=${i1.ok},w2=${w2.ok},i2=${i2.ok}; runs ${baseRuns}→${endRuns})`,
        evidence: { sessionTitle: env.id, baseRuns, endRuns, cycle: { w1: w1.ok, i1: i1.ok, w2: w2.ok, i2: i2.ok }, finalLine: i2.line },
      };
    },
  },

  // ── L1.AGENT.014 — SubagentStop is liveness-only, never ends the main run ────
  {
    id: "agent.subagentStopLiveness",
    specId: "L1.AGENT.014",
    title: "SubagentStop is liveness only; the main run stays working",
    tags: ["agent", "hub"],
    isolation: "fresh",
    needs: [],
    rationale: `
WHAT: Drives a session to \`working\` (injected UserPromptSubmit), injects a
\`claude {hook_event_name:"SubagentStop"}\` for that session, and asserts the session STAYS
\`working\` (a short poll for \`idle\` returns not-ok). Then injects a real \`Stop\` and asserts
it settles \`idle\` — proving only a real Stop ends the run.

WHY THIS IS THE EXPECTED OUTCOME: a SubagentStop ends a SUBAGENT's turn, not the parent
turn. The adapter treats it as pure liveness — \`SubagentStop | PreCompact => no_op(true)\`
(claude_infer.rs) — so it refreshes the liveness window but never flips state. The main run
must therefore remain \`working\` until a genuine \`Stop\`, which we then send to confirm the
real completion edge still works.

WHY IT MATTERS: if SubagentStop were mistaken for the main turn's Stop, every Task/subagent
call would falsely show the parent as finished — agents would appear done mid-work. Guards
the SubagentStop-is-not-completion rule at the wire. Deterministic injection; Hub gate only.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;

      const sid = `sub-${env.id}`;
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: sid, cwd: "/home/coder/project" });
      const working = await pollHub(env.id, (_l, st) => st === "working", { ms: 20000, every: 750 });

      // SubagentStop must NOT settle the main run — it should stay working.
      sendFrame(env, { hook_event_name: "SubagentStop", session_id: sid, cwd: "/home/coder/project" });
      const wrongIdle = await pollHub(env.id, (_l, st) => st === "idle", { ms: 5000, every: 500 });
      const stillWorking = await pollHub(env.id, (_l, st) => st === "working", { ms: 5000, every: 500 });

      // A real Stop then settles it idle (the genuine completion edge).
      sendFrame(env, { hook_event_name: "Stop", session_id: sid, cwd: "/home/coder/project", stop_hook_active: false });
      const idle = await pollHub(env.id, (_l, st) => st === "idle", { ms: 15000, every: 750 });

      const pass = working.ok && !wrongIdle.ok && stillWorking.ok && idle.ok;
      return {
        pass,
        detail: pass
          ? `Hub session "${env.id}": SubagentStop kept the run working; a real Stop then settled idle`
          : `SubagentStop liveness rule violated (working=${working.ok}, idleAfterSubStop=${wrongIdle.ok} [want false], stillWorking=${stillWorking.ok}, idleAfterStop=${idle.ok})`,
        evidence: { sessionTitle: env.id, sawWorking: working.ok, idleAfterSubagentStop: wrongIdle.ok, stillWorking: stillWorking.ok, idleAfterStop: idle.ok, finalLine: idle.line },
      };
    },
  },

  // ── L1.AGENT.018 — waiting carries the [approval] urgency label ──────────────
  {
    id: "agent.waitingApprovalUrgency",
    specId: "L1.AGENT.018",
    title: "An inferred waiting carries the [approval] urgency in the rollup",
    tags: ["agent", "hub"],
    isolation: "fresh",
    needs: [],
    rationale: `
WHAT: Injects a single \`PreToolUse\`-without-\`Stop\` (the S16 inference trigger) and waits
for the env's Hub row to reach \`waiting\`; then asserts the SAME row carries BOTH the
\`[waiting]\` state AND the \`  [approval]\` urgency label. Resolves with a Stop.

WHY THIS IS THE EXPECTED OUTCOME: when the debounce fires, the infer machine sets
\`state = Waiting\` AND \`urgency = Some(Urgency::Approval)\` together (claude_infer.rs
fire_inference). The Hub's rollup carries that urgency, and \`fleet ls\` renders it as the
\`  [approval]\` label after the state (fleet-cli render.rs urgency_label). So a row that is
\`[waiting]\` from this path must also show \`[approval]\` — the two are emitted as one unit.
This extends agent.waitingState (L1.AGENT.005, which only asserts the state) to the urgency
stamping.

WHY IT MATTERS: the urgency label is what drives the rail badge/ping priority
(approval > question > none). A waiting with no/wrong urgency would mis-prioritise the
user's attention — a high-stakes approval could rank below a low-priority question. Guards
the urgency stamping all the way through rollup → CLI render. Hub gate only; deterministic.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;

      const sid = `appr-${env.id}`;
      sendFrame(env, { hook_event_name: "PreToolUse", session_id: sid, tool_name: "Bash", tool_use_id: "toolu_appr", cwd: "/home/coder/project" });

      // Wait for waiting AND the approval label on the SAME row.
      const waited = await pollHub(
        env.id,
        (line, st) => st === "waiting" && line.includes("[approval]"),
        { ms: 45000, every: 1000 },
      );

      // Resolve cleanly.
      sendFrame(env, { hook_event_name: "Stop", session_id: sid, cwd: "/home/coder/project", stop_hook_active: false });

      return {
        pass: waited.ok,
        detail: waited.ok
          ? `Hub session "${env.id}" row shows [waiting] AND [approval] (${JSON.stringify((waited.line || "").trim())})`
          : `waiting+[approval] not observed within budget (states seen: ${JSON.stringify(waited.seen)}; last line: ${JSON.stringify((waited.line || "").trim())})`,
        evidence: { sessionTitle: env.id, statesSeen: [...new Set(waited.seen)], finalLine: waited.line || gate.boot.line },
      };
    },
  },

  // ── L1.AGENT.020 — concurrent agents in one reporter: per-session multiplexing ─
  {
    id: "agent.concurrentSessions",
    specId: "L1.AGENT.020",
    title: "One reporter multiplexes two sessions; both runs appear, independent",
    tags: ["agent", "hub"],
    isolation: "fresh",
    needs: [],
    rationale: `
WHAT: Injects interleaved frames for TWO distinct session_ids through the env's ONE
reporter socket — UserPromptSubmit(a), UserPromptSubmit(b), Stop(a), Stop(b) — and asserts
the env's Hub row ends up tracking BOTH runs (run count grew by ≥ 2 over the pre-injection
baseline) and settles \`idle\` (both turns ended). It also injects a fourth frame with NO
session_id and asserts that does NOT mint a phantom run.

WHY THIS IS THE EXPECTED OUTCOME: one reporter shell can host several \`claude\` invocations;
the adapter owns one state machine per session_id and mints one run per DISTINCT session_id
(claude_infer.rs ingest, keyed on session_id). Two distinct session_ids therefore produce
two independent runs under the env's session row, each cycling working→idle on its own
frames with no cross-bleed. A frame with no session_id is a MissingSessionId parse error and
is dropped (identity honesty: no durable anchor → no run), so it must not increase the
count. We measure the baseline run count first and assert the delta is ≥ 2 from the two
valid sessions and unchanged by the anchorless frame.

WHY IT MATTERS: multi-session correctness is load-bearing — state leaking between concurrent
agents, or a phantom run from an anchorless frame, would corrupt the rail. Guards per-session
multiplexing + identity honesty at the socket. Hub gate only; deterministic injection.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;
      const baseRuns = runCountOf(gate.boot.line) ?? 0;

      const a = `a-${env.id}`;
      const b = `b-${env.id}`;
      // Interleaved start of two independent sessions.
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: a, cwd: "/home/coder/project" });
      sendFrame(env, { hook_event_name: "UserPromptSubmit", session_id: b, cwd: "/home/coder/project" });
      // Both should be registered as runs under the env's row.
      const grew = await pollHub(env.id, (line) => (runCountOf(line) ?? 0) >= baseRuns + 2, { ms: 30000, every: 1000 });

      // An anchorless frame must NOT mint a run.
      sendFrame(env, { hook_event_name: "UserPromptSubmit", cwd: "/home/coder/project" });

      // End both turns.
      sendFrame(env, { hook_event_name: "Stop", session_id: a, cwd: "/home/coder/project", stop_hook_active: false });
      sendFrame(env, { hook_event_name: "Stop", session_id: b, cwd: "/home/coder/project", stop_hook_active: false });
      const idle = await pollHub(env.id, (_l, st) => st === "idle", { ms: 20000, every: 1000 });

      const finalRuns = runCountOf(idle.line || grew.line) ?? baseRuns;
      // The anchorless frame must not have pushed the count beyond the two valid sessions.
      const noPhantom = finalRuns <= baseRuns + 2;
      const pass = grew.ok && idle.ok && noPhantom;
      return {
        pass,
        detail: pass
          ? `Hub session "${env.id}": two concurrent sessions → +2 runs (${baseRuns}→${finalRuns}); anchorless frame minted no phantom; settled idle`
          : `multiplexing failed (grew=${grew.ok}, idle=${idle.ok}, runs ${baseRuns}→${finalRuns}, noPhantom=${noPhantom})`,
        evidence: { sessionTitle: env.id, baseRuns, finalRuns, sawTwoRuns: grew.ok, sawIdle: idle.ok, finalLine: idle.line || grew.line },
      };
    },
  },

  // ── L1.AGENT.023 — empty state: no agent run leaves the session quiescent ────
  {
    id: "agent.emptyQuiescent",
    specId: "L1.AGENT.023",
    title: "With no agent ever invoked the session sits quiescent at idle",
    tags: ["agent", "hub"],
    isolation: "fresh", // a clean container guarantees the no-run baseline
    needs: [],
    rationale: `
WHAT: On a fresh env where NO agent frame is ever injected, reads the env's Hub row and
asserts it is registered (the reporter phoned home at boot) but quiescent: state \`idle\`,
no active run, and a short poll for \`working\`/\`waiting\` returns NOT ok (no spurious
activity fabricated on boot).

WHY THIS IS THE EXPECTED OUTCOME: the reporter registers the env's session at boot before
any agent runs, in \`idle\` (Session::new starts idle; the machines start idle —
claude.rs/claude_infer.rs ClaudeStateMachine::new → State::Idle). With nothing driving a
turn, no UpsertRun(working) is ever forwarded, so the row must stay \`[idle]\`. We assert the
quiescent baseline directly: state == idle AND no working/waiting appears over a short
budget. \`fresh\` isolation is load-bearing — a clean container guarantees no prior behaviour
left a run on this env's session.

WHY IT MATTERS: the empty/idle baseline is the reference every active-state assertion is
measured against. A reporter that fabricated activity on boot would false-ping the user and
invalidate every "reached working/waiting" test in this area. Guards the quiescent baseline.
Hub gate only; no frames injected, so nothing to clean up.`,
    async run(env) {
      const gate = await hubGate(env);
      if (gate.skip) return gate.skip;
      const startState = stateOf(gate.boot.line);

      // No frames injected. The row must be idle and must NOT drift to working/waiting.
      const drift = await pollHub(env.id, (_l, st) => st === "working" || st === "waiting", { ms: 4000, every: 500 });
      // Re-read to confirm the final state is idle.
      const idleNow = await pollHub(env.id, (_l, st) => st === "idle", { ms: 4000, every: 500 });

      const pass = !drift.ok && idleNow.ok;
      return {
        pass,
        detail: pass
          ? `Hub session "${env.id}" is quiescent [idle] with no spurious working/waiting`
          : `quiescent baseline violated (startState=${startState}, sawActivity=${drift.ok}, idleNow=${idleNow.ok}; states: ${JSON.stringify([...drift.seen, ...idleNow.seen])})`,
        evidence: { sessionTitle: env.id, startState, sawActivity: drift.ok, idleNow: idleNow.ok, finalLine: idleNow.line || gate.boot.line },
      };
    },
  },
];
