// Terminal behaviours. Ported from harness.mjs ("Terminal: New Terminal" — the
// proven baseline, terminalCount 0→1). New terminal behaviours land in THIS file
// per area (Track B). See behaviours/_contract.mjs for the Behaviour shape.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  {
    id: "terminal.new",
    title: "Terminal: New Terminal opens a terminal",
    tags: ["terminal", "smoke"],
    // needs only the baseline {command,query} — always available.
    rationale: `
WHAT: Snapshots terminalCount, fires the built-in command
'workbench.action.terminal.new' through the bridge act() channel, waits 2s, and
asserts the post-action terminalCount is strictly greater than the pre-action
count. This is the single most basic terminal assertion in the suite — "an
integrated terminal actually came into existence when we asked for one."

WHY THIS IS THE EXPECTED OUTCOME: 'workbench.action.terminal.new' is VS Code's
canonical command for creating an integrated terminal. When invoked it spawns a
backing pty/shell process and registers a Terminal object with the workbench;
our query bridge reads window.terminals and reports terminalCount. So a healthy
env must show count rising by (at least) one. We assert ">" rather than "== +1"
deliberately: this is the SHARED-isolation smoke test, so the env may already
carry terminals from earlier behaviours — the only invariant we can rely on is
monotonic growth, not an exact value. The 2s sleep covers the async gap between
the command resolving and the pty registering in the snapshot.

WHY IT MATTERS: This is the proven baseline ported straight from harness.mjs
(terminalCount 0→1) and the canary for the entire bridge round-trip — if act()
can dispatch a command, VS Code can execute it, and query() can observe the
result, then the whole command→effect→observation pipeline is alive. If this
ever breaks after a refactor, suspect the bridge wiring (act/query channel,
command id rename, or the query no longer reading window.terminals) BEFORE
suspecting terminals themselves; every richer terminal behaviour builds on this
exact mechanism, so a green here narrows a failure elsewhere to that behaviour's
own specifics.`,
    async run(env) {
      const before = await env.observe("terminal.new.before");
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      const after = await env.observe("terminal.new.after");
      return {
        pass: after.vscode.terminalCount > before.vscode.terminalCount,
        detail: `terminals ${before.vscode.terminalCount} → ${after.vscode.terminalCount}` +
          ` (${JSON.stringify(after.vscode.terminals)})`,
        evidence: {
          before: { terminalCount: before.vscode.terminalCount, terminals: before.vscode.terminals },
          after: { terminalCount: after.vscode.terminalCount, terminals: after.vscode.terminals },
        },
      };
    },
  },
];
