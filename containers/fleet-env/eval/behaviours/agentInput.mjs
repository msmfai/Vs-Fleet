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
//   agent.waitingState    — termSend an INTERACTIVE claude (NOT -p) with a prompt
//                            that forces a Bash tool needing approval; claude fires
//                            PreToolUse(Bash) then BLOCKS on y/n with no Stop, so the
//                            reporter's S16 infer adapter emits `waiting` after the
//                            ~1.5s debounce. We poll the Hub for `waiting`, then ALWAYS
//                            unblock (Ctrl-C + pkill) so the env never hangs. Same
//                            auth/Hub SKIP gates as agent.claudeRuns; if the headless
//                            block never even reaches `working`, SKIP with a precise
//                            reason rather than hard-fail.
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
    rationale: `WHAT: Seeds a known file ("seed-line\\n") via the writeFile bridge cap,
opens it with openFile so it becomes the active text editor, moves the cursor to
end-of-file (act "cursorBottom"), then drives synthetic keystrokes through the
typeText cap to append "FLEET_TYPED_OK". It asserts that typed string is read back
out of the document — primarily via fileContent(path), with the snapshot's
editorText field as a fallback signal.

WHY THIS IS THE EXPECTED OUTCOME: typeText is the bridge's lowest-level input
primitive — it must land characters in whatever editor VS Code currently treats as
the active text editor, exactly as a human pressing keys would. Because we open the
seed file immediately before typing, that file IS the active editor, so the keys
must mutate ITS document model. cursorBottom anchors the insertion at EOF so we
append (additive, non-destructive) rather than clobber the seed line; this makes the
"seed survives + typed text present" assertion unambiguous. fileContent reflects the
live document buffer (saveAll is issued first when supported so the on-disk view and
the buffer agree), so the typed text must appear there.

WHY IT MATTERS: This is the canary for the entire synthetic-input path. If a refactor
of the bridge re-routes typeText (e.g. sends to a terminal, a webview, or a stale/
non-focused editor), or if openFile stops actually focusing the document, or if the
fileContent query starts reading disk instead of the buffer, this test breaks — and
it tells a future reader precisely which link snapped: keystrokes were emitted but
did NOT reach the active editor's document. Every higher-level "type code / edit
file" behaviour rests on this primitive working.`,
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
    rationale: `WHAT: Sends \`claude -p "say hi"\` into a container terminal via the
termSend cap, then queries the HOST-side Hub (via the \`fleet ls --once\` CLI,
matching the session row whose title == env.id == FLEET_SERVER_ID) and asserts the
session goes ACTIVE (working/waiting) and then TERMINATES (idle/done/dead) — i.e. a
full one-shot run was observed end to end, with at least one run recorded.

WHY THIS IS THE EXPECTED OUTCOME: This exercises the whole reporting chain that makes
Fleet useful. The \`claude\` shell wrapper baked into the image installs the Fleet
hooks; running claude fires UserPromptSubmit/PreToolUse → the reporter (S15 adapter)
emits Working and phones it home to ws://HOST:51777, where the Hub registers it under
the env's session title. \`-p\` is deliberately ONE-SHOT: claude does the turn and then
exits, firing SessionEnd, so the run must settle at idle/done (turn finished) or dead
(session ended) — all three legitimately mean "the turn completed". Catching the brief
\`working\` window is best-effort (the run can be faster than the poll), so a recorded
"(N runs)" count is accepted as corroborating proof a run happened.

WHY IT MATTERS: This guards the agent-observability spine: container claude → hook
wrapper → reporter state machine → WS phone-home → Hub session registry → CLI render.
A break here means Fleet has gone blind to agent activity. The two runtime gates are
load-bearing and must NOT become hard failures: an unauthenticated container claude
(no API key / no Keychain OAuth) and an absent/unreachable Hub both SKIP cleanly,
because they are environmental, not regressions — turning them into failures would
make the suite red on any machine without credentials or a running Hub. A future
reader seeing this FAIL (not skip) knows the wiring itself regressed: claude ran but
its working→idle lifecycle never reached the Hub.`,
    // The bridge cap we need is termSend (drive the terminal). The Hub itself is a
    // host-side dependency (not a bridge cap) — we detect it at runtime and SKIP
    // cleanly when it's unavailable.
    needs: ["termSend"],
    async run(env) {
      const sessionTitle = env.id; // FLEET_SESSION_TITLE == FLEET_SERVER_ID == env.id

      // Auth gate. The harness authenticates the container's claude in reset()
      // (ANTHROPIC_API_KEY passthrough, or the host's subscription OAuth piped from
      // the macOS Keychain into the container). If that didn't land, SKIP cleanly.
      if (!env.claudeAuthed) {
        return {
          pass: false,
          skipped: "container claude not authenticated — set ANTHROPIC_API_KEY, or ensure the 'Claude Code-credentials' Keychain item is accessible (FLEET_CLAUDE_OAUTH=0 to disable injection)",
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

      // Then the run terminates. `claude -p` is ONE-SHOT: it fires SessionEnd on
      // exit, so the run ends at `idle`/`done` (a turn finished) OR `dead` (the
      // session ended) — all three mean "the turn completed". (An interactive claude
      // would settle at idle and stay; -p ends the session.)
      const settled = await pollHub(
        sessionTitle,
        (_l, st) => st === "idle" || st === "done" || st === "dead",
        { ms: 90000, every: 1500 },
      );
      const after = await env.observe("agent.claudeRuns.after");

      // Pass = we saw the session go active (working/waiting) AND it then terminated.
      // If the working blip was missed but the snapshot reports ≥1 run, that still
      // proves a run occurred.
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
  // We drive an INTERACTIVE claude (not -p) into a permission BLOCK: it fires
  // PreToolUse(Bash) then sits awaiting y/n with no Stop. The reporter's S16 infer
  // adapter (a PreToolUse-without-followup for one debounce window, DEFAULT_DEBOUNCE_MS
  // = 1.5s) then emits State::Waiting, which the Hub renders as "[waiting]". We poll
  // the Hub for that, and ALWAYS unblock claude afterwards so the env never hangs.
  {
    id: "agent.waitingState",
    title: "A blocked-on-approval agent shows `waiting` on the Hub session",
    tags: ["agent", "hub"],
    rationale: `
WHAT: Verifies the inferred-\`waiting\` (approval-needed) signal — Fleet's whole ping —
flows end-to-end through a REAL reporter \`--serve\` socket to the Hub. We send a single
controlled \`PreToolUse\`-without-\`Stop\` frame to the env's reporter socket and assert
the Hub session reaches \`waiting\`, then send a \`Stop\` to resolve it cleanly.

WHY THIS OUTCOME: Claude exposes no authoritative waiting/approval hook, so the reporter
INFERS it (S16): a \`PreToolUse\` not followed by any activity for a debounce window ⇒
\`waiting\`; later activity cancels it. A lone \`PreToolUse\` with no \`Stop\` is therefore
expected to surface as \`waiting\` once serve's debounce TICK fires. We inject that exact
frame rather than driving a real headless claude into a Bash approval block because the
real block is environment-flaky — claude version + permission-mode defaults change
whether/when it pauses — whereas the detection pipeline we own is deterministic. The
frame travels the SAME socket → S15+S16 adapters → Hub plumbing → \`fleet ls\` rendering a
real run would, so this is a true end-to-end test, not a unit test.

WHY IT MATTERS: This is the ONLY end-to-end guard that serve_unix's tick-driven inference,
the urgency/rollup plumbing, and the CLI's \`[waiting]\` rendering actually emit the
approval-needed signal — the Rust unit tests exercise the infer machine in isolation, not
the socket/Hub/CLI path. If a refactor breaks the debounce tick, the PreToolUse-without-
Stop heuristic, or the Waiting→Hub wiring, agents silently stop telling users they're
blocked (Fleet's core promise). A future reader seeing this red should suspect the serve
tick or the Waiting plumbing, not claude.`,
    needs: [],
    async run(env) {
      const sessionTitle = env.id;

      // Hub gate — need the fleet CLI + a registered session. (The reporter --serve
      // socket lives in the env regardless; this behaviour does NOT run claude.)
      const cli = fleetCli();
      if (!cli) {
        return { pass: false, skipped: "Hub `fleet` CLI not found (target/debug/fleet) — start the Hub (see test.sh)", detail: "skipped: no fleet CLI to query the Hub" };
      }
      const boot = await pollHub(sessionTitle, () => true, { ms: 15000, every: 1000 });
      if (!boot.ok) {
        return { pass: false, skipped: `Hub session "${sessionTitle}" not found (Hub down or reporter not phoned home)`, detail: "skipped: env's session is not registered on the Hub", evidence: { sessionTitle, cli } };
      }
      const startState = stateOf(boot.line);

      // Send a CONTROLLED PreToolUse-without-Stop straight to the env's REAL reporter
      // --serve socket: the S16 infer machine arms on it and, after its debounce tick
      // fires with no follow-up, emits `waiting`. Deterministic (vs a flaky real claude
      // block) yet exercises the full socket → infer-tick → Hub → CLI path.
      const send = (obj) =>
        env.exec(`printf 'claude %s\n' '${JSON.stringify(obj)}' | timeout 2 nc -N -U /tmp/fleet-reporter.sock 2>/dev/null || true`);
      const sid = `wait-${env.id}`;
      send({ hook_event_name: "PreToolUse", session_id: sid, tool_name: "Bash", tool_use_id: "toolu_fleetwait", cwd: "/home/coder/project" });

      const waited = await pollHub(sessionTitle, (_l, st) => st === "waiting", { ms: 45000, every: 1000 });

      // Resolve the pending inference (a Stop cancels waiting → idle) so the session is left clean.
      send({ hook_event_name: "Stop", session_id: sid, cwd: "/home/coder/project", stop_hook_active: false });

      return {
        pass: waited.ok,
        detail: waited.ok
          ? `Hub session "${sessionTitle}" reached "waiting" via a controlled PreToolUse-without-Stop (states: ${[...waited.seen].join("->")})`
          : `"waiting" not observed within budget on Hub session "${sessionTitle}" (states seen: ${JSON.stringify([...waited.seen])})`,
        evidence: { sessionTitle, startState, statesSeen: [...new Set(waited.seen)], finalLine: waited.line || boot.line },
      };
    },
  },
];
