// Agent + Input behaviours (Track B — §6 "Agent" + "Input").
//
//   input.typeIntoEditor  — writeFile a seed file, openFile it, typeText into the
//                            active editor, then assert the change via fileContent
//                            (or the editorText snapshot field when the bridge
//                            ships it). Needs Track-E caps: writeFile/openFile/
//                            typeText + (fileContent | query.editorText).
//
//   agent.claudeRuns      — termSend `claude -p "say hi"` into a terminal, then
//                            assert the env's **Hub session** advances through a
//                            working→idle (or done) run. The Hub runs on the HOST
//                            (the harness/integration starts it on :51777, see
//                            containers/fleet-env/test.sh); we query it with the
//                            `fleet` CLI (`target/debug/fleet ls --once`). The env's
//                            session is titled by FLEET_SERVER_ID (== env.id), so we
//                            match its row by that title. Needs cap: termSend, AND a
//                            reachable Hub — if the Hub/CLI is absent we SKIP cleanly
//                            (never hard-fail).
//
//   agent.waitingState    — documented TODO stub: a prompt that triggers an approval
//                            should drive the Hub session to `waiting`. Triggering an
//                            approval headlessly is non-trivial (needs a hook that
//                            blocks on user input); SKIPped until that's wired.
//
// See behaviours/_contract.mjs for the Behaviour shape and §3.3 for the wire/caps.
// Patterns copied from the proven terminal.new / core.palette behaviours.

import { execSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { existsSync } from "node:fs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ─── Host-side Hub query (the Hub lives on the host, not in the container) ──────
// The reporter inside each env phones home to ws://HOST:51777 and registers a
// session titled by FLEET_SERVER_ID. We read the live snapshot off the host with
// the `fleet` CLI. `fleet ls --once` prints one line per session of the form
//   [<state>]<unread> <title>  (<n> run[s])[ <urgency>]
// where <state> ∈ {working,waiting,idle,done,error,dead} (crates/fleet-cli render).

const HERE = dirname(fileURLToPath(import.meta.url));
// eval/  →  …/containers/fleet-env/eval ; repo root is four levels up of HERE's
// grandparent. Resolve the workspace `fleet` binary the same way test.sh does
// (ROOT/target/debug/fleet); fall back to a `fleet` on PATH.
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

// Run `fleet ls --once` and return its raw stdout (""/null on any failure). The
// CLI honors FLEET_WS_URL for the Hub address; the integration sets that (or the
// default ws://127.0.0.1:51777 matches test.sh's host-bound Hub).
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
    return null; // Hub unreachable / CLI errored → caller treats as unavailable.
  }
}

// Is the Hub reachable AND does it already know this env's session? (the reporter
// registers the session on boot, before any agent run). Returns the matched line
// or null.
function sessionLineFor(snapshot, sessionTitle) {
  if (!snapshot) return null;
  for (const raw of snapshot.split("\n")) {
    const line = raw.trim();
    // Session rows look like "[idle] <title> …"; the title equals the env id.
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

// Poll the Hub until `match(line)` is true for this session, or timeout. Records
// every distinct state seen (so evidence can show the working→idle transition).
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

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ── Input: typeText lands in the active editor ───────────────────────────────
  {
    id: "input.typeIntoEditor",
    title: "Typing into the editor changes its content",
    tags: ["input", "editor"],
    // writeFile+openFile seed/open the file; typeText drives the keystrokes;
    // fileContent reads it back. All are Track-E caps → SKIP until E ships them.
    needs: ["writeFile", "openFile", "typeText", "fileContent"],
    async run(env) {
      const path = "/home/coder/project/fleet-input.txt";
      const seed = "seed-line\n";
      const typed = "FLEET_TYPED_OK";

      // Seed a known file and open it so the editor is the active text editor.
      await env.request({ type: "writeFile", path, content: seed });
      await env.request({ type: "openFile", path });
      await sleep(800);
      const before = await env.observe("input.typeIntoEditor.before");

      // Move to end-of-file then type — so we append rather than overwrite. (The
      // cursor lands at the doc start on open; end-of-file is a safe anchor.)
      await env.act("cursorBottom").catch(() => {});
      await env.request({ type: "typeText", text: typed });
      await sleep(800);

      // Persist if the bridge offers saveAll (so fileContent reflects the buffer);
      // otherwise fileContent of an unsaved buffer should still reflect the doc.
      if (env.supports("saveAll")) await env.request({ type: "saveAll" }).catch(() => {});
      await sleep(500);
      const after = await env.observe("input.typeIntoEditor.after");

      // Primary assertion: fileContent contains the typed text.
      let text = "";
      try {
        const r = await env.request({ type: "fileContent", path });
        text = (r && (r.text ?? r.data?.text)) || "";
      } catch {}
      // Fallback signal: the snapshot's editorText (Track-D/E), if present.
      const editorText = after.vscode.editorText ?? before.vscode.editorText ?? "";

      const pass = text.includes(typed) || String(editorText).includes(typed);
      return {
        pass,
        detail: pass
          ? `typed "${typed}" appears in ${path}`
          : `typed "${typed}" not found (fileContent=${JSON.stringify(text.slice(0, 80))},` +
            ` editorText=${JSON.stringify(String(editorText).slice(0, 80))})`,
        evidence: {
          path, typed,
          fileContent: text.slice(0, 200),
          editorText: String(editorText).slice(0, 200),
        },
      };
    },
  },

  // ── Agent: `claude -p` produces a working→idle run on the Hub session ─────────
  {
    id: "agent.claudeRuns",
    title: "claude -p drives the env's Hub session working→idle",
    tags: ["agent", "hub"],
    // The bridge cap we need is termSend (drive the terminal). The Hub itself is a
    // host-side dependency (not a bridge cap) — we detect it at runtime and SKIP
    // cleanly when it's unavailable.
    needs: ["termSend"],
    async run(env) {
      const sessionTitle = env.id; // FLEET_SESSION_TITLE == FLEET_SERVER_ID == env.id

      // Auth gate. claude must be authenticated inside the container. macOS Keychain
      // auth isn't mountable into Linux, so without ANTHROPIC_API_KEY (forwarded by
      // the harness) or a ~/.claude/.credentials.json file, claude can't run — SKIP.
      const hasAuth = !!process.env.ANTHROPIC_API_KEY ||
        (process.env.HOME && existsSync(`${process.env.HOME}/.claude/.credentials.json`));
      if (!hasAuth) {
        return {
          pass: false,
          skipped: "container claude not authenticated — set ANTHROPIC_API_KEY (or have ~/.claude/.credentials.json); macOS Keychain auth can't be mounted",
          detail: "skipped: no claude auth available to the container",
        };
      }

      // Hub-availability gate. The integration phase starts the Hub on the host;
      // if it isn't reachable (or `fleet` CLI is absent / session never registered),
      // SKIP cleanly rather than hard-fail. `skipped` is honored by the reporter
      // (console/JUnit/HTML/summary all branch on it).
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
      const startState = stateOf(boot.line); // typically "idle"

      // Drive the agent: send the prompt into a terminal. termSend opens/uses a
      // terminal and writes the line; the `claude` shell wrapper installs the Fleet
      // hooks so the run is reported to the Hub (working on start, idle/done on stop).
      const before = await env.observe("agent.claudeRuns.before");
      await env.request({ type: "termSend", text: 'claude -p "say hi"\n' });

      // Observe the run advance to `working` (the agent started). Don't fail hard if
      // the working window is too brief to catch — the terminal-state below confirms.
      const working = await pollHub(
        sessionTitle,
        (_l, st) => st === "working" || st === "waiting",
        { ms: 30000, every: 750 },
      );

      // Then settle back to idle/done (the run finished). This is the success edge.
      const settled = await pollHub(
        sessionTitle,
        (_l, st) => st === "idle" || st === "done",
        { ms: 90000, every: 1500 },
      );
      const after = await env.observe("agent.claudeRuns.after");

      // Pass = we saw the session transition (working/waiting observed) AND it ended
      // back at idle/done. If the working blip was missed but the session ends idle
      // AND the snapshot now reports ≥1 run, that still proves a run occurred.
      const sawActive = working.ok || settled.seen.includes("working") ||
        settled.seen.includes("waiting");
      const endedQuiet = settled.ok;
      const endLine = settled.line || working.line || boot.line;
      const hasRun = /\(\d+ runs?\)/.test(endLine || "");

      const pass = endedQuiet && (sawActive || hasRun);
      return {
        pass,
        detail: pass
          ? `Hub session "${sessionTitle}": ${startState}→${[...settled.seen].join("→")}` +
            ` (run completed)`
          : `no working→idle run observed on Hub session "${sessionTitle}"` +
            ` (states seen: ${JSON.stringify([...working.seen, ...settled.seen])})`,
        evidence: {
          sessionTitle, startState,
          statesSeen: [...new Set([...working.seen, ...settled.seen])],
          finalLine: endLine,
          terminalsBefore: before.vscode.terminalCount,
          terminalsAfter: after.vscode.terminalCount,
        },
      };
    },
  },

  // ── Agent: an approval-triggering prompt drives the session to `waiting` ──────
  // TODO(track-E + hooks): triggering an approval headlessly needs a Claude run that
  // hits a tool the Fleet approval hook blocks on (e.g. a destructive Bash command)
  // AND a way to observe `waiting` without auto-approving. Until that path exists we
  // SKIP cleanly. The assertion, once wired, mirrors agent.claudeRuns but matches
  // st === "waiting" (urgency "[approval]") on the Hub session.
  {
    id: "agent.waitingState",
    title: "An approval-triggering prompt shows `waiting` on the Hub session",
    tags: ["agent", "hub", "todo"],
    needs: ["termSend"],
    async run(_env) {
      return {
        pass: false,
        skipped: "TODO: headless approval-triggering not yet wired (needs an" +
          " approval hook that blocks + a non-auto-approving observe path)",
        detail: "skipped: documented TODO stub (agent.waitingState)",
      };
    },
  },
];
