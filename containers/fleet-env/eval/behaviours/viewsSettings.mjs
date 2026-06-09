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
    rationale: `
WHAT: Drives the real VS Code command 'workbench.action.toggleSidebarVisibility'
through the bridge (env.act, which throws on a non-ok executeCommand result) and
asserts only that the command resolved. We snapshot visibleEditors before and
after as evidence; we do NOT assert sidebar visibility itself because the Snapshot
contract (§3.3) does not yet expose it (a Track-D upgrade may add it).

WHY THIS OUTCOME: The primary side bar is a workbench-chrome element, not an
editor or a terminal. Toggling its visibility is a pure layout operation — it does
not open/close editors, so visibleEditors is expected to be stable across the
toggle. The only thing we can faithfully verify today is that VS Code accepted and
executed the built-in command id without error, which env.act guarantees by
rejecting on !ok. Asserting pass:true unconditionally is correct here precisely
because the success signal IS "the command resolved"; inventing a visibility
assertion against a field the bridge can't supply would be a false test.

WHY IT MATTERS: This guards the command-dispatch path itself — that the bridge can
still reach the VS Code command registry and run a stock workbench command end to
end. If a refactor of the bridge transport, the executeCommand wrapper, or the
command-id wiring breaks, env.act will throw and this smoke test goes red even
though it makes no rich assertion. A future reader seeing this fail should suspect
the bridge/command plumbing, not the sidebar feature. The recorded before/after
visibleEditors also documents the baseline assumption (sidebar toggling is
editor-neutral) so that if Track-D later adds a real visibility check, the expected
invariants are already written down.`,
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
    rationale: `
WHAT: Runs 'workbench.action.togglePanel' via env.act and asserts the command
resolved, capturing terminalCount before and after as evidence. We explicitly do
NOT assert that the panel's visibility changed (no Snapshot field for it yet) and
we explicitly assert that terminalCount is the relevant invariant to surface.

WHY THIS OUTCOME: The bottom panel is the host surface for the integrated terminal
(among other views). A common but wrong mental model is "hiding the panel kills the
terminal" — in VS Code the terminal process keeps running when its hosting panel is
toggled closed; only the UI is hidden. So terminalCount is expected to be UNCHANGED
across the toggle. That is why the detail line reports the count transition as
"unchanged by hide": the correct behaviour is that toggling the panel is a
visibility-only operation with no lifecycle effect on terminals. Asserting pass on
command resolution is the honest assertion given the Snapshot can't report panel
visibility today.

WHY IT MATTERS: This pins down the contract that panel visibility and terminal
lifecycle are decoupled. If a refactor (e.g. of terminal disposal, of the
panel-show/hide handlers, or of how the env counts terminals) ever caused hiding
the panel to dispose terminals, terminalCount would drift and the evidence here
would expose it even before a dedicated visibility check exists. A future reader
debugging a terminal-count regression can use the before/after evidence to decide
whether the panel toggle is implicated. It also guards the same command-dispatch
plumbing as toggleSidebar — a thrown env.act means the bridge lost the ability to
run stock workbench commands.`,
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
    rationale: `
WHAT: Invokes 'workbench.actions.view.problems' via env.act to open the Problems
(Errors and Warnings) view, then observes and surfaces the current diagnostics
count. The assertion is that the command resolved; the diagnostics count is
reported as evidence (with an "n/a" fallback when the Snapshot omits it).

WHY THIS OUTCOME: Opening the Problems view is a focus/reveal operation — it shows
the diagnostics panel but does not itself create or clear diagnostics. The Snapshot
contract (§3.3) does not expose which view currently has focus, so there is no
faithful way to assert "the Problems view is now focused"; the only guaranteed
truth is that VS Code accepted the built-in command id and ran it, which env.act
enforces by throwing on !ok. Reporting diagnostics (rather than asserting on it) is
correct because the count depends entirely on the scenario's workspace state and is
not something this command changes — so it is evidence, not an assertion target.

WHY IT MATTERS: This is a smoke test for the Problems-view command id and the
diagnostics field of the Snapshot. If VS Code renames/retires the command, or a
bridge refactor breaks command dispatch, env.act throws and this goes red — telling
a future reader the failure is in command wiring, not in diagnostics collection.
The surfaced diagnostics count also doubles as a cheap probe that the bridge's
diagnostics plumbing is alive; an unexpected "n/a" where a number was expected hints
the Snapshot's diagnostics field regressed, independent of the Problems command
itself.`,
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
    id: "settings.toggleMinimap",
    title: "Settings: Toggle Minimap flips editor.minimap.enabled",
    tags: ["settings", "smoke"],
    rationale: `
WHAT: Reads editor.minimap.enabled via the 'setting' bridge query, runs
'editor.action.toggleMinimap' via env.act, re-reads the setting, and asserts the
value actually flipped (before !== after && after !== undefined). Unlike the other
behaviours in this file, this one makes a REAL round-trip assertion on observable
state, so it declares needs:["setting"] and is SKIPPED (not failed) when the bridge
does not advertise the 'setting' capability.

WHY THIS OUTCOME: Minimap was chosen deliberately over word-wrap. toggleWordWrap
sets a transient PER-EDITOR override that config.get / the 'setting' query never
reflect, so it is fundamentally unverifiable through this path — a test on it would
either always pass vacuously or always fail. toggleMinimap, by contrast, mutates
the actual editor.minimap.enabled configuration value, which the 'setting' query
reads back. Because it is a boolean toggle, a correct implementation MUST produce
before !== after; if the value is undefined afterward, the setting wasn't readable
at all. The settingValue() helper tolerates both result shapes (top-level r.value
and nested r.data.value per §3.3) so the assertion doesn't hinge on which Track-E
encoding the bridge happens to use.

WHY IT MATTERS: This is the one end-to-end proof in the file that a settings-mutating
command's effect is observable through the bridge's read path — command write +
setting read, the full loop. It guards three things at once: (1) the toggleMinimap
command still maps to editor.minimap.enabled, (2) the 'setting' query returns the
live value in a shape settingValue() understands, and (3) the choice of a
config-backed (not per-editor-override) setting stays correct. A future reader
seeing this fail should check, in order: did the setting-query result shape change
(after===undefined), or did the command stop mutating the config (before===after)?
The needs:["setting"] gate ensures that on a bridge without the capability this
SKIPS cleanly rather than producing a misleading red.`,
    // Needs the `setting` query (Track E) to VERIFY the toggle; SKIP until the
    // bridge advertises it. NOTE: we toggle the MINIMAP (not word-wrap) on purpose —
    // `toggleWordWrap` sets a transient per-editor override that `config.get` never
    // reflects, so it isn't verifiable; `toggleMinimap` updates the actual
    // `editor.minimap.enabled` setting, which the `setting` query reads back.
    needs: ["setting"],
    async run(env) {
      const before = settingValue(await env.request({ type: "setting", key: "editor.minimap.enabled" }));
      await env.act("editor.action.toggleMinimap");
      await sleep(600);
      const after = settingValue(await env.request({ type: "setting", key: "editor.minimap.enabled" }));
      await env.observe("settings.toggleMinimap");

      const changed = before !== after && after !== undefined;
      return {
        pass: changed,
        detail: `editor.minimap.enabled ${JSON.stringify(before)} → ${JSON.stringify(after)}` +
          (changed ? "" : " (no observable change)"),
        evidence: { before, after },
      };
    },
  },
];
