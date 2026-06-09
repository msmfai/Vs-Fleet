// Core / palette / views behaviours. Ported from harness.mjs ("Command Palette
// opens"). Views/panels/palette behaviours (Track B §6) land in THIS file.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  {
    id: "palette.open",
    title: "Command Palette opens",
    tags: ["core", "palette", "smoke"],
    rationale: `
WHAT: Drives the bridge to executeCommand("workbench.action.showCommands") —
the built-in VS Code action behind the Command Palette (Ctrl/Cmd+Shift+P) — and
asserts only that the call returns ok (env.act throws on a non-ok bridge reply,
so reaching the return is itself the assertion). It then observe()s under the
"palette.open" tag to capture a screenshot/snapshot for the report.

WHY THIS IS THE EXPECTED OUTCOME: "workbench.action.showCommands" is one of the
most fundamental, always-registered commands in a VS Code / code-server
workbench; it has no preconditions (no file open, no selection, no extension
required) and never fails on a healthy editor host. So a successful, error-free
executeCommand round-trip is exactly what a live, responsive workbench produces.
We deliberately do NOT assert palette-visible state here: the quick-input widget
is a transient overlay that the headless snapshot does not reliably expose, so
asserting on it would be flaky. The honest, stable contract at this layer is
"the editor accepted and ran a core command".

WHY IT MATTERS: This is the smoke test that proves the whole bridge round-trip is
alive — JSON-RPC/WebSocket from harness → reporter → VS Code extension host →
command execution → ok reply. If the container, the bridge wiring, or command
registration regresses (e.g. a refactor breaks the act() transport, the
extension fails to activate, or the workbench never finishes booting), this is
the first and cheapest test to go red. A future reader seeing this fail should
suspect the transport/activation layer itself, not palette-specific logic,
before touching any higher-level behaviour.`,
    async run(env) {
      await env.act("workbench.action.showCommands");
      await sleep(800);
      await env.observe("palette.open");
      return {
        pass: true,
        detail: "executeCommand(showCommands) returned ok",
        evidence: {},
      };
    },
  },
];
