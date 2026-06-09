// Terminal behaviours beyond terminal.new (Track B). One self-contained file the
// registry auto-discovers; terminal.mjs (Track A's `terminal.new`) is NOT touched.
// See behaviours/_contract.mjs for the Behaviour shape and §3.3 for the wire.
//
// Each behaviour drives a real action and ASSERTS the effect via the snapshot
// (`terminalCount`/`terminals`) or via the `terminalText` query — never "command
// returned ok". Behaviours that depend on Track-E caps declare `needs:[...]` so the
// runner SKIPS them cleanly until the bridge advertises those caps.
//
// Why `isolation:"fresh"` on split/kill: terminalCount is process-global within an
// env, and `terminal.new` (shared) leaves a terminal behind. A fresh env starts at
// terminalCount 0, so the +1 / →0 arithmetic is deterministic and not flaky.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Poll the `terminalText` query until it contains `needle` (the shell takes a beat
// to echo). Returns the last buffer seen so callers can show it in `detail`.
async function waitForTerminalText(env, needle, { name, tries = 15, gap = 800 } = {}) {
  let text = "";
  for (let i = 0; i < tries; i++) {
    await sleep(gap);
    const r = await env.request({ type: "terminalText", ...(name ? { name } : {}) }).catch(() => null);
    text = (r && r.text) || "";
    if (text.includes(needle)) return { hit: true, text };
  }
  return { hit: false, text };
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // terminal.split — split the active terminal → one more terminal in the workbench.
  // The snapshot exposes only terminal names/count (not group membership), so we
  // assert terminalCount +1 and note that the split shares the active group.
  {
    id: "terminal.split",
    title: "Terminal: Split Terminal adds a terminal to the group",
    tags: ["terminal"],
    isolation: "fresh", // deterministic 0→1→2 count from an empty env
    rationale: `
WHAT: In a fresh env (count 0) creates one terminal, snapshots the count, fires
'workbench.action.terminal.split', then asserts the count rose by EXACTLY one
(before+1). Unlike terminal.new (which only requires monotonic growth), this
demands an exact delta because the env is fresh and we control the full 0→1→2
progression.

WHY THIS IS THE EXPECTED OUTCOME: 'terminal.split' requires an active terminal
to split — hence the explicit 'terminal.new' first — and produces a SECOND
terminal pane that shares the active terminal group (side-by-side panes, one
group). Crucially, VS Code models each split pane as its own Terminal object, so
window.terminals grows by one and our snapshot's terminalCount goes 1→2. The
snapshot exposes only flat terminal names/count, NOT group/pane membership, so
"same group" can't be asserted directly — we assert the count delta and note the
grouping in detail. 'fresh' isolation is load-bearing: terminalCount is
process-global within an env, so a leftover shared terminal would make the exact
+1 arithmetic non-deterministic and flaky.

WHY IT MATTERS: Split is distinct from New at the workbench level (it joins a
group rather than opening a standalone terminal) yet must still surface as a new
Terminal object to the observer. If this regresses to count staying at 1, the
split command silently degraded into a no-op or the second pane stopped
registering as a distinct terminal — a refactor of terminal-group handling or of
the query's terminal enumeration. Pairing it with terminal.new lets a future
reader bisect: if new passes but split fails, the fault is split-specific, not
the bridge.`,
    async run(env) {
      // Need an active terminal to split; create one first.
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      const before = await env.observe("terminal.split.before");
      await env.act("workbench.action.terminal.split");
      await sleep(2000);
      const after = await env.observe("terminal.split.after");
      return {
        pass: after.vscode.terminalCount === before.vscode.terminalCount + 1,
        detail: `terminals ${before.vscode.terminalCount} → ${after.vscode.terminalCount}` +
          ` after split (same group; ${JSON.stringify(after.vscode.terminals)})`,
        evidence: {
          before: { terminalCount: before.vscode.terminalCount, terminals: before.vscode.terminals },
          after: { terminalCount: after.vscode.terminalCount, terminals: after.vscode.terminals },
        },
      };
    },
  },

  // terminal.runEcho — send `echo FLEET_OK` into a terminal and read it back via the
  // `terminalText` query. Needs Track-E termSend + terminalText (SKIP until shipped).
  {
    id: "terminal.runEcho",
    title: "Terminal: echo marker round-trips through terminalText",
    tags: ["terminal"],
    needs: ["termSend", "terminalText"],
    rationale: `
WHAT: Opens a terminal, uses the Track-E 'termSend' cap to send the literal
command 'echo FLEET_OK' (termSend appends a newline, so the command actually
runs), then polls the 'terminalText' query until the buffer contains the marker
FLEET_OK. Pass == the marker was observed in the terminal's text buffer.

WHY THIS IS THE EXPECTED OUTCOME: This exercises a full real-shell round-trip,
not just terminal existence. termSend writes keystrokes to the pty's stdin; the
shell receives 'echo FLEET_OK\\n', executes it, and writes 'FLEET_OK' back to the
pty's stdout, which VS Code's terminal renders into its buffer; terminalText reads
that buffer back to us. FLEET_OK is chosen as a marker the shell PROMPT itself
won't contain — so a match proves the echo command genuinely executed and emitted
output, not merely that we typed the word (which would already appear on the input
line). Because shell echo is asynchronous and the prompt takes a beat,
waitForTerminalText polls (15 tries × 800ms) rather than reading once. It declares
needs:[termSend,terminalText]; until the bridge advertises both caps the runner
SKIPS this cleanly rather than failing.

WHY IT MATTERS: terminal.new/split prove a terminal OBJECT exists; this is the
first behaviour proving the terminal is a live, interactive shell that runs
commands and returns output. If it breaks while terminal.new stays green, suspect
the Track-E I/O path specifically — termSend not delivering keystrokes, the
newline-append being dropped (command typed but never run), or terminalText
capturing the input line but not the output stream. It guards the assumption that
agents can drive real work through the terminal and read results back.`,
    async run(env) {
      const MARKER = "FLEET_OK";
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      // termSend appends a newline → runs the command. Use a marker the prompt
      // text itself won't contain, so a match means the echo actually ran.
      const sent = await env.request({ type: "termSend", text: `echo ${MARKER}` });
      const name = sent && sent.terminal;
      const { hit, text } = await waitForTerminalText(env, MARKER, { name });
      await env.observe("terminal.runEcho.after");
      return {
        pass: hit,
        detail: hit
          ? `terminalText contains "${MARKER}" after echo (terminal ${JSON.stringify(name)})`
          : `terminalText never showed "${MARKER}" (last buffer: ${JSON.stringify(text.slice(-120))})`,
        evidence: { marker: MARKER, terminal: name, tail: text.slice(-200) },
      };
    },
  },

  // terminal.kill — open a terminal then kill it → terminalCount back to 0. Fresh env
  // so the starting count is a clean 0 and the round-trip to 0 is unambiguous.
  {
    id: "terminal.kill",
    title: "Terminal: Kill Terminal removes the terminal",
    tags: ["terminal"],
    isolation: "fresh",
    rationale: `
WHAT: In a fresh env, opens a terminal and confirms count >= 1, then FIRES (not
awaits) 'workbench.action.terminal.kill' via env.fire(), waits 2.5s, and asserts
the post-kill count is strictly less than the opened count. This verifies that
killing disposes the terminal and the disposal is reflected in the snapshot.

WHY THIS IS THE EXPECTED OUTCOME: 'terminal.kill' tears down the active terminal —
it disposes the Terminal object and terminates the backing pty/shell process — so
window.terminals shrinks and terminalCount drops below where it was after opening
(fresh env: 1 → 0). The notable mechanism here is env.fire() instead of env.act():
the kill command's executeCommand promise does NOT resolve in this headless
configuration (the disposal teardown leaves the RPC reply hanging), so awaiting it
would stall the test. Instead we fire-and-forget and verify the EFFECT through
observation after a 2.5s settle — observation, not the command's return, is the
source of truth. 'fresh' isolation gives a clean starting count so the round-trip
down is unambiguous. We assert "after < opened" (rather than "== 0") to stay
robust if isolation ever leaks a background terminal.

WHY IT MATTERS: This is the teardown half of the terminal lifecycle and the only
behaviour that proves terminals can be DISPOSED, not just created — important for
resource hygiene (leaked ptys accumulate). Two distinct things can break it: the
kill command no longer disposing the terminal (count stays put), or — subtler —
env.fire() regressing so the un-resolving promise is awaited again and the test
hangs. A future reader seeing a hang vs a count-unchanged failure should look at
the fire-vs-act distinction first, since that asymmetry (kill uses fire, new/split
use act) is the non-obvious detail this rationale exists to preserve.`,
    async run(env) {
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      const opened = await env.observe("terminal.kill.opened");
      // kill's executeCommand promise doesn't resolve headlessly — fire it and
      // verify the effect via observation rather than blocking on a reply.
      env.fire("workbench.action.terminal.kill");
      await sleep(2500);
      const after = await env.observe("terminal.kill.after");
      return {
        pass: opened.vscode.terminalCount >= 1 && after.vscode.terminalCount < opened.vscode.terminalCount,
        detail: `terminals ${opened.vscode.terminalCount} → ${after.vscode.terminalCount} after kill`,
        evidence: {
          opened: { terminalCount: opened.vscode.terminalCount, terminals: opened.vscode.terminals },
          after: { terminalCount: after.vscode.terminalCount, terminals: after.vscode.terminals },
        },
      };
    },
  },

  // terminal.cwd — assert a new terminal opens in the workspace project root.
  // `fresh` env (no shared-terminal pollution); we write `pwd` to a file and read
  // it back via the reliable fileContent query — terminalText output-stream capture
  // depends on shell integration and is racy.
  {
    id: "terminal.cwd",
    title: "Terminal: a new terminal's cwd is the workspace project root",
    tags: ["terminal"],
    isolation: "fresh",
    needs: ["termSend", "fileContent"],
    rationale: `
WHAT: Opens a fresh terminal, sends 'pwd > /tmp/fleet-cwd.txt' via termSend, then
polls the fileContent query (12 tries × 500ms) for that file and asserts its text
contains '/home/coder/project'. I.e. a freshly opened terminal's working
directory is the workspace project root.

WHY THIS IS THE EXPECTED OUTCOME: VS Code opens integrated terminals in the
workspace root by default (terminal.integrated.cwd unset → folder root), and in
this container the workspace folder is /home/coder/project. So 'pwd' in a new
terminal must print exactly that path. The deliberate design choice is HOW we read
the result: rather than scraping the terminal's output stream via terminalText, we
redirect pwd to a file and read it back through fileContent. terminalText output
capture depends on shell-integration / buffer-render timing and is racy; a file on
disk written by the shell is unambiguous and deterministic. We poll because the
shell write is async. needs:[termSend,fileContent] → SKIP until both caps ship;
'fresh' isolation avoids any shared-terminal pollution affecting which terminal is
active when we send.

WHY IT MATTERS: An agent that opens a terminal expecting to be at the project root
will silently run commands in the wrong directory if the default cwd regresses —
builds, git, file edits all land in the wrong place with no error. This guards the
cwd default against changes to the container's workspace layout, a VS Code setting
override, or shell startup (e.g. a profile 'cd' in .bashrc) quietly moving the
starting directory. If it breaks, compare the captured path in evidence.got
against '/home/coder/project': a different real path points at workspace/container
config; an empty file points at the termSend→shell→file write path failing rather
than the cwd being wrong.`,
    async run(env) {
      const EXPECT = "/home/coder/project";
      const marker = "/tmp/fleet-cwd.txt";
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      await env.request({ type: "termSend", text: `pwd > ${marker}` });
      let text = "";
      for (let i = 0; i < 12; i++) {
        await sleep(500);
        const r = await env.request({ type: "fileContent", path: marker }).catch(() => null);
        text = (r && (r.text ?? (r.data && r.data.text))) || "";
        if (text.includes(EXPECT)) break;
      }
      await env.observe("terminal.cwd.after");
      const hit = text.includes(EXPECT);
      return {
        pass: hit,
        detail: hit ? `terminal cwd is ${EXPECT}` : `cwd file lacked ${EXPECT} (got ${JSON.stringify(text.trim().slice(-120))})`,
        evidence: { expected: EXPECT, got: text.trim() },
      };
    },
  },
];
