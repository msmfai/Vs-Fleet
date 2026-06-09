// Core / palette / views behaviours. Ported from harness.mjs ("Command Palette
// opens"). Views/panels/palette behaviours (Track B §6) land in THIS file.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  {
    id: "palette.open",
    title: "Command Palette opens",
    tags: ["core", "palette", "smoke"],
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
