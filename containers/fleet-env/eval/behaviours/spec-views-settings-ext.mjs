// SPEC areas 16-views-panels + 17-settings + 18-extensions, implemented as real
// container behaviours. One self-contained file the registry auto-discovers; the
// existing viewsSettings.mjs / core.mjs are NOT touched.
//
// Each behaviour drives a real VS Code command/query via the bridge and ASSERTS
// the strongest faithful observable:
//   - chrome-only commands (sidebar/panel/zen/output/view-switch/quickOpen/gotoLine):
//     the Snapshot exposes NO chrome state, so we assert "command resolved ok"
//     (env.act throws on !ok) PLUS the snapshot fields it CAN read as INVARIANTS
//     (must not drift) — never inventing a visibility assertion the bridge can't
//     supply (16-views §header).
//   - count/identity-observable commands (toggleTerminal-from-zero, newUntitledFile,
//     closeActiveEditor): a real snapshot delta/identity assertion.
//   - config-backed settings (minimap/autoSave/zoomLevel/workspace settings.json):
//     a real `setting` read-back round-trip (17-settings THE LESSON: only
//     config-backed values are verifiable through `setting`; per-editor overrides
//     like wordWrap are asserted as "config did NOT move").
//   - the `extensions` query + the self-referential fleet-bridge presence/active +
//     the on-disk bridge activation/command log (18-extensions THE GOTCHA).
//
// NOTE on coverage split: VIEW.001 (palette.open, core.mjs), VIEW.010
// (view.toggleSidebar) / VIEW.020 (view.togglePanel) / VIEW.030 (problems.open)
// (viewsSettings.mjs), and VIEW.071 (quickOpen.byName, files.mjs) live in OTHER
// files and are NOT duplicated here — this file carries the remaining 16-views
// entries plus all of 17-settings and 18-extensions.
//
// See behaviours/_contract.mjs for the Behaviour shape and §3.3 for the wire/caps.
// Idioms copied from files.mjs / terminal_more.mjs / viewsSettings.mjs.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const PROJECT = "/home/coder/project";

// Pull a `setting {key}` value whether the bridge spreads it onto the result msg
// (`r.value`, the §3.3 shape) or nests it under `.data` (the snapshot shape).
function settingValue(r) {
  if (!r || typeof r !== "object") return undefined;
  if ("value" in r) return r.value;
  if (r.data && typeof r.data === "object" && "value" in r.data) return r.data.value;
  return undefined;
}

// Read a setting value through the bridge (tolerant of the result-shape variants).
async function readSetting(env, key) {
  return settingValue(await env.request({ type: "setting", key }));
}

// Length of a snapshot array field, or null if the field isn't exposed.
const lenOf = (arr) => (Array.isArray(arr) ? arr.length : null);

// Names of open terminals as a stable JSON string, or null if not exposed.
const termNames = (snap) => (Array.isArray(snap.terminals) ? JSON.stringify(snap.terminals) : null);

// Does any installed-extensions entry id look like the fleet-bridge?
const isFleetBridge = (id) => /fleet[-.]?bridge/i.test(String(id || ""));

// CAPS frozen list (packages/fleet-bridge/src/extension.ts `CAPS`) — the set the
// `hello` frame must advertise so `needs[]` gating works (18-extensions EXT.020).
const EXPECTED_CAPS = [
  "command", "query", "openFile", "typeText", "termSend", "writeFile",
  "saveAll", "closeEditor", "fileContent", "terminalText", "diagnostics",
  "openEditors", "setting", "extensions", "editorText", "selection",
];

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ─────────────────────────── 16-views-panels ──────────────────────────────

  {
    id: "view.paletteRepeat",
    specId: "L1.VIEW.002",
    title: "View: showCommands twice in a row is idempotent (no throw, no stacked editors)",
    tags: ["views", "palette"],
    needs: ["command", "query"],
    rationale: `
WHAT: Runs 'workbench.action.showCommands' TWICE in a row via env.act and asserts both
calls resolve ok AND that openTabs length is unchanged across the pair (the palette opens
no editor and re-issuing it on an already-open overlay must not stack tabs).

WHY THIS OUTCOME: showCommands opens the command-palette quick-input overlay — a transient
widget the headless Snapshot can't see. Re-issuing it while it is already open must simply
re-focus/re-open the same quick-input, never throwing and never opening a second editor. So
the faithful observable is "both calls resolved (env.act throws on !ok)" plus the invariant
that openTabs did not drift (the overlay is editor-neutral). Asserting on the palette widget
itself would be flaky, so we deliberately don't.

WHY IT MATTERS: EDGE (repeat) — a common refactor break is double-dispatch on an
already-open overlay raising or stacking state. This pins that re-issuing a focus/overlay
command is safe and editor-neutral; a thrown env.act or an openTabs delta would expose the
regression. needs:[command,query] → SKIP cleanly without the caps.`,
    async run(env) {
      const before = await env.observe("view.paletteRepeat.before");
      await env.act("workbench.action.showCommands");
      await sleep(500);
      await env.act("workbench.action.showCommands");
      await sleep(500);
      const after = await env.observe("view.paletteRepeat.after");
      const b = lenOf(before.vscode.openTabs);
      const a = lenOf(after.vscode.openTabs);
      const stable = b !== null && a !== null && a === b;
      return {
        pass: stable,
        detail: stable
          ? `showCommands x2 resolved; openTabs ${b} → ${a} (unchanged)`
          : `openTabs ${JSON.stringify(b)} → ${JSON.stringify(a)} (drifted or not exposed)`,
        evidence: { beforeTabs: before.vscode.openTabs, afterTabs: after.vscode.openTabs },
      };
    },
  },

  {
    id: "view.toggleSidebarRoundtrip",
    specId: "L1.VIEW.011",
    title: "View: Toggle Side Bar twice returns to the original state (editor-neutral)",
    tags: ["views"],
    needs: ["command", "query"],
    rationale: `
WHAT: Runs 'workbench.action.toggleSidebarVisibility' TWICE (hide then show) via env.act and
asserts both resolve ok AND that visibleEditors + openTabs are identical across the pair.

WHY THIS OUTCOME: A visibility toggle must be its own inverse — hide then show returns the
side bar to its original visible state — and it is a pure layout operation that opens/closes
no editor. So the editor surface (visibleEditors, openTabs) must be exactly as it started.
Side-bar visibility itself is NOT in the Snapshot (awaits a Track-D 'sideBarVisible' field),
so editor-invariance across the pair plus "both commands resolved" is the strongest honest
assertion.

WHY IT MATTERS: EDGE (repeat / round-trip) — guards a state-tracking regression where the
second toggle no-ops or errors, or where toggling the side bar perturbs editors. Builds on
the VIEW.010 dispatch smoke (view.toggleSidebar) by adding the round-trip invariant.
needs:[command,query] → SKIP cleanly.`,
    async run(env) {
      const before = await env.observe("view.toggleSidebarRoundtrip.before");
      await env.act("workbench.action.toggleSidebarVisibility");
      await sleep(500);
      await env.act("workbench.action.toggleSidebarVisibility");
      await sleep(500);
      const after = await env.observe("view.toggleSidebarRoundtrip.after");
      const ve = JSON.stringify(before.vscode.visibleEditors) === JSON.stringify(after.vscode.visibleEditors);
      const ot = lenOf(before.vscode.openTabs) === lenOf(after.vscode.openTabs);
      return {
        pass: ve && ot,
        detail: `toggleSidebarVisibility x2 resolved; visibleEditors ${ve ? "stable" : "DRIFTED"}, openTabs ${ot ? "stable" : "DRIFTED"}`,
        evidence: {
          beforeVisible: before.vscode.visibleEditors, afterVisible: after.vscode.visibleEditors,
          beforeTabs: before.vscode.openTabs, afterTabs: after.vscode.openTabs,
        },
      };
    },
  },

  {
    id: "view.panelKeepsTerminalIdentity",
    specId: "L1.VIEW.021",
    title: "View: Toggle Panel (hide then show) keeps the same terminal identity",
    tags: ["views", "panel"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: In a fresh env, creates exactly one terminal, records terminalCount and the terminals
name array, runs 'workbench.action.togglePanel' to hide then again to show, and asserts the
terminals array (names) is identical before-hide and after-show AND terminalCount stays 1.

WHY THIS OUTCOME: Hiding the panel that hosts the integrated terminal must NOT dispose or
recreate the terminal — only the UI is hidden, the shell process and its identity survive.
So both the count (1) and the named-terminal identity must be unchanged across a hide/show.
This is stronger than VIEW.020 (which only guards the count): it pins that the SAME terminal
(by name) survives, guarding against a silent recreate that preserves the count but swaps the
process.

WHY IT MATTERS: EDGE (lifecycle under repeat) — guards that panel visibility and terminal
lifecycle are decoupled at the identity level, not just the count level. A break where
hide/show silently recreated the terminal would change its name while keeping count==1; this
catches exactly that. If the snapshot doesn't expose the terminals name array we fall back to
the count invariant. needs:[command,query] → SKIP cleanly.`,
    async run(env) {
      // Create exactly one terminal.
      await env.act("workbench.action.terminal.new");
      await sleep(1200);
      const before = await env.observe("view.panelKeepsTerminalIdentity.before");
      const bNames = termNames(before.vscode);
      const bCount = before.vscode.terminalCount;
      // Hide then show the panel.
      await env.act("workbench.action.togglePanel");
      await sleep(600);
      await env.act("workbench.action.togglePanel");
      await sleep(600);
      const after = await env.observe("view.panelKeepsTerminalIdentity.after");
      const aNames = termNames(after.vscode);
      const aCount = after.vscode.terminalCount;
      const countOk = bCount === 1 && aCount === 1;
      // Identity check when names are exposed; otherwise the count invariant carries it.
      const identityOk = bNames !== null && aNames !== null ? bNames === aNames : true;
      return {
        pass: countOk && identityOk,
        detail: `terminalCount ${bCount} → ${aCount} (want 1→1); terminal identity ${identityOk ? "preserved" : "CHANGED"}`,
        evidence: {
          beforeTerminals: before.vscode.terminals, afterTerminals: after.vscode.terminals,
          beforeCount: bCount, afterCount: aCount,
        },
      };
    },
  },

  {
    id: "view.showExplorer",
    specId: "L1.VIEW.040",
    title: "View: Show Explorer view resolves (editor-neutral side-bar switch)",
    tags: ["views"],
    needs: ["command", "query"],
    rationale: `
WHAT: Runs 'workbench.view.explorer' via env.act and asserts it resolves ok AND that
visibleEditors is unchanged (switching the active side-bar view opens no editor).

WHY THIS OUTCOME: workbench.view.explorer makes the Explorer (file-tree) the active side-bar
view — a side-bar view-switch, not an editor operation, so visibleEditors must be stable.
Which side-bar view is focused is NOT in the Snapshot (awaits a Track-D 'focusedView' field),
so "command resolved + editor-neutral" is the faithful observable.

WHY IT MATTERS: Covers the View-menu Explorer id from mux.rs; guards the side-bar view-switch
dispatch. A thrown env.act localises the break to command/view-switch wiring, not the
Explorer feature. needs:[command,query] → SKIP cleanly.`,
    async run(env) {
      const before = await env.observe("view.showExplorer.before");
      await env.act("workbench.view.explorer");
      await sleep(500);
      const after = await env.observe("view.showExplorer.after");
      const ve = JSON.stringify(before.vscode.visibleEditors) === JSON.stringify(after.vscode.visibleEditors);
      return {
        pass: ve,
        detail: `workbench.view.explorer resolved; visibleEditors ${ve ? "unchanged" : "DRIFTED"}`,
        evidence: { before: before.vscode.visibleEditors, after: after.vscode.visibleEditors },
      };
    },
  },

  {
    id: "view.showSearch",
    specId: "L1.VIEW.041",
    title: "View: Show Search view resolves (editor-neutral side-bar switch)",
    tags: ["views", "search"],
    needs: ["command", "query"],
    rationale: `
WHAT: Runs 'workbench.view.search' via env.act and asserts it resolves ok AND that
visibleEditors is unchanged.

WHY THIS OUTCOME: workbench.view.search focuses the Search side-bar view — a view-switch with
no editor lifecycle effect, so visibleEditors is the invariant. The focused-view id is not in
the Snapshot (awaits Track-D 'focusedView'), so "command resolved + editor-neutral" is the
faithful observable. Pairs with the 14-search area's find-in-files.

WHY IT MATTERS: Covers the Search side-bar view id; guards its view-switch dispatch. A thrown
env.act points at command/view-switch wiring. needs:[command,query] → SKIP cleanly.`,
    async run(env) {
      const before = await env.observe("view.showSearch.before");
      await env.act("workbench.view.search");
      await sleep(500);
      const after = await env.observe("view.showSearch.after");
      const ve = JSON.stringify(before.vscode.visibleEditors) === JSON.stringify(after.vscode.visibleEditors);
      return {
        pass: ve,
        detail: `workbench.view.search resolved; visibleEditors ${ve ? "unchanged" : "DRIFTED"}`,
        evidence: { before: before.vscode.visibleEditors, after: after.vscode.visibleEditors },
      };
    },
  },

  {
    id: "view.showScm",
    specId: "L1.VIEW.042",
    title: "View: Show Source Control view resolves regardless of git state",
    tags: ["views", "scm"],
    needs: ["command", "query"],
    rationale: `
WHAT: Runs 'workbench.view.scm' via env.act and asserts it resolves ok AND that visibleEditors
is unchanged.

WHY THIS OUTCOME: workbench.view.scm focuses the Source Control side-bar view. It must resolve
whether or not the workspace is a git repo — in a non-git workspace the view simply shows "no
repo" rather than erroring (that no-folder/no-repo robustness is the EDGE in VIEW.043). The
focused-view id isn't in the Snapshot, so "command resolved + editor-neutral" is the faithful
observable.

WHY IT MATTERS: Covers the SCM side-bar id; guards that the view-switch resolves even with no
git repository present. A thrown env.act points at command/view-switch wiring.
needs:[command,query] → SKIP cleanly.`,
    async run(env) {
      const before = await env.observe("view.showScm.before");
      await env.act("workbench.view.scm");
      await sleep(500);
      const after = await env.observe("view.showScm.after");
      const ve = JSON.stringify(before.vscode.visibleEditors) === JSON.stringify(after.vscode.visibleEditors);
      return {
        pass: ve,
        detail: `workbench.view.scm resolved; visibleEditors ${ve ? "unchanged" : "DRIFTED"}`,
        evidence: { before: before.vscode.visibleEditors, after: after.vscode.visibleEditors },
      };
    },
  },

  {
    id: "view.showExtensions",
    specId: "L1.VIEW.043",
    title: "View: Show Extensions view resolves (workspace-independent)",
    tags: ["views", "extensions"],
    needs: ["command"],
    rationale: `
WHAT: Runs 'workbench.view.extensions' via env.act and asserts it resolves ok — the Extensions
side-bar view is workspace-independent and must focus even with no folder open.

WHY THIS OUTCOME: View-switch commands must not require a workspace folder; the Extensions
view lists installed extensions regardless of any open folder. The focused-view id isn't in
the Snapshot, so "command resolved" (env.act throws on !ok) is the faithful observable. This
is the no-folder EDGE counterpart to the SCM view's no-repo robustness.

WHY IT MATTERS: EDGE (missing precondition) — guards a regression where a no-folder boot makes
side-bar view ids throw. (Cross-ref 18-extensions EXT.040 ext.openExtensionsView, which covers
the same command from the extensions angle.) needs:[command] only.`,
    async run(env) {
      await env.act("workbench.view.extensions");
      await sleep(400);
      await env.observe("view.showExtensions.after");
      return { pass: true, detail: "workbench.view.extensions resolved (workspace-independent)" };
    },
  },

  {
    id: "view.toggleTerminalFromZero",
    specId: "L1.VIEW.050",
    title: "View: Toggle Terminal from zero terminals creates one (0→1)",
    tags: ["views", "terminal"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: In a fresh env with zero terminals (asserts the precondition terminalCount==0), runs
'workbench.action.terminal.toggleTerminal' and asserts terminalCount delta == +1 (the first
toggle from empty creates a terminal).

WHY THIS OUTCOME: Unlike toggleSidebar/togglePanel (pure chrome), toggleTerminal from ZERO
terminals has a snapshot-OBSERVABLE effect — it must spawn a terminal, so terminalCount goes
0→1. This is therefore a REAL assertion, not a resolve-only smoke. We require the precondition
(count==0) so the create-branch is genuinely exercised.

WHY IT MATTERS: EDGE (empty state) — the same command id behaves differently by precondition
(create-from-zero here vs hide-existing in VIEW.051); this pins the create branch. A break
where the first toggle doesn't spawn a terminal would leave the count at 0. needs:[command,
query] → SKIP cleanly.`,
    async run(env) {
      const before = await env.observe("view.toggleTerminalFromZero.before");
      const b = before.vscode.terminalCount;
      if (typeof b !== "number" || b !== 0) {
        return {
          pass: false,
          detail: `precondition not met: expected 0 terminals, found ${JSON.stringify(b)}`,
          evidence: { beforeCount: b },
        };
      }
      await env.act("workbench.action.terminal.toggleTerminal");
      await sleep(1500);
      const after = await env.observe("view.toggleTerminalFromZero.after");
      const a = after.vscode.terminalCount;
      const pass = a === 1;
      return {
        pass,
        detail: `terminalCount ${b} → ${a} (want 0→1; toggle-from-zero creates one)`,
        evidence: { beforeCount: b, afterCount: a },
      };
    },
  },

  {
    id: "view.toggleTerminalHidesOne",
    specId: "L1.VIEW.051",
    title: "View: Toggle Terminal with one open terminal hides it (count unchanged)",
    tags: ["views", "terminal"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: In a fresh env, creates exactly one terminal (asserts terminalCount==1), runs
'workbench.action.terminal.toggleTerminal', and asserts terminalCount is unchanged (==1) —
the toggle only hides the panel, it does not dispose the terminal.

WHY THIS OUTCOME: With a terminal already open, toggleTerminal hides the panel rather than
creating/destroying a terminal, so the surviving terminal keeps the count at 1. This is the
hide branch of the same command whose create branch VIEW.050 pins — the two contracts must not
be conflated.

WHY IT MATTERS: EDGE (non-empty state) — guards that the hide branch doesn't dispose the
terminal (count must stay 1). A break where toggling killed the terminal would drop the count
to 0; a break where it spawned another would push it to 2. needs:[command,query] → SKIP
cleanly.`,
    async run(env) {
      await env.act("workbench.action.terminal.new");
      await sleep(1200);
      const before = await env.observe("view.toggleTerminalHidesOne.before");
      const b = before.vscode.terminalCount;
      if (typeof b !== "number" || b !== 1) {
        return {
          pass: false,
          detail: `precondition not met: expected 1 terminal, found ${JSON.stringify(b)}`,
          evidence: { beforeCount: b },
        };
      }
      await env.act("workbench.action.terminal.toggleTerminal");
      await sleep(800);
      const after = await env.observe("view.toggleTerminalHidesOne.after");
      const a = after.vscode.terminalCount;
      const pass = a === 1;
      return {
        pass,
        detail: `terminalCount ${b} → ${a} (want 1→1; toggle only hides, does not dispose)`,
        evidence: { beforeCount: b, afterCount: a },
      };
    },
  },

  {
    id: "view.toggleOutput",
    specId: "L1.VIEW.060",
    title: "View: Toggle Output panel resolves (Output is not a terminal)",
    tags: ["views", "panel"],
    needs: ["command", "query"],
    rationale: `
WHAT: Runs 'workbench.action.output.toggleOutput' via env.act and asserts it resolves ok AND
that terminalCount is unchanged (the Output panel is not a terminal).

WHY THIS OUTCOME: The Output panel shows extension/log channels — it is a distinct panel view,
not a terminal — so toggling it must not change terminalCount. Output-panel visibility itself
is NOT in the Snapshot (awaits a Track-D field), so "command resolved + terminalCount
invariant" is the faithful observable.

WHY IT MATTERS: Covers the View-menu Output id; guards that the Output surface is reachable and
that it is not conflated with the terminal count. A thrown env.act points at command wiring; a
terminalCount drift would mean Output and terminal lifecycle got tangled.
needs:[command,query] → SKIP cleanly.`,
    async run(env) {
      const before = await env.observe("view.toggleOutput.before");
      await env.act("workbench.action.output.toggleOutput");
      await sleep(500);
      const after = await env.observe("view.toggleOutput.after");
      const tc = before.vscode.terminalCount === after.vscode.terminalCount;
      return {
        pass: tc,
        detail: `output.toggleOutput resolved; terminalCount ${before.vscode.terminalCount} → ${after.vscode.terminalCount} (${tc ? "unchanged" : "DRIFTED"})`,
        evidence: { beforeCount: before.vscode.terminalCount, afterCount: after.vscode.terminalCount },
      };
    },
  },

  {
    id: "view.quickOpenCommand",
    specId: "L1.VIEW.070",
    title: "View: Go to File (quickOpen) command resolves without navigating",
    tags: ["views", "quickopen"],
    needs: ["command", "query"],
    rationale: `
WHAT: Runs 'workbench.action.quickOpen' via env.act and asserts it resolves ok AND that
activeEditor is unchanged (opening the picker selects no file, so nothing navigates yet).

WHY THIS OUTCOME: quickOpen opens the Go-to-File quick-input overlay; until the user types and
accepts a pick, no document opens, so activeEditor must stay put. The picker contents are not
snapshot-observable, so the faithful observable is "command resolved + no navigation". The
PICK outcome (a named file becoming active) is deliberately covered separately via the bridge
openFile action (VIEW.071 quickOpen.byName in files.mjs) because driving the quick-input widget
by synthetic keystrokes is untrusted headlessly — this entry is the raw command-dispatch smoke.

WHY IT MATTERS: Covers the Go-menu quickOpen id. A regression where quickOpen eagerly
navigates, or throws, trips this. Splitting the widget-open guard (here) from the pick outcome
(VIEW.071) lets a reader bisect command vs openFile/resolution faults. needs:[command,query] →
SKIP cleanly.`,
    async run(env) {
      const before = await env.observe("view.quickOpenCommand.before");
      await env.act("workbench.action.quickOpen");
      await sleep(600);
      const after = await env.observe("view.quickOpenCommand.after");
      const stayed = (before.vscode.activeEditor ?? null) === (after.vscode.activeEditor ?? null);
      return {
        pass: stayed,
        detail: stayed
          ? "quickOpen resolved; activeEditor unchanged (no pick yet)"
          : `activeEditor changed: ${JSON.stringify(before.vscode.activeEditor)} → ${JSON.stringify(after.vscode.activeEditor)}`,
        evidence: { activeBefore: before.vscode.activeEditor, activeAfter: after.vscode.activeEditor },
      };
    },
  },

  {
    id: "view.gotoLineWithEditor",
    specId: "L1.VIEW.072",
    title: "View: Go to Line/Column command resolves with an editor open",
    tags: ["views"],
    isolation: "fresh",
    needs: ["command", "openFile"],
    rationale: `
WHAT: Seeds and opens a multi-line file as the active editor, runs
'workbench.action.gotoLine' via env.act, and asserts it resolves ok AND that activeEditor is
unchanged (opening the line picker navigates within the same editor, not to a different one).

WHY THIS OUTCOME: gotoLine opens the line/column quick-input for the active editor — an overlay
the Snapshot can't see, and it does not switch which editor is active. So "command resolved +
activeEditor invariant" is the faithful observable. A cursor-line assertion would need the
'selection' Snapshot field set after a controlled goto (a Track-D upgrade).

WHY IT MATTERS: Covers the Go-menu gotoLine id with an editor present (the happy path), paired
with the no-editor EDGE in VIEW.073. A thrown env.act points at command wiring.
needs:[command,openFile] → SKIP cleanly.`,
    async run(env) {
      const path = `${PROJECT}/fleet-gotoline.txt`;
      env.exec(`printf 'one\\ntwo\\nthree\\nfour\\nfive\\n' > ${path}`);
      await env.request({ type: "openFile", path });
      await sleep(800);
      const before = await env.observe("view.gotoLineWithEditor.before");
      await env.act("workbench.action.gotoLine");
      await sleep(500);
      const after = await env.observe("view.gotoLineWithEditor.after");
      const stayed = (before.vscode.activeEditor ?? null) === (after.vscode.activeEditor ?? null);
      return {
        pass: stayed,
        detail: stayed
          ? "gotoLine resolved with an editor open; activeEditor unchanged"
          : `activeEditor changed: ${JSON.stringify(before.vscode.activeEditor)} → ${JSON.stringify(after.vscode.activeEditor)}`,
        evidence: { activeBefore: before.vscode.activeEditor, activeAfter: after.vscode.activeEditor },
      };
    },
  },

  {
    id: "view.gotoLineNoEditor",
    specId: "L1.VIEW.073",
    title: "View: gotoLine with NO active editor resolves cleanly (no-op)",
    tags: ["views"],
    isolation: "fresh",
    needs: ["command"],
    rationale: `
WHAT: In a fresh env with all editors closed (no active editor), runs
'workbench.action.gotoLine' via env.act and asserts it resolves ok (does not throw).

WHY THIS OUTCOME: An editor-scoped Go command with no editor has nothing to navigate; the
correct behaviour is to no-op gracefully (the picker simply has no target), NOT to throw. So
"env.act does not throw" (ok:true) is the exact, faithful assertion.

WHY IT MATTERS: EDGE (missing precondition) — guards a reject-instead-of-noop regression where
gotoLine rejects without an editor, which would surface as a spurious !ok for any caller
issuing it before opening a file. Mirrors settings.toggleWordWrapNoEditor (SET.021).
needs:[command] only.`,
    async run(env) {
      for (let i = 0; i < 3; i++) {
        await env.act("workbench.action.closeAllEditors").catch(() => {});
        await sleep(300);
      }
      await env.act("workbench.action.gotoLine");
      await sleep(400);
      await env.observe("view.gotoLineNoEditor.after");
      return { pass: true, detail: "gotoLine with no active editor resolved (no-op, did not throw)" };
    },
  },

  {
    id: "view.newUntitledFile",
    specId: "L1.VIEW.090",
    title: "File: New Untitled File adds a tab",
    tags: ["views", "editor"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: Records openTabs length N, runs 'workbench.action.files.newUntitledFile' via
env.act, and asserts openTabs length delta == +1 (a new untitled editor opened).

WHY THIS OUTCOME: New Untitled File opens a fresh untitled editor that becomes a real
tab, so openTabs must grow by exactly one. We assert a DELTA rather than an absolute
count because the fresh-window tab count is env-dependent (restored editors,
extensions). activeEditor is intentionally NOT asserted because untitled docs have no
fsPath (the snapshot's activeEditor would be null), so openTabs delta is the stable
observable.

WHY IT MATTERS: Covers the File-menu New Text File id with a snapshot-OBSERVABLE
effect — a real assertion, the open half of the open/close round-trip (paired with
VIEW.091). A break where the count doesn't grow means the command stopped materialising
an editor or openTabs stopped tracking untitled docs.`,
    async run(env) {
      const before = await env.observe("view.newUntitledFile.before");
      const b = lenOf(before.vscode.openTabs);
      await env.act("workbench.action.files.newUntitledFile");
      await sleep(800);
      const after = await env.observe("view.newUntitledFile.after");
      const a = lenOf(after.vscode.openTabs);
      const measurable = b !== null && a !== null;
      return {
        pass: measurable && a === b + 1,
        detail: measurable
          ? `openTabs ${b} → ${a} (delta ${a - b}, want +1)`
          : "openTabs not exposed by snapshot — cannot assert",
        evidence: { beforeTabs: before.vscode.openTabs, afterTabs: after.vscode.openTabs },
      };
    },
  },

  {
    id: "view.closeAfterUntitled",
    specId: "L1.VIEW.091",
    title: "File: Close active editor after New Untitled shrinks the tab count",
    tags: ["views", "editor"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: Opens a new untitled editor ('workbench.action.files.newUntitledFile'), records
the post-create openTabs length, then closes the active editor via the bridge
'closeEditor' action and asserts openTabs length delta == -1 vs the post-create count.

WHY THIS OUTCOME: closeActiveEditor closes exactly the focused (just-created untitled)
tab, so the open-tab set must shrink by one relative to the post-create count. We chain
create→close in one behaviour so the count delta is self-contained and deterministic
('fresh' isolation), proving the open/close tab-count round-trip end to end.

WHY IT MATTERS: Pairs with VIEW.090 to prove the full open/close round-trip and guards
the closeEditor action wiring specifically (which the palette/panel tests don't touch).
A break where the count doesn't drop means closeEditor stopped closing the active tab
or openTabs went stale on close.`,
    async run(env) {
      await env.act("workbench.action.files.newUntitledFile");
      await sleep(800);
      const created = await env.observe("view.closeAfterUntitled.created");
      const c = lenOf(created.vscode.openTabs);
      if (env.supports("closeEditor")) {
        await env.request({ type: "closeEditor" });
      } else {
        await env.act("workbench.action.closeActiveEditor");
      }
      await sleep(800);
      const after = await env.observe("view.closeAfterUntitled.after");
      const a = lenOf(after.vscode.openTabs);
      const measurable = c !== null && a !== null;
      return {
        pass: measurable && a === c - 1,
        detail: measurable
          ? `openTabs ${c} → ${a} (delta ${a - c}, want -1)`
          : "openTabs not exposed by snapshot — cannot assert",
        evidence: { createdTabs: created.vscode.openTabs, afterTabs: after.vscode.openTabs },
      };
    },
  },

  {
    id: "view.closeEditorNoEditor",
    specId: "L1.VIEW.092",
    title: "File: closeEditor with no editor open is a clean no-op",
    tags: ["views", "editor"],
    isolation: "fresh",
    needs: ["closeEditor", "query"],
    rationale: `
WHAT: In a fresh env, closes all editors so the editor area is empty, records openTabs
length, then issues the bridge 'closeEditor' action and asserts it returns ok:true AND
that openTabs length is unchanged (nothing to close).

WHY THIS OUTCOME: closeActiveEditor on an empty editor area has nothing to close, so it
must resolve ok and leave the tab set unchanged — NOT throw. env.request returns ok:true
(it throws only on ok:false), so a successful return is the assertion; openTabs stability
confirms it was a true no-op.

WHY IT MATTERS: EDGE (empty state) — guards a regression where the bridge's closeEditor
wrapper rejects on no-active-editor (it wraps closeActiveEditor and should tolerate the
empty case). A throw here would mean the wrapper lost its no-active-editor tolerance.`,
    async run(env) {
      for (let i = 0; i < 3; i++) {
        await env.act("workbench.action.closeAllEditors").catch(() => {});
        await sleep(300);
      }
      const before = await env.observe("view.closeEditorNoEditor.before");
      const b = lenOf(before.vscode.openTabs);
      let ok = false;
      try {
        const r = await env.request({ type: "closeEditor" });
        ok = !r || r.ok !== false; // env.request throws on ok:false; reaching here is ok
      } catch {
        ok = false;
      }
      await sleep(400);
      const after = await env.observe("view.closeEditorNoEditor.after");
      const a = lenOf(after.vscode.openTabs);
      const unchanged = b !== null && a !== null && a === b;
      return {
        pass: ok && unchanged,
        detail: ok
          ? `closeEditor on empty editor area resolved ok; openTabs ${b} → ${a} (${unchanged ? "unchanged" : "DRIFTED"})`
          : "closeEditor threw on an empty editor area (expected a clean no-op)",
        evidence: { beforeTabs: before.vscode.openTabs, afterTabs: after.vscode.openTabs },
      };
    },
  },

  {
    id: "view.toggleZen",
    specId: "L1.VIEW.100",
    title: "View: Toggle Zen Mode resolves (editors untouched)",
    tags: ["views"],
    needs: ["command", "query"],
    isolation: "fresh",
    rationale: `
WHAT: Runs 'workbench.action.toggleZenMode' via env.act and asserts it resolves ok and
that visibleEditors + activeEditor are unchanged across the call. (Fresh isolation so the
env isn't left in zen for siblings.)

WHY THIS OUTCOME: Zen mode hides workbench CHROME (side bar, panel, status bar) to focus
a single editor — it does not open or close editors. So visibleEditors/activeEditor must
be stable. "In zen" itself isn't in the Snapshot (a Track-D 'zenMode' field is needed),
so dispatch + editor-neutrality is the faithful observable.

WHY IT MATTERS: Covers zen layout dispatch; guards that entering zen never perturbs the
editor surface. We immediately toggle back out so the env's layout isn't left in zen.`,
    async run(env) {
      const before = await env.observe("view.toggleZen.before");
      await env.act("workbench.action.toggleZenMode");
      await sleep(700);
      const after = await env.observe("view.toggleZen.after");
      // Restore normal layout for cleanliness.
      await env.act("workbench.action.toggleZenMode").catch(() => {});
      await sleep(500);
      const ve = JSON.stringify(before.vscode.visibleEditors) === JSON.stringify(after.vscode.visibleEditors);
      const ae = before.vscode.activeEditor === after.vscode.activeEditor;
      return {
        pass: ve && ae,
        detail: `toggleZenMode resolved; visibleEditors ${ve ? "stable" : "DRIFTED"}, activeEditor ${ae ? "stable" : "DRIFTED"}`,
        evidence: {
          beforeVisible: before.vscode.visibleEditors, afterVisible: after.vscode.visibleEditors,
          beforeActive: before.vscode.activeEditor, afterActive: after.vscode.activeEditor,
        },
      };
    },
  },

  {
    id: "view.zenRoundtrip",
    specId: "L1.VIEW.101",
    title: "View: Toggle Zen Mode twice restores normal layout (editors untouched)",
    tags: ["views"],
    needs: ["command", "query"],
    isolation: "fresh",
    rationale: `
WHAT: Runs 'workbench.action.toggleZenMode' TWICE (enter then exit) via env.act and
asserts both resolve ok AND that visibleEditors + openTabs are identical across the pair.

WHY THIS OUTCOME: Zen toggle must be self-inverse — enter then exit returns to the normal
layout — and zen never drops editors, so the editor surface must be exactly as it
started. Zen state isn't in the Snapshot (awaits a Track-D 'zenMode' field), so editor
invariance across the enter/exit pair is the strongest honest assertion.

WHY IT MATTERS: EDGE (round-trip) — guards a layout-state regression where the second
toggle no-ops/errors or where zen drops an editor. 'fresh' isolation keeps the editor
baseline clean.`,
    async run(env) {
      const before = await env.observe("view.zenRoundtrip.before");
      await env.act("workbench.action.toggleZenMode");
      await sleep(700);
      await env.act("workbench.action.toggleZenMode");
      await sleep(700);
      const after = await env.observe("view.zenRoundtrip.after");
      const ve = JSON.stringify(before.vscode.visibleEditors) === JSON.stringify(after.vscode.visibleEditors);
      const ot = lenOf(before.vscode.openTabs) === lenOf(after.vscode.openTabs);
      return {
        pass: ve && ot,
        detail: `toggleZenMode x2 resolved; visibleEditors ${ve ? "stable" : "DRIFTED"}, openTabs ${ot ? "stable" : "DRIFTED"}`,
        evidence: {
          beforeVisible: before.vscode.visibleEditors, afterVisible: after.vscode.visibleEditors,
          beforeTabs: before.vscode.openTabs, afterTabs: after.vscode.openTabs,
        },
      };
    },
  },

  {
    id: "view.resetLayout",
    specId: "L1.VIEW.110",
    title: "View: Reset View Locations resolves from a mutated layout",
    tags: ["views"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: First MUTATES the layout (hide side bar + hide panel via their toggle commands),
then runs 'workbench.action.resetViewLocations' via env.act and asserts it resolves ok
and that visibleEditors is unchanged (a layout reset opens no editor).

WHY THIS OUTCOME: A reset command must run from an already-mutated layout without error —
that is the recovery path it exists for. It restores default view locations but opens no
editor, so visibleEditors is the invariant. Whether the default was ACTUALLY restored
isn't in the Snapshot (awaits Track-D chrome fields), so dispatch-from-mutated +
editor-neutrality is the faithful observable.

WHY IT MATTERS: EDGE (recovery) — documents and guards the recovery path: resetting from
a non-default layout must not throw. A break here means the reset command regressed or
started requiring a default starting layout.`,
    async run(env) {
      // Mutate the layout first so the reset runs from a non-default state.
      await env.act("workbench.action.toggleSidebarVisibility").catch(() => {});
      await sleep(300);
      await env.act("workbench.action.togglePanel").catch(() => {});
      await sleep(300);
      const before = await env.observe("view.resetLayout.before");
      await env.act("workbench.action.resetViewLocations");
      await sleep(600);
      const after = await env.observe("view.resetLayout.after");
      const ve = JSON.stringify(before.vscode.visibleEditors) === JSON.stringify(after.vscode.visibleEditors);
      return {
        pass: ve,
        detail: `resetViewLocations resolved from a mutated layout; visibleEditors ${ve ? "unchanged" : "DRIFTED"}`,
        evidence: { before: before.vscode.visibleEditors, after: after.vscode.visibleEditors },
      };
    },
  },

  {
    id: "view.unknownCommandFails",
    specId: "L1.VIEW.120",
    title: "View: an unknown command id fails cleanly (ok:false, no hang)",
    tags: ["views"],
    needs: ["command"],
    rationale: `
WHAT: Issues 'workbench.action.doesNotExist.fleet' (an unregistered command id) through
the bridge and asserts the round-trip produces a fast ok:false with a non-empty error —
NOT a silent drop or a hang. env.request throws "request failed: <error>" on ok:false, so
we assert the throw carries a non-empty error within a bounded time.

WHY THIS OUTCOME: VS Code rejects executeCommand for an unregistered id; the bridge funnels
that rejection to a {ok:false, error} reply (the handle()→fail() path in extension.ts). So
the EXPECTED outcome of an unknown id is a prompt failure reply, observed here as env.request
throwing with a message — distinct from a timeout (which would mean the bridge swallowed the
frame).

WHY IT MATTERS: EDGE (failure mode) — proves the bridge's error path is alive and BOUNDED:
an unknown id must produce a quick ok:false, not a silent drop or a hang that would stall the
whole suite. Guards the command handler's reject→fail wiring. We measure elapsed time to
distinguish a real failure reply from a timeout.`,
    async run(env) {
      const t0 = Date.now();
      let failedOk = false;
      let msg = "";
      try {
        await env.request({ type: "command", id: "workbench.action.doesNotExist.fleet" });
        // Reaching here means ok:true for a non-existent id — that's WRONG.
        failedOk = false;
        msg = "command unexpectedly reported ok:true for an unregistered id";
      } catch (e) {
        const s = String(e && e.message ? e.message : e);
        // env.request throws "request failed: <error>" on ok:false. A timeout throws
        // "bridge req timeout" — that is NOT the clean-failure we want.
        const isTimeout = /timeout/i.test(s);
        failedOk = !isTimeout && /request failed/i.test(s) && s.replace(/request failed:?/i, "").trim().length > 0;
        msg = s;
      }
      const elapsed = Date.now() - t0;
      return {
        pass: failedOk,
        detail: failedOk
          ? `unknown command id failed cleanly (ok:false) in ${elapsed}ms: ${msg}`
          : `unknown command id did NOT fail cleanly (${msg})`,
        evidence: { elapsedMs: elapsed, error: msg },
      };
    },
  },

  // ───────────────────────────── 17-settings ────────────────────────────────

  {
    id: "settings.readMinimapDefault",
    specId: "L1.SET.001",
    title: "Settings: read editor.minimap.enabled default round-trips to a boolean",
    tags: ["settings"],
    needs: ["setting"],
    rationale: `
WHAT: Issues the bridge 'setting {key:"editor.minimap.enabled"}' query and asserts the
reply is ok AND that settingValue() resolves to a BOOLEAN (true on a stock image).

WHY THIS OUTCOME: editor.minimap.enabled is a real default config key; getConfiguration
(section).get(leaf) must resolve it to its live boolean value (default true). A boolean
(not undefined) proves the section/leaf split in extension.ts is correct and the read path
works — the prerequisite for every toggle assertion. settingValue() tolerates both the
top-level (r.value) and nested (r.data.value) result shapes per §3.3.

WHY IT MATTERS: Smoke for the 'setting' read path. An undefined here means the
section/leaf split or the config read regressed; this is the cheapest, earliest tripwire
for the read half of every settings round-trip. needs:[setting] → SKIP cleanly without
the cap.`,
    async run(env) {
      const r = await env.request({ type: "setting", key: "editor.minimap.enabled" });
      const v = settingValue(r);
      const pass = typeof v === "boolean";
      return {
        pass,
        detail: pass
          ? `editor.minimap.enabled read back as boolean ${JSON.stringify(v)}`
          : `editor.minimap.enabled read back as ${JSON.stringify(v)} (expected a boolean)`,
        evidence: { value: v },
      };
    },
  },

  {
    id: "settings.readUnknownKey",
    specId: "L1.SET.002",
    title: "Settings: an unknown setting key reads back undefined (not an error)",
    tags: ["settings"],
    needs: ["setting"],
    rationale: `
WHAT: Issues 'setting {key:"fleet.nonexistent.setting"}' and asserts the reply is ok AND
that settingValue() is undefined.

WHY THIS OUTCOME: VS Code's config.get returns undefined for an unknown key — it does NOT
throw — so the bridge must reply ok:true with value undefined. This distinguishes "key
absent (undefined)" from "query failed (ok:false)", which is essential so an undefined in a
toggle test means "not set" rather than "query broken".

WHY IT MATTERS: EDGE (missing key) — pins the contract that the read path never throws on
an unknown key. A regression where unknown keys raised would make every toggle test's
undefined ambiguous. needs:[setting] → SKIP cleanly without the cap.`,
    async run(env) {
      const r = await env.request({ type: "setting", key: "fleet.nonexistent.setting" });
      const v = settingValue(r);
      const okReply = !r || r.ok !== false; // env.request throws on ok:false
      const pass = okReply && v === undefined;
      return {
        pass,
        detail: pass
          ? "unknown key read back as undefined with ok:true"
          : `unknown key read back as ${JSON.stringify(v)} (okReply=${okReply})`,
        evidence: { value: v },
      };
    },
  },

  {
    id: "settings.readEmptyKeyFails",
    specId: "L1.SET.003",
    title: "Settings: reading an empty key fails cleanly (ok:false, requires key)",
    tags: ["settings"],
    needs: ["setting"],
    rationale: `
WHAT: Issues 'setting {key:""}' and asserts the round-trip produces ok:false with an error
containing "requires key". env.request throws "request failed: <error>" on ok:false, so we
catch the throw and assert its message includes the guard text.

WHY THIS OUTCOME: extension.ts explicitly guards the empty key (if (!key) throw new
Error("setting requires key")), which the handle()→fail() path turns into ok:false +
error. So the expected outcome of an empty key is a clean failure reply carrying that
exact message — not a silent getConfiguration("").get("") surprise.

WHY IT MATTERS: EDGE (bad input) — pins the empty-key guard so a future caller gets a clear
failure rather than reading a garbage/default value. A break where empty keys returned ok
(some default) would silently mask bad query construction. needs:[setting] → SKIP cleanly.`,
    async run(env) {
      let sawGuard = false;
      let msg = "";
      try {
        const r = await env.request({ type: "setting", key: "" });
        // If we get here without throwing, the empty-key guard didn't fire → WRONG.
        sawGuard = false;
        msg = `empty key unexpectedly returned ok (value=${JSON.stringify(settingValue(r))})`;
      } catch (e) {
        msg = String(e && e.message ? e.message : e);
        sawGuard = /requires key/i.test(msg);
      }
      return {
        pass: sawGuard,
        detail: sawGuard ? `empty key failed cleanly: ${msg}` : `empty key did NOT fail with "requires key": ${msg}`,
        evidence: { error: msg },
      };
    },
  },

  {
    id: "settings.minimapRoundtrip",
    specId: "L1.SET.011",
    title: "Settings: Toggle Minimap twice returns editor.minimap.enabled to baseline",
    tags: ["settings"],
    needs: ["setting"],
    rationale: `
WHAT: Reads editor.minimap.enabled baseline (B), runs 'editor.action.toggleMinimap' TWICE,
re-reads the value, and asserts it equals B (the net config value is back to baseline).

WHY THIS OUTCOME: editor.minimap.enabled is a config-backed boolean the 'setting' query
reads back (THE LESSON — chosen over per-editor word-wrap). A boolean toggle is its own
inverse, so two toggles must net to the baseline. This builds directly on the SET.010
write/read loop (settings.toggleMinimap), extending it to the round-trip invariant.

WHY IT MATTERS: EDGE (round-trip) — guards a regression where the toggle latches or drifts
(e.g. writes a different scope on the second call). A break with SET.010 green isolates the
fault to the inverse/second-toggle. needs:[setting] → SKIP cleanly without the cap.`,
    async run(env) {
      const before = await readSetting(env, "editor.minimap.enabled");
      await env.act("editor.action.toggleMinimap");
      await sleep(500);
      await env.act("editor.action.toggleMinimap");
      await sleep(500);
      const after = await readSetting(env, "editor.minimap.enabled");
      return {
        pass: typeof before === "boolean" && after === before,
        detail: `editor.minimap.enabled ${JSON.stringify(before)} → (toggle x2) → ${JSON.stringify(after)} (${after === before ? "round-trip ok" : "DRIFTED"})`,
        evidence: { before, after },
      };
    },
  },

  {
    id: "settings.wordWrapNotConfigBacked",
    specId: "L1.SET.020",
    title: "Settings: Toggle Word Wrap does NOT move editor.wordWrap config (per-editor override)",
    tags: ["settings"],
    isolation: "fresh",
    needs: ["setting", "openFile"],
    rationale: `
WHAT: Opens a seeded file, reads editor.wordWrap baseline via 'setting', runs
'editor.action.toggleWordWrap', re-reads editor.wordWrap, and asserts it is UNCHANGED vs
baseline. Pass == "config correctly did NOT move".

WHY THIS OUTCOME: THE LESSON encoded as a POSITIVE test. toggleWordWrap sets a transient
PER-EDITOR view override that config.get / the 'setting' query never reflect — so the
correct, faithful observable is that the CONFIGURATION value editor.wordWrap is unchanged.
A test that asserted a config flip here would always fail (the override never touches
config); asserting "config unchanged" documents exactly why word-wrap is unverifiable
through this path (the real wrap state would need a Track-D editor-view observable).

WHY IT MATTERS: Prevents a future contributor from "fixing" the minimap test to use
word-wrap and getting a vacuous pass — it pins, in a test, that word-wrap is a per-editor
override the 'setting' query cannot see. needs:[setting,openFile] → SKIP cleanly without
the caps.`,
    async run(env) {
      const path = `${PROJECT}/fleet-wordwrap.txt`;
      env.exec(`printf 'wrap me\\n' > ${path}`);
      await env.request({ type: "openFile", path });
      await sleep(800);
      const before = await readSetting(env, "editor.wordWrap");
      await env.act("editor.action.toggleWordWrap");
      await sleep(500);
      const after = await readSetting(env, "editor.wordWrap");
      const unchanged = JSON.stringify(before) === JSON.stringify(after);
      return {
        pass: unchanged,
        detail: unchanged
          ? `editor.wordWrap config correctly UNCHANGED (${JSON.stringify(before)}) — toggle is a per-editor override`
          : `editor.wordWrap config MOVED ${JSON.stringify(before)} → ${JSON.stringify(after)} (unexpected — should be a per-editor override)`,
        evidence: { before, after },
      };
    },
  },

  {
    id: "settings.toggleWordWrapNoEditor",
    specId: "L1.SET.021",
    title: "Settings: toggleWordWrap with NO active editor resolves cleanly",
    tags: ["settings"],
    isolation: "fresh",
    needs: ["command"],
    rationale: `
WHAT: In a fresh env with all editors closed (no active editor), runs
'editor.action.toggleWordWrap' via env.act and asserts it resolves ok (does not throw).

WHY THIS OUTCOME: An editor-scoped setting command with no editor has nothing to wrap; the
correct behaviour is to no-op gracefully, NOT to throw. So "env.act does not throw"
(ok:true) is the exact, faithful assertion.

WHY IT MATTERS: EDGE (missing precondition) — guards a reject-instead-of-noop regression
where the command rejects without an editor, which would surface as a spurious !ok for any
caller issuing it before opening a file.`,
    async run(env) {
      for (let i = 0; i < 3; i++) {
        await env.act("workbench.action.closeAllEditors").catch(() => {});
        await sleep(300);
      }
      await env.act("editor.action.toggleWordWrap");
      await sleep(400);
      await env.observe("settings.toggleWordWrapNoEditor.after");
      return { pass: true, detail: "toggleWordWrap with no active editor resolved (no-op, did not throw)" };
    },
  },

  {
    id: "settings.toggleAutoSave",
    specId: "L1.SET.030",
    title: "Settings: Toggle Auto Save flips files.autoSave (string enum, config-backed)",
    tags: ["settings"],
    needs: ["setting"],
    rationale: `
WHAT: Reads files.autoSave baseline (a string enum: "off"/"afterDelay"/…), runs
'workbench.action.toggleAutoSave', re-reads it, and asserts after !== before AND after is
defined.

WHY THIS OUTCOME: files.autoSave is a CONFIG-backed string-enum setting the 'setting' query
reads back; toggling it changes the value (e.g. "off" ↔ "afterDelay"). This proves the
read/write loop generalises beyond booleans to a string-valued setting — a different value
type through the same 'setting' path. The exact strings vary by VS Code default, so we
assert change (not a specific target).

WHY IT MATTERS: Covers the File-menu Auto Save id and exercises a string enum through the
config round-trip. A break where after===undefined means the read shape regressed; after===
before means the toggle stopped mutating config. needs:[setting] → SKIP cleanly.`,
    async run(env) {
      const before = await readSetting(env, "files.autoSave");
      await env.act("workbench.action.toggleAutoSave");
      await sleep(500);
      const after = await readSetting(env, "files.autoSave");
      const pass = before !== after && after !== undefined;
      // Restore baseline so siblings aren't perturbed (shared isolation).
      if (pass) { await env.act("workbench.action.toggleAutoSave").catch(() => {}); await sleep(300); }
      return {
        pass,
        detail: `files.autoSave ${JSON.stringify(before)} → ${JSON.stringify(after)}` + (pass ? "" : " (no observable change)"),
        evidence: { before, after },
      };
    },
  },

  {
    id: "settings.autoSaveRoundtrip",
    specId: "L1.SET.031",
    title: "Settings: Toggle Auto Save twice returns files.autoSave to baseline",
    tags: ["settings"],
    needs: ["setting"],
    rationale: `
WHAT: Reads files.autoSave baseline, runs 'workbench.action.toggleAutoSave' TWICE, re-reads
it, and asserts it equals the baseline string.

WHY THIS OUTCOME: files.autoSave is config-backed and the toggle cycles between two states,
so two toggles must net back to the baseline. This pairs with SET.030 to prove the string
enum cycles cleanly (rather than advancing through states unevenly).

WHY IT MATTERS: EDGE (round-trip on a string enum) — guards that the enum toggle is its own
inverse over two steps. A break with SET.030 green isolates the fault to the cycle's second
step. needs:[setting] → SKIP cleanly without the cap.`,
    async run(env) {
      const before = await readSetting(env, "files.autoSave");
      await env.act("workbench.action.toggleAutoSave");
      await sleep(500);
      await env.act("workbench.action.toggleAutoSave");
      await sleep(500);
      const after = await readSetting(env, "files.autoSave");
      return {
        pass: before !== undefined && JSON.stringify(after) === JSON.stringify(before),
        detail: `files.autoSave ${JSON.stringify(before)} → (toggle x2) → ${JSON.stringify(after)} (${JSON.stringify(after) === JSON.stringify(before) ? "round-trip ok" : "DRIFTED"})`,
        evidence: { before, after },
      };
    },
  },

  {
    id: "settings.writeSettingsJson",
    specId: "L1.SET.040",
    title: "Settings: write .vscode/settings.json then read editor.fontSize back via the bridge",
    tags: ["settings"],
    isolation: "fresh",
    needs: ["writeFile", "setting"],
    rationale: `
WHAT: Writes '{ "editor.fontSize": 17 }' to <root>/.vscode/settings.json via the bridge
'writeFile' action, then POLLS the 'setting {key:"editor.fontSize"}' query until it reports
17 (bounded). Asserts the workspace config picks up the written value.

WHY THIS OUTCOME: This proves the WRITE-via-disk → READ-via-bridge loop — the
workspace-settings path, distinct from a command-driven toggle. VS Code's config watcher
must notice the new settings.json and surface editor.fontSize as 17 to getConfiguration. We
poll because the watcher fires asynchronously after the file write. Naming the exact
key/value makes the read-back unambiguous.

WHY IT MATTERS: Guards that VS Code's workspace-settings watcher picks up a bridge-written
settings.json (a different mutation path than toggle commands). A break means either
writeFile didn't flush to disk or the watcher/getConfiguration scope resolution regressed.
needs:[writeFile,setting] → SKIP cleanly.`,
    async run(env) {
      const path = `${PROJECT}/.vscode/settings.json`;
      await env.request({ type: "writeFile", path, content: '{\n  "editor.fontSize": 17\n}\n' });
      let v;
      for (let i = 0; i < 20; i++) {
        await sleep(750);
        v = await readSetting(env, "editor.fontSize");
        if (v === 17) break;
      }
      return {
        pass: v === 17,
        detail: v === 17
          ? "editor.fontSize read back as 17 after writing .vscode/settings.json"
          : `editor.fontSize read back as ${JSON.stringify(v)} (expected 17 from the written settings.json)`,
        evidence: { value: v },
      };
    },
  },

  {
    id: "settings.malformedSettingsJson",
    specId: "L1.SET.041",
    title: "Settings: a malformed settings.json does not crash the config read",
    tags: ["settings"],
    isolation: "fresh",
    needs: ["writeFile", "setting"],
    rationale: `
WHAT: Writes invalid JSON ('{ "editor.fontSize": }') to <root>/.vscode/settings.json via
'writeFile', then issues 'setting {key:"editor.fontSize"}' and asserts the read still
resolves ok with a value that is a number OR undefined — i.e. NOT a thrown bridge error.

WHY THIS OUTCOME: VS Code ignores an unparseable settings file (it surfaces a Problems entry
but keeps the prior/default config), so the 'setting' read must still resolve — returning a
number (default/prior) or undefined — rather than crashing the ext-host or throwing on the
bridge. We assert "the read survived" (no throw, sane value type).

WHY IT MATTERS: EDGE (failure injection) — guards ext-host resilience: a corrupt settings
file must not take down the 'setting' query path. A thrown bridge error here would mean the
config read stopped tolerating an unparseable file. needs:[writeFile,setting] → SKIP
cleanly.`,
    async run(env) {
      const path = `${PROJECT}/.vscode/settings.json`;
      await env.request({ type: "writeFile", path, content: '{ "editor.fontSize": }\n' });
      await sleep(2000);
      let survived = false;
      let v;
      try {
        const r = await env.request({ type: "setting", key: "editor.fontSize" });
        v = settingValue(r);
        survived = (typeof v === "number" || v === undefined);
      } catch (e) {
        survived = false;
        v = `THREW: ${String(e && e.message ? e.message : e)}`;
      }
      return {
        pass: survived,
        detail: survived
          ? `config read survived a malformed settings.json (editor.fontSize=${JSON.stringify(v)})`
          : `config read did NOT survive a malformed settings.json (${JSON.stringify(v)})`,
        evidence: { value: v },
      };
    },
  },

  {
    id: "settings.workspaceOverride",
    specId: "L1.SET.050",
    title: "Settings: a workspace settings.json override wins over the default scope",
    tags: ["settings"],
    isolation: "fresh",
    needs: ["writeFile", "setting"],
    rationale: `
WHAT: Writes '{ "editor.tabSize": 2 }' to <root>/.vscode/settings.json via 'writeFile'
(default editor.tabSize is 4), then POLLS 'setting {key:"editor.tabSize"}' until it reports
2 (bounded). Asserts the EFFECTIVE value is the workspace override (2), not the default (4).

WHY THIS OUTCOME: getConfiguration resolves the EFFECTIVE value across scopes, with a
workspace setting winning over user/default. So after the workspace write, the read must
return 2 — proving the bridge reads the effective (workspace-winning) value, not just the
default scope. We poll because the config watcher applies the file asynchronously.

WHY IT MATTERS: Guards the documented scope-resolution behaviour of the 'setting' handler's
section/leaf split. A regression where the bridge read only the user/default scope would
leave this at 4 and silently break every workspace-scoped read. needs:[writeFile,setting] →
SKIP cleanly.`,
    async run(env) {
      const path = `${PROJECT}/.vscode/settings.json`;
      await env.request({ type: "writeFile", path, content: '{\n  "editor.tabSize": 2\n}\n' });
      let v;
      for (let i = 0; i < 20; i++) {
        await sleep(750);
        v = await readSetting(env, "editor.tabSize");
        if (v === 2) break;
      }
      return {
        pass: v === 2,
        detail: v === 2
          ? "editor.tabSize effective value is the workspace override 2 (not the default 4)"
          : `editor.tabSize read back as ${JSON.stringify(v)} (expected the workspace override 2)`,
        evidence: { value: v },
      };
    },
  },

  {
    id: "settings.toggleAndWriteAgree",
    specId: "L1.SET.060",
    title: "Settings: a minimap toggle and a settings.json write target the same key",
    tags: ["settings"],
    isolation: "fresh",
    needs: ["setting", "writeFile"],
    rationale: `
WHAT: Writes '{ "editor.minimap.enabled": false }' to .vscode/settings.json and POLLS
'setting' until it reads false; then runs 'editor.action.toggleMinimap' and asserts the
'setting' read flips to true (i.e. !the file-written false).

WHY THIS OUTCOME: The command toggle and the file write must target the SAME config key and
compose predictably: after a written baseline of false, one toggle flips it to true. If the
toggle wrote a different scope than the file, they would diverge and the post-toggle read
would not be the negation of the written baseline.

WHY IT MATTERS: EDGE (interaction) — guards against the toggle and the file write silently
targeting different scopes of editor.minimap.enabled. A break here with SET.010/SET.040
green means the two mutation paths stopped agreeing on a key/scope. needs:[setting,writeFile]
→ SKIP cleanly.`,
    async run(env) {
      const path = `${PROJECT}/.vscode/settings.json`;
      await env.request({ type: "writeFile", path, content: '{\n  "editor.minimap.enabled": false\n}\n' });
      let base;
      for (let i = 0; i < 20; i++) {
        await sleep(750);
        base = await readSetting(env, "editor.minimap.enabled");
        if (base === false) break;
      }
      if (base !== false) {
        return {
          pass: false,
          detail: `settings.json write of editor.minimap.enabled:false did not take (read ${JSON.stringify(base)})`,
          evidence: { base },
        };
      }
      await env.act("editor.action.toggleMinimap");
      await sleep(700);
      const after = await readSetting(env, "editor.minimap.enabled");
      return {
        pass: after === true,
        detail: `editor.minimap.enabled file=false → toggle → ${JSON.stringify(after)} (${after === true ? "flipped to !false as expected" : "did NOT flip as expected"})`,
        evidence: { base, after },
      };
    },
  },

  {
    id: "settings.readBeforeEditor",
    specId: "L1.SET.070",
    title: "Settings: reading a setting with zero editors open still resolves (config is global)",
    tags: ["settings"],
    isolation: "fresh",
    needs: ["setting"],
    rationale: `
WHAT: In a fresh env, closes all editors (zero open), then issues 'setting
{key:"editor.minimap.enabled"}' and asserts the reply is ok with a BOOLEAN value.

WHY THIS OUTCOME: getConfiguration is workspace/global, NOT editor-scoped, so a config read
must succeed with no editor open and return the boolean value. This distinguishes config
reads (always available) from editor-view state (which needs an editor) — complementing the
per-editor-override lesson in SET.020.

WHY IT MATTERS: EDGE (empty editor state) — guards a regression where the 'setting' read
started depending on an active editor (e.g. resolving against the active resource's scope
and failing when none exists). A break here means config reads became editor-coupled.
needs:[setting] → SKIP cleanly without the cap.`,
    async run(env) {
      for (let i = 0; i < 3; i++) {
        await env.act("workbench.action.closeAllEditors").catch(() => {});
        await sleep(300);
      }
      const r = await env.request({ type: "setting", key: "editor.minimap.enabled" });
      const v = settingValue(r);
      const pass = typeof v === "boolean";
      return {
        pass,
        detail: pass
          ? `setting read with zero editors open returned boolean ${JSON.stringify(v)}`
          : `setting read with zero editors returned ${JSON.stringify(v)} (expected a boolean)`,
        evidence: { value: v },
      };
    },
  },

  // ──────────────────────────── 18-extensions ───────────────────────────────

  {
    id: "ext.listReturns",
    specId: "L1.EXT.001",
    title: "Extensions: the extensions query returns a well-shaped installed list",
    tags: ["extensions"],
    needs: ["extensions"],
    rationale: `
WHAT: Issues the bridge 'extensions {}' query and asserts the reply is ok, r.items is an
array of length >= 1, and EVERY entry has a string id and a boolean active.

WHY THIS OUTCOME: The handler maps vscode.extensions.all → [{id, active}], so a healthy
ext-host returns a non-empty, well-typed array (every installed extension, with its live
activation state). A string id + boolean active per entry is the exact shape contract;
anything else means the query handler or vscode.extensions.all access regressed.

WHY IT MATTERS: Smoke for the 'extensions' query path — the prerequisite for every
fleet-bridge presence/active assertion below. An empty or malformed result here means the
ext-host or query handler is broken, not a specific extension. needs:[extensions] → SKIP
cleanly without the cap.`,
    async run(env) {
      const r = await env.request({ type: "extensions" });
      const items = (r && (r.items ?? r.data?.items)) || [];
      const arr = Array.isArray(items) ? items : [];
      const wellShaped =
        arr.length >= 1 && arr.every((e) => e && typeof e.id === "string" && typeof e.active === "boolean");
      return {
        pass: wellShaped,
        detail: wellShaped
          ? `extensions query returned ${arr.length} well-shaped {id,active} entries`
          : `extensions query returned ${arr.length} entries; shape ${wellShaped ? "ok" : "INVALID"}`,
        evidence: { count: arr.length, sample: arr.slice(0, 3) },
      };
    },
  },

  {
    id: "ext.fleetBridgePresent",
    specId: "L1.EXT.010",
    title: "Extensions: fleet-bridge is present in the installed list",
    tags: ["extensions"],
    needs: ["extensions"],
    rationale: `
WHAT: Issues 'extensions {}' and asserts some entry's id matches /fleet[-.]?bridge/i — the
fleet-bridge .vsix is installed in the image.

WHY THIS OUTCOME: The image's build step installs the fleet-bridge .vsix, so its
<publisher>.fleet-bridge id must appear in vscode.extensions.all. Presence (this entry) is
distinct from ACTIVE (EXT.011): listed-but-inactive is the silent-trust-failure mode, so we
separate the two checks.

WHY IT MATTERS: The cheapest proof the harness's own driver shipped into the image — if the
bridge isn't even listed, the .vsix install step broke. A break here (with EXT.001 green)
points specifically at the install step, not the query. needs:[extensions] → SKIP cleanly.`,
    async run(env) {
      const r = await env.request({ type: "extensions" });
      const items = (r && (r.items ?? r.data?.items)) || [];
      const arr = Array.isArray(items) ? items : [];
      const match = arr.find((e) => isFleetBridge(e && e.id));
      return {
        pass: !!match,
        detail: match
          ? `fleet-bridge present in installed list (id=${match.id})`
          : `fleet-bridge NOT found among ${arr.length} extensions`,
        evidence: { match, ids: arr.map((e) => e && e.id).filter(Boolean).slice(0, 20) },
      };
    },
  },

  {
    id: "ext.fleetBridgeActive",
    specId: "L1.EXT.011",
    title: "Extensions: fleet-bridge is ACTIVE (the silent-trust-failure guard)",
    tags: ["extensions"],
    needs: ["extensions"],
    rationale: `
WHAT: Issues 'extensions {}', finds the fleet-bridge entry, and asserts its active === true.

WHY THIS OUTCOME: THE GOTCHA as a test. The bridge needs extensionKind:["workspace"] +
capabilities.untrustedWorkspaces in its manifest AND the image must disable
security.workspace.trust — otherwise it installs but SILENTLY never activates (no log). With
a workbench client connected (Playwright opened the editor in reset()), a correctly wired
bridge reports active:true. This is also transitively true whenever any behaviour passes
(they all go through the bridge), but an explicit assertion makes the diagnosis unambiguous.

WHY IT MATTERS: installed-but-inactive is the EXACT silent failure mode from engineering spec §8. If
this goes red while EXT.010 (present) is green, the trust/extensionKind/manifest wiring
regressed — a precise, loud signal rather than a mysterious suite-wide skip/hang.
needs:[extensions] → SKIP cleanly without the cap.`,
    async run(env) {
      const r = await env.request({ type: "extensions" });
      const items = (r && (r.items ?? r.data?.items)) || [];
      const arr = Array.isArray(items) ? items : [];
      const match = arr.find((e) => isFleetBridge(e && e.id));
      const active = !!(match && match.active === true);
      return {
        pass: active,
        detail: match
          ? `fleet-bridge (id=${match.id}) active=${match.active}`
          : "fleet-bridge entry not found — cannot assert active",
        evidence: { match },
      };
    },
  },

  {
    id: "ext.helloCaps",
    specId: "L1.EXT.020",
    title: "Extensions: the bridge advertises the full frozen CAPS set",
    tags: ["extensions"],
    needs: ["query"],
    rationale: `
WHAT: For every token in the frozen CAPS list (extension.ts), asserts env.supports(token)
returns true — i.e. the bridge's 'hello' frame advertised the complete capability set the
harness captured on connect.

WHY THIS OUTCOME: The hello.caps handshake is what gates needs[] skips. If caps DRIFT (a
refactor drops a token), behaviours that need it silently SKIP instead of running, hiding
regressions. env.supports() consults the recorded hello.caps per env, so asserting every
frozen token is present pins the advertised set against the CAPS const — a missing cap
becomes a LOUD failure here, not a silent skip elsewhere.

WHY IT MATTERS: Guards the capability handshake itself — the meta-contract underpinning the
whole needs[]-gating model. A break here means a cap was dropped from CAPS (or the hello
frame wasn't captured), which would quietly erode coverage. needs:[query] is the minimal
baseline cap (the env must be connected).`,
    async run(env) {
      const missing = EXPECTED_CAPS.filter((c) => !env.supports(c));
      return {
        pass: missing.length === 0,
        detail: missing.length === 0
          ? `all ${EXPECTED_CAPS.length} frozen CAPS advertised by the bridge`
          : `bridge is MISSING caps: ${JSON.stringify(missing)}`,
        evidence: { expected: EXPECTED_CAPS, missing },
      };
    },
  },

  {
    id: "ext.openExtensionsView",
    specId: "L1.EXT.040",
    title: "Extensions: Open the Extensions view command resolves (UI surface)",
    tags: ["extensions", "views"],
    needs: ["command"],
    rationale: `
WHAT: Runs 'workbench.view.extensions' via env.act and asserts it resolves ok — the
human-facing UI surface for the same installed list the 'extensions' query reads
programmatically.

WHY THIS OUTCOME: The Extensions side-bar view focuses on command dispatch; the focused-view
id isn't in the Snapshot, so "command resolved" (env.act throws on !ok) is the faithful
observable. Pairs the UI command with the programmatic query so both paths to "what's
installed" are covered. (Cross-ref VIEW.043 for the no-folder edge.)

WHY IT MATTERS: Covers the View-menu Extensions id; a thrown env.act localises the break to
command/view-switch wiring rather than the extensions feature. needs:[command] only.`,
    async run(env) {
      await env.act("workbench.view.extensions");
      await sleep(400);
      await env.observe("ext.openExtensionsView.after");
      return { pass: true, detail: "workbench.view.extensions resolved" };
    },
  },

  {
    id: "ext.shapeStableMinimal",
    specId: "L1.EXT.060",
    title: "Extensions: query shape is stable on a minimal extension set",
    tags: ["extensions"],
    needs: ["extensions"],
    rationale: `
WHAT: Issues 'extensions {}' and asserts ok, r.items length >= 1, and the fleet-bridge id is
present — proving the list is well-formed even on a base image (built-ins + fleet-bridge
only, no language servers).

WHY THIS OUTCOME: On the leanest extension set the query must still return a well-formed,
non-empty array containing at least the built-ins and the fleet-bridge entry — it must not
throw or return empty on a sparse set. The fleet-bridge entry is the Fleet-relevant
guaranteed member, so its presence anchors the assertion.

WHY IT MATTERS: EDGE (minimal env) — guards the query against a sparse extension set
(complements EXT.001, which may run under repo scenarios with more extensions). A break
where a minimal set returns empty/throws would mean the query mishandles small lists.
needs:[extensions] → SKIP cleanly.`,
    async run(env) {
      const r = await env.request({ type: "extensions" });
      const items = (r && (r.items ?? r.data?.items)) || [];
      const arr = Array.isArray(items) ? items : [];
      const hasBridge = arr.some((e) => isFleetBridge(e && e.id));
      const pass = arr.length >= 1 && hasBridge;
      return {
        pass,
        detail: pass
          ? `minimal extensions list well-formed (${arr.length} entries, fleet-bridge present)`
          : `minimal extensions list issue (count=${arr.length}, fleet-bridge present=${hasBridge})`,
        evidence: { count: arr.length, hasBridge },
      };
    },
  },

  {
    id: "ext.bridgeLogActivation",
    specId: "L1.EXT.070",
    title: "Extensions: the fleet-bridge log records activation (out-of-band witness)",
    tags: ["extensions"],
    needs: ["extensions"],
    rationale: `
WHAT: Reads the in-container bridge log (/tmp/fleet-mux/bridge-<FLEET_SERVER_ID>.log, the
deterministic path from extension.ts) via env.exec and asserts it contains an 'activate:'
line AND a 'ws open' / hello line — the extension's own on-disk activation witnesses.

WHY THIS OUTCOME: extension.ts log()s 'activate: url=... serverId=...' on activation and a
connect/hello line when the WS opens. A bridge that genuinely activated and dialed home
leaves both on disk. This cross-checks the in-VS-Code activation state (EXT.011) against an
INDEPENDENT on-disk witness — if the query says active but this log is empty (or vice
versa), the discrepancy localises the bug (query plumbing vs real activation).

WHY IT MATTERS: An independent witness for activation — the log path is deterministic from
FLEET_SERVER_ID (== env.id), so we read exactly this env's log. A break where the log lacks
'activate:' while the query reports active would expose a query that lies about activation.
We tolerate the exact wording of the connect line (match 'hello' or 'ws open'). The behaviour
gates on extensions (a serving bridge) and reads the log via the always-available env.exec.`,
    async run(env) {
      const logPath = `/tmp/fleet-mux/bridge-${env.id}.log`;
      // Touch the bridge so any lazy logging has fired (a cheap round-trip).
      await env.request({ type: "extensions" }).catch(() => {});
      await sleep(300);
      const text = env.exec(`cat ${logPath} 2>/dev/null || true`);
      const hasActivate = /activate:/.test(text);
      const hasConnect = /ws open|hello/i.test(text);
      const pass = hasActivate && hasConnect;
      return {
        pass,
        detail: pass
          ? `bridge log records activation (activate: + ws/hello) at ${logPath}`
          : `bridge log missing markers (activate=${hasActivate}, ws/hello=${hasConnect}) at ${logPath}`,
        evidence: { logPath, activate: hasActivate, connect: hasConnect, tail: String(text).slice(-300) },
      };
    },
  },

  {
    id: "ext.bridgeLogCommand",
    specId: "L1.EXT.071",
    title: "Extensions: the fleet-bridge log records a forwarded command (recv + ok)",
    tags: ["extensions"],
    needs: ["command"],
    rationale: `
WHAT: Runs 'workbench.action.showCommands' via env.act, then reads the in-container bridge
log via env.exec and asserts it contains BOTH 'command recv: workbench.action.showCommands'
AND 'command ok: workbench.action.showCommands' — the on-disk counterpart of the ok:true
reply.

WHY THIS OUTCOME: extension.ts logs 'command recv: <id>' on receipt and 'command ok: <id>'
after a successful executeCommand. So a forwarded command that ran leaves BOTH lines. This
is the out-of-band trace that witnesses receipt AND successful execution — distinct from the
in-band ok:true reply.

WHY IT MATTERS: EDGE (out-of-band command tracing) — invaluable for debugging a behaviour
that times out: if 'recv' is present but 'ok' is not, the command hung INSIDE VS Code, not in
transport. This pins both log lines so that diagnostic split stays reliable. We poll the log
briefly since appendFileSync is async-ish relative to the reply. needs:[command] only; the
log read uses the always-available env.exec.`,
    async run(env) {
      const id = "workbench.action.showCommands";
      const logPath = `/tmp/fleet-mux/bridge-${env.id}.log`;
      await env.act(id);
      let recv = false;
      let ok = false;
      let text = "";
      for (let i = 0; i < 10; i++) {
        await sleep(400);
        text = env.exec(`cat ${logPath} 2>/dev/null || true`);
        recv = text.includes(`command recv: ${id}`);
        ok = text.includes(`command ok: ${id}`);
        if (recv && ok) break;
      }
      const pass = recv && ok;
      return {
        pass,
        detail: pass
          ? `bridge log records '${id}' recv + ok`
          : `bridge log missing command markers (recv=${recv}, ok=${ok}) for ${id}`,
        evidence: { logPath, recv, ok, tail: String(text).slice(-400) },
      };
    },
  },
];
