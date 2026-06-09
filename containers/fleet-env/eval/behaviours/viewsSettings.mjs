// Views / panels + Settings behaviours (Track B, §6 "Views/panels/palette" +
// "Settings"). Each behaviour drives a real VS Code command via the bridge and
// asserts the effect through a snapshot/setting query where the bridge exposes
// one; otherwise it asserts the command resolved (executeCommand returned ok,
// which `env.act` enforces by throwing on !ok).
//
// Self-contained per §6/§3 contracts. See behaviours/_contract.mjs for shapes.
//
//   view.toggleSidebar      — toggle the primary side bar visibility
//   view.togglePanel        — toggle the bottom panel visibility
//   problems.open           — open the Problems view
//   settings.toggleWordWrap — toggle editor.wordWrap (needs:["setting"] to verify)

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Pull a setting value out of a raw bridge result. §3.3 says `setting {key} →
// {value}`; Track E may put it at the top level (`r.value`) or under `r.data`
// (mirroring how `query` returns the Snapshot under `.data`). Tolerate both.
function settingValue(r) {
  if (!r || typeof r !== "object") return undefined;
  if ("value" in r) return r.value;
  if (r.data && typeof r.data === "object" && "value" in r.data) return r.data.value;
  return undefined;
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  {
    id: "view.toggleSidebar",
    title: "View: Toggle Primary Side Bar",
    tags: ["views", "smoke"],
    // Baseline {command,query} only. We can't yet read sidebar visibility from the
    // Snapshot (Track-D may add it), so we assert the command resolved and record
    // the snapshot before/after as evidence.
    async run(env) {
      const before = await env.observe("view.toggleSidebar.before");
      await env.act("workbench.action.toggleSidebarVisibility");
      await sleep(600);
      const after = await env.observe("view.toggleSidebar.after");
      return {
        pass: true,
        detail: "toggleSidebarVisibility resolved (visibility not yet in Snapshot)",
        evidence: {
          beforeVisibleEditors: before.vscode.visibleEditors,
          afterVisibleEditors: after.vscode.visibleEditors,
        },
      };
    },
  },

  {
    id: "view.togglePanel",
    title: "View: Toggle Panel",
    tags: ["views", "panel", "smoke"],
    // Baseline only. The bottom panel hosts the terminal; toggling it does NOT
    // change terminalCount (terminals stay alive when hidden), so we assert the
    // command resolved. Track-D can upgrade this to a visibility assertion.
    async run(env) {
      const before = await env.observe("view.togglePanel.before");
      await env.act("workbench.action.togglePanel");
      await sleep(600);
      const after = await env.observe("view.togglePanel.after");
      return {
        pass: true,
        detail: "togglePanel resolved" +
          ` (terminalCount ${before.vscode.terminalCount} → ${after.vscode.terminalCount}, unchanged by hide)`,
        evidence: {
          beforeTerminalCount: before.vscode.terminalCount,
          afterTerminalCount: after.vscode.terminalCount,
        },
      };
    },
  },

  {
    id: "problems.open",
    title: "View: Open Problems (Errors and Warnings)",
    tags: ["views", "problems", "smoke"],
    // Baseline only. We can't read which view is focused from the Snapshot yet, so
    // we assert the command resolved and surface the current diagnostics count.
    async run(env) {
      await env.act("workbench.actions.view.problems");
      await sleep(600);
      const after = await env.observe("problems.open");
      return {
        pass: true,
        detail: `Problems view opened (diagnostics: ${after.vscode.diagnostics ?? "n/a"})`,
        evidence: { diagnostics: after.vscode.diagnostics },
      };
    },
  },

  {
    id: "settings.toggleWordWrap",
    title: "Settings: Toggle Word Wrap reflects in the setting",
    tags: ["settings", "smoke"],
    // Needs the `setting` query (Track E) to VERIFY the toggle; SKIP cleanly until
    // the bridge advertises it. The command itself (toggleWordWrap) only needs the
    // baseline, but the assertion is the whole point — so gate on `setting`.
    needs: ["setting"],
    async run(env) {
      // Read the effective editor.wordWrap before and after toggling. The toggle
      // command flips between "off" and "on" (VS Code's editor.action.toggleWordWrap
      // sets a per-editor override; the `setting` query reflects the effective value).
      const beforeRaw = await env.request({ type: "setting", key: "editor.wordWrap" });
      const before = settingValue(beforeRaw);

      await env.act("editor.action.toggleWordWrap");
      await sleep(600);

      const afterRaw = await env.request({ type: "setting", key: "editor.wordWrap" });
      const after = settingValue(afterRaw);
      await env.observe("settings.toggleWordWrap");

      // Pass when the effective value actually changed. If the bridge returns
      // undefined (no editor open / key not resolvable) we still pass on a defined
      // change but flag the ambiguity in detail.
      const changed = before !== after && after !== undefined;
      return {
        pass: changed,
        detail: `editor.wordWrap ${JSON.stringify(before)} → ${JSON.stringify(after)}` +
          (changed ? "" : " (no observable change)"),
        evidence: { before, after },
      };
    },
  },
];
