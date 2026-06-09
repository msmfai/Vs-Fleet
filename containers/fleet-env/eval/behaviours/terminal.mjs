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
