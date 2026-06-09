// L2 — multiplexer (23) · agent-state-flow (24) · deploy-spawn (25).
//
// The overwhelming majority of these three spec areas exercise the HOST-side Tauri
// app (`fleet-host`: the rail/editor webviews, the bridge registry, the native menu)
// and the host process SUPERVISOR (`spawn::ServerSupervisor`). The container eval
// harness only drives in-container bridges over :51778 + the in-container reporter
// socket + the host-side `fleet ls --once` Hub query — it does NOT boot `fleet-host`,
// so every MUX.* and SPAWN.* entry, and every FLOW.* entry that asserts the rail
// `RenderedInbox`/badge face, is left TODO (needs a host-harness lane). See the spec
// files' headers for that rationale.
//
// What IS testable here, deterministically, are the agent-state-flow entries that
// reduce to the env's REAL `fleet-reporter --serve` socket → Hub → `fleet ls --once`
// path — the exact pipeline already proven by `agent.claudeRuns` (FLOW.001) and
// `agent.waitingState` (FLOW.002). This file adds the two siblings of those that the
// same primitives reach:
//
//   flow.cancelBeforeDebounce (L2.FLOW.003) — inject a PreToolUse then a Stop for the
//     same session BEFORE the S16 debounce window elapses, and assert the env's Hub
//     session NEVER renders `waiting` (the inverse of agent.waitingState: the cancel
//     path must suppress the false ping). Fully deterministic, no claude/auth needed —
//     it drives the SAME socket → infer-tick → Hub → CLI path with a controlled frame
//     pair, so it skips ONLY when the Hub/CLI is unreachable.
//
//   flow.secondRunMergesSession (L2.FLOW.012) — run `claude -p` TWICE in one env and
//     assert the env's single Hub session's run count rises (1→2) without spawning a
//     second row. Needs a real authenticated claude + a reachable Hub, so it carries
//     the SAME clean-SKIP gates as agent.claudeRuns (unauth / no-Hub ⇒ skip, never
//     fail).
//
// Both reuse the Hub-query helpers from agentInput.mjs (the live Hub face). We import
// them rather than replicate so the wire-format assumptions stay in one place.

import { execSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ─── Host-side Hub query (mirrors agentInput.mjs; the Hub lives on the host) ────
// The reporter inside each env phones home to ws://HOST:51777 and registers ONE
// session titled by FLEET_SESSION_TITLE == FLEET_SERVER_ID == env.id. Every run the
// reporter emits (whatever the injected frame's internal session_id) folds into that
// one Hub session, so we always match the env.id row. `fleet ls --once` renders one
// line per session: "[<state>]<unread> <title>  (<n> run[s])[ <urgency>]".

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

// Parse the "(N run[s])" count off a rendered session line; 0 when absent.
function runCountOf(line) {
  if (!line) return 0;
  const m = line.match(/\((\d+) runs?\)/);
  return m ? Number(m[1]) : 0;
}

// Poll the Hub until `match(line, state)` is true for this session, or timeout.
// Records every distinct state seen so evidence can show the transition path.
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

// Watch the Hub for `windowMs`, asserting the session NEVER reaches `forbidden`.
// Returns { clean:true } if it stayed clear, else { clean:false, line } at the hit.
async function watchHubNever(sessionTitle, forbidden, { windowMs, every = 500 } = {}) {
  const t0 = Date.now();
  const seen = [];
  let lastLine = null;
  while (Date.now() - t0 < windowMs) {
    const line = sessionLineFor(hubSnapshot(), sessionTitle);
    if (line) {
      lastLine = line;
      const st = stateOf(line);
      if (st && seen[seen.length - 1] !== st) seen.push(st);
      if (st === forbidden) return { clean: false, seen, line };
    }
    await sleep(every);
  }
  return { clean: true, seen, line: lastLine };
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ── L2.FLOW.003 — activity before the debounce cancels the waiting inference ──
  {
    id: "flow.cancelBeforeDebounce",
    specId: "L2.FLOW.003",
    title: "A Stop before the debounce cancels the pending waiting inference",
    tags: ["agent", "hub", "flow"],
    isolation: "fresh",
    needs: [],
    rationale: `
WHAT: Verifies the CANCEL half of the S16 inferred-waiting machine end-to-end through
the env's REAL \`fleet-reporter --serve\` socket. We inject a single controlled
\`PreToolUse\` frame (which ARMS the infer adapter) and then, well WITHIN the debounce
window (DEFAULT_DEBOUNCE_MS = 1.5s), inject a \`Stop\` frame for the SAME session. We
then watch the env's Hub session (titled by FLEET_SESSION_TITLE == env.id, read via
\`fleet ls --once\`) for several debounce windows and assert it NEVER renders
\`waiting\` — the arm was cancelled before its tick could fire.

WHY THIS IS THE EXPECTED OUTCOME: Claude exposes no authoritative "approval needed"
hook, so the reporter INFERS waiting (S16): a \`PreToolUse\` not followed by any
activity for the debounce window ⇒ \`waiting\`; ANY later frame for that session (here
a \`Stop\`) cancels the pending arm before the INFER_TICK_INTERVAL (250ms) advances the
clock past the debounce. So a PreToolUse promptly followed by a Stop must settle at
\`idle\` and is expected to NEVER surface as \`waiting\`. This is the exact inverse of
\`agent.waitingState\` (L2.FLOW.002), which sends a PreToolUse with NO follow-up and DOES
reach waiting. We inject the controlled frame pair rather than driving a real claude
into (and out of) an approval block because the detection pipeline we own is
deterministic, whereas a real claude block is environment-flaky. The frames travel the
SAME socket → S15+S16 adapters → Hub → \`fleet ls\` path a real run would, so this is a
true end-to-end test of the cancel path, not a unit test.

WHY IT MATTERS: This is the ONLY end-to-end guard that the infer adapter's arm-then-
cancel suppresses a FALSE ping when activity resumes before the human was actually
blocked. If a refactor breaks the cancel (e.g. a follow-up frame stops cancelling the
pending arm, or the debounce tick fires regardless), Fleet would cry wolf — pinging the
user "agent is waiting" when it had already moved on — eroding trust in the one signal
that justifies the rail badge. The Rust unit
\`activity_before_debounce_cancels_the_inference\` exercises the infer machine in
isolation; this is the socket→Hub→CLI proof that the cancel actually reaches the face.
A future reader seeing this FAIL (a spurious \`waiting\` appeared) should suspect the
infer adapter's cancel wiring or the serve tick, not claude. It SKIPS cleanly only when
the Hub/CLI is unreachable — that is environmental, not a regression — never hard-fails
for a missing Hub.`,
    async run(env) {
      const sessionTitle = env.id;

      // Hub gate — need the fleet CLI + a registered session. (The reporter --serve
      // socket lives in the env regardless; this behaviour does NOT run claude.)
      const cli = fleetCli();
      if (!cli) {
        return {
          pass: false,
          skipped: "Hub `fleet` CLI not found (target/debug/fleet) — start the Hub (see test.sh)",
          detail: "skipped: no fleet CLI to query the Hub",
        };
      }
      const boot = await pollHub(sessionTitle, () => true, { ms: 15000, every: 1000 });
      if (!boot.ok) {
        return {
          pass: false,
          skipped: `Hub session "${sessionTitle}" not found (Hub down or reporter not phoned home)`,
          detail: "skipped: env's session is not registered on the Hub",
          evidence: { sessionTitle, cli },
        };
      }
      const startState = stateOf(boot.line);

      // Send a CONTROLLED PreToolUse-then-Stop pair to the env's REAL reporter --serve
      // socket, with the Stop arriving well within the 1.5s debounce so it CANCELS the
      // pending waiting arm before the infer tick can fire. Same socket/path as a real
      // claude run, but deterministic.
      const send = (obj) =>
        env.exec(`printf 'claude %s\n' '${JSON.stringify(obj)}' | timeout 2 nc -N -U /tmp/fleet-reporter.sock 2>/dev/null || true`);
      const sid = `cancel-${env.id}`;
      send({ hook_event_name: "PreToolUse", session_id: sid, tool_name: "Bash", tool_use_id: "toolu_fleetcancel", cwd: "/home/coder/project" });
      // Cancel well inside DEFAULT_DEBOUNCE_MS (1.5s): a Stop ~400ms later.
      await sleep(400);
      send({ hook_event_name: "Stop", session_id: sid, cwd: "/home/coder/project", stop_hook_active: false });

      // Watch for ~6s (4× the debounce) — if the cancel works, `waiting` never appears.
      const watch = await watchHubNever(sessionTitle, "waiting", { windowMs: 6000, every: 500 });

      return {
        pass: watch.clean,
        detail: watch.clean
          ? `Hub session "${sessionTitle}" never reached "waiting" after a PreToolUse cancelled by a prompt Stop (states: ${[...watch.seen].join("->") || startState})`
          : `Hub session "${sessionTitle}" wrongly reached "waiting" despite a cancelling Stop (false ping; line: ${JSON.stringify(watch.line)})`,
        evidence: { sessionTitle, startState, statesSeen: [...new Set(watch.seen)], finalLine: watch.line || boot.line },
      };
    },
  },

  // ── L2.FLOW.012 — a second run merges into the same session (run_count +1) ─────
  {
    id: "flow.secondRunMergesSession",
    specId: "L2.FLOW.012",
    title: "A second claude -p run appears as run_count +1, not a new session row",
    tags: ["agent", "hub", "flow"],
    isolation: "fresh",
    needs: ["termSend"],
    rationale: `
WHAT: Runs \`claude -p\` TWICE in one env (two one-shot turns in the same workspace),
then asserts the env's SINGLE Hub session — titled by FLEET_SESSION_TITLE == env.id,
read via \`fleet ls --once\` — accumulates both runs under ONE row: its rendered
"(N run[s])" count rises (from ≤1 before the second run to ≥2 after), and the env
still has exactly ONE session row (no ghost second row).

WHY THIS IS THE EXPECTED OUTCOME: every \`claude\` invocation in this env reports through
the SAME \`fleet-reporter --serve\` process, which phones home as ONE Hub session keyed
by the env id. \`-p\` is one-shot (does a turn, fires SessionEnd, exits), so each run is a
distinct run record, but the Hub MERGES runs into the existing session by its durable id
rather than spawning a fresh session per invocation. Thus the correct outcome is one row
whose run count grows, not two rows — repeated agent runs in one workspace must
accumulate under one tab, not litter the rail. We drive real \`claude -p\` (not injected
frames) because the merge-by-durable-id reclaim is precisely the run-record bookkeeping a
synthetic single-frame inject would bypass; the run count is the observable that proves
two real runs folded into one session.

WHY IT MATTERS: This guards the Hub session-merge / reclaim-no-ghost path end-to-end —
the property that makes the rail usable when a user runs claude repeatedly in a
workspace. If a refactor regresses the reclaim (each run minting a new session, or the
count not incrementing), the rail would either spawn duplicate rows for one workspace or
under-count its activity. \`agent.claudeRuns\` (L2.FLOW.001) proves ONE run reaches the
Hub and reads "(N runs)"; this is the distinct second-run-merges assertion that one
proves does not cover. The two runtime gates are load-bearing and must NOT become hard
failures: an unauthenticated container claude and an absent/unreachable Hub both SKIP
cleanly (environmental, not regressions). A future reader seeing this FAIL (the row
count did not advance, or a second row appeared) knows the session-merge wiring itself
regressed, not claude.`,
    async run(env) {
      const sessionTitle = env.id;

      // Auth gate — same as agent.claudeRuns: the harness authenticates the
      // container's claude in reset(); if that didn't land, SKIP cleanly.
      if (!env.claudeAuthed) {
        return {
          pass: false,
          skipped: "container claude not authenticated — set ANTHROPIC_API_KEY, or ensure the 'Claude Code-credentials' Keychain item is accessible (FLEET_CLAUDE_OAUTH=0 to disable injection)",
          detail: "skipped: no claude auth available to the container",
        };
      }

      // Hub gate — need the fleet CLI + a registered session.
      const cli = fleetCli();
      if (!cli) {
        return {
          pass: false,
          skipped: "Hub `fleet` CLI not found (target/debug/fleet) — start the Hub (see test.sh)",
          detail: "skipped: no fleet CLI to query the Hub",
        };
      }
      const boot = await pollHub(sessionTitle, () => true, { ms: 15000, every: 1000 });
      if (!boot.ok) {
        return {
          pass: false,
          skipped: `Hub session "${sessionTitle}" not found (Hub down or reporter not phoned home)`,
          detail: "skipped: env's session is not registered on the Hub",
          evidence: { sessionTitle, cli },
        };
      }

      // Run #1 — drive the agent and wait for it to settle (idle/done/dead).
      await env.request({ type: "termSend", text: 'claude -p "say hi"\n' });
      const settled1 = await pollHub(
        sessionTitle,
        (_l, st) => st === "idle" || st === "done" || st === "dead",
        { ms: 90000, every: 1500 },
      );
      const afterFirst = sessionLineFor(hubSnapshot(), sessionTitle) || settled1.line || boot.line;
      const countAfterFirst = runCountOf(afterFirst);

      // If the first run never completed (e.g. claude could not start a turn at all),
      // SKIP rather than hard-fail the merge assertion on an environment problem.
      if (!settled1.ok && countAfterFirst === 0) {
        return {
          pass: false,
          skipped: "first claude -p run never settled / recorded — cannot test second-run merge (claude could not complete a turn)",
          detail: "skipped: no first run to merge a second into",
          evidence: { sessionTitle, statesSeen: settled1.seen, finalLine: afterFirst },
        };
      }

      // Run #2 — same env, same workspace, same reporter session.
      await env.request({ type: "termSend", text: 'claude -p "again"\n' });
      const settled2 = await pollHub(
        sessionTitle,
        (_l, st) => st === "idle" || st === "done" || st === "dead",
        { ms: 90000, every: 1500 },
      );

      // After the second run, poll for the run count to advance to ≥2 (the count can
      // lag the state settle by a beat). The env's session row must still be unique.
      let afterSecond = sessionLineFor(hubSnapshot(), sessionTitle) || settled2.line || afterFirst;
      let countAfterSecond = runCountOf(afterSecond);
      for (let i = 0; i < 10 && countAfterSecond < 2; i++) {
        await sleep(1500);
        const line = sessionLineFor(hubSnapshot(), sessionTitle);
        if (line) { afterSecond = line; countAfterSecond = runCountOf(line); }
      }

      // Exactly one row titled by this env id (no ghost second session).
      const snap = hubSnapshot() || "";
      const rowsForEnv = snap
        .split("\n")
        .map((l) => l.trim())
        .filter((l) => l.startsWith("[") && l.includes(sessionTitle)).length;

      const merged = countAfterSecond >= 2 && countAfterSecond > countAfterFirst && rowsForEnv === 1;
      return {
        pass: merged,
        detail: merged
          ? `Hub session "${sessionTitle}" merged a second run: runs ${countAfterFirst}→${countAfterSecond} in ONE row`
          : `second run did not merge as run_count+1 in one row (runs ${countAfterFirst}→${countAfterSecond}, rowsForEnv=${rowsForEnv})`,
        evidence: {
          sessionTitle,
          countAfterFirst,
          countAfterSecond,
          rowsForEnv,
          afterFirst,
          afterSecond,
          statesRun1: settled1.seen,
          statesRun2: settled2.seen,
        },
      };
    },
  },
];
