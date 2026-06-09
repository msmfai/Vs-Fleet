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
