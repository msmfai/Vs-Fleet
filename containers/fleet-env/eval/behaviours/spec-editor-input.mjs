// SPEC implementations for areas L1.EDITOR (10-editor.md) and L1.INPUT (1a-input.md).
//
// Each behaviour drives a REAL VS Code action via the bridge and ASSERTS the
// effect on an observable — the `query` Snapshot (activeEditor / visibleEditors /
// openTabs / editorText / selection / terminalCount / terminals), a `fileContent`
// / `terminalText` query reply, or out-of-band `exec` (docker exec cat/ls/test) —
// never "command returned ok". Behaviours needing a Track-E cap declare needs:[...]
// so the runner SKIPS them cleanly until the bridge advertises that cap.
//
// See behaviours/_contract.mjs for the Behaviour shape, lib/env.mjs for the Env
// surface, and the bridge wire (packages/fleet-bridge/src/extension.ts):
//   - openFile on a missing path THROWS (reply ok:false) and does NOT create it.
//   - typeText with no activeTextEditor THROWS ("no active editor") — never inserts.
//   - typeText INSERTS AT THE CURSOR (b.insert(ed.selection.active, text)) in
//     vscode.window.activeTextEditor (the active TEXT editor — a focused terminal
//     does NOT steal it). It does NOT replace an active selection — hence
//     L1.INPUT.005 (select-all + type overwrites) is left TODO.
//   - openTabs is the list of tab LABELS (basenames / "Welcome" / "Untitled-1").
//   - selection is {start:{line,character},end:{line,character}} of the active editor.
//   - fileContent prefers the open in-memory doc (reflects unsaved edits) else disk.
//   - integrated terminals are all named "bash" by default, so name-routing to one
//     of two same-named terminals is not testable headlessly — L1.INPUT.011 is TODO.
//
// Patterns copied from files.mjs / agentInput.mjs / terminal_more.mjs.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const PROJECT = "/home/coder/project";

// Pull a §3.3-query payload field whether the bridge spreads it onto the result
// msg (the query shape) or nests it under `.data` (the snapshot shape).
const field = (r, key) => (r && r[key] !== undefined ? r[key] : r?.data?.[key]);

const base = (p) => (p ? String(p).split("/").pop() : p);

const isActive = (snap, path) => {
  const a = snap?.activeEditor;
  return !!a && (a === path || base(a) === base(path));
};

const tabLen = (snap) => (Array.isArray(snap?.openTabs) ? snap.openTabs.length : null);
const visLen = (snap) =>
  Array.isArray(snap?.visibleEditors) ? snap.visibleEditors.length : null;

// Read a fileContent query's text (both result shapes).
async function readFile(env, path) {
  const r = await env.request({ type: "fileContent", path }).catch(() => null);
  const t = field(r, "text");
  return typeof t === "string" ? t : null;
}

// Poll fileContent until `needle` appears (shell writes / saves are async).
async function pollFile(env, path, needle, { tries = 14, gap = 500 } = {}) {
  let text = "";
  for (let i = 0; i < tries; i++) {
    await sleep(gap);
    const t = await readFile(env, path);
    text = t || "";
    if (text.includes(needle)) return { hit: true, text };
  }
  return { hit: false, text };
}

// Poll a terminalText query until `needle` appears.
async function pollTerm(env, needle, { name, tries = 15, gap = 800 } = {}) {
  let text = "";
  for (let i = 0; i < tries; i++) {
    await sleep(gap);
    const r = await env
      .request({ type: "terminalText", ...(name ? { name } : {}) })
      .catch(() => null);
    text = field(r, "text") || "";
    if (text.includes(needle)) return { hit: true, text };
  }
  return { hit: false, text };
}

// Empty the workbench: repeat closeAllEditors until openTabs is 0 (or we give up).
async function closeAll(env, { tries = 5 } = {}) {
  for (let i = 0; i < tries; i++) {
    await env.act("workbench.action.closeAllEditors").catch(() => {});
    await sleep(700);
    const snap = (await env.observe("closeAll.poll")).vscode;
    if (tabLen(snap) === 0) return true;
  }
  return false;
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ── L1.EDITOR.002 — openFile on a missing path is a clean no-op ───────────────
  {
    id: "editor.openMissingNoop",
    specId: "L1.EDITOR.002",
    title: "openFile on a missing path does not focus a phantom editor nor create the file",
    tags: ["editor"],
    isolation: "fresh",
    needs: ["openFile"],
    rationale: `
WHAT: With \`PROJECT/fleet-does-not-exist.txt\` confirmed absent (\`exec test -f\` → no),
captures the \`activeEditor\` before, requests \`openFile {path}\` on the missing path, and
asserts EITHER the bridge replied \`ok:false\` OR the \`activeEditor\` is unchanged — AND in
both cases the file is STILL absent on disk afterwards.

WHY THIS IS CORRECT: Opening a path that does not exist must not fabricate a phantom
editor. The bridge's \`openFile\` opens an existing document; on a missing path it throws
(reply \`ok:false\`, surfaced by \`env.request\` as a rejection) and never calls
\`workbench.openTextDocument\` in a create-on-open mode. So the only correct post-states are
(a) the request rejected, or (b) nothing changed — the previously active editor is still
active. Crucially, the read must not have CREATED the file, so we re-check \`test -f\`.

WHY IT MATTERS: This is the empty-precondition edge of the write→open→read loop
(L1.EDITOR.001). A regression where \`openFile\` silently materialised a missing path as an
empty unsaved editor — or worse, touched it onto disk — would let agents "open" files that
don't exist and operate on phantoms. Asserting both the no-focus and the no-create halves
catches either failure mode.`,
    async run(env) {
      const path = `${PROJECT}/fleet-does-not-exist.txt`;
      env.exec(`rm -f ${path}`);
      const absentBefore = env.exec(`test -f ${path} && echo yes || echo no`) === "no";

      const before = await env.observe("editor.openMissingNoop.before");

      let replyOk = true;
      try {
        await env.request({ type: "openFile", path });
      } catch {
        replyOk = false; // bridge replied ok:false → env.request threw
      }
      await sleep(800);

      const after = await env.observe("editor.openMissingNoop.after");
      const unchanged = before.vscode.activeEditor === after.vscode.activeEditor;
      const absentAfter = env.exec(`test -f ${path} && echo yes || echo no`) === "no";

      const noPhantom = replyOk === false || unchanged;
      return {
        pass: absentBefore && noPhantom && absentAfter,
        detail:
          `reply ${replyOk ? "ok" : "ok:false"}; activeEditor ${unchanged ? "unchanged" : "CHANGED"}; ` +
          `file ${absentAfter ? "still absent" : "WAS CREATED"}`,
        evidence: {
          activeBefore: before.vscode.activeEditor,
          activeAfter: after.vscode.activeEditor,
          replyOk,
          absentBefore,
          absentAfter,
        },
      };
    },
  },

  // ── L1.EDITOR.005 — closeAllEditors empties openTabs ──────────────────────────
  {
    id: "editor.closeAllEmpties",
    specId: "L1.EDITOR.005",
    title: "closeAllEditors empties openTabs and clears activeEditor",
    tags: ["editor"],
    isolation: "fresh",
    needs: ["openFile", "writeFile"],
    rationale: `
WHAT: Writes and opens two files (\`a.txt\`, \`b.txt\`) so \`openTabs.length >= 2\`, fires
\`workbench.action.closeAllEditors\`, then asserts \`openTabs.length === 0\` and
\`activeEditor == null\`.

WHY THIS IS CORRECT: \`closeAllEditors\` closes EVERY editor in every group, so the
workbench is left with no open tabs and no active editor. We open two real files first so
the close has something non-trivial to tear down (distinguishing this from the single-close
path). The exact \`=== 0\` / \`== null\` assertion is safe under \`fresh\` isolation: a fresh
env starts with at most the Welcome tab, which closeAllEditors also closes, so the post
state is deterministically empty.

WHY IT MATTERS: This is the bulk-teardown path, distinct from \`closeActiveEditor\` (which
removes one). It guards the snapshot reflecting a fully emptied editor area — both the
\`openTabs\` count collapsing to 0 AND \`activeEditor\` being nulled (not left pointing at a
disposed editor). A regression that closed the tabs but left a stale \`activeEditor\`, or
that missed editors in non-active groups, is caught by the two-part assertion.`,
    async run(env) {
      const a = `${PROJECT}/a.txt`;
      const b = `${PROJECT}/b.txt`;
      await env.request({ type: "writeFile", path: a, content: "aaa\n" });
      await env.request({ type: "writeFile", path: b, content: "bbb\n" });
      await env.request({ type: "openFile", path: a });
      await env.request({ type: "openFile", path: b });
      await sleep(900);

      const before = await env.observe("editor.closeAllEmpties.before");
      await env.act("workbench.action.closeAllEditors").catch(() => {});
      await sleep(900);
      const after = await env.observe("editor.closeAllEmpties.after");

      const tabsZero = tabLen(after.vscode) === 0;
      const noActive =
        after.vscode.activeEditor == null || after.vscode.activeEditor === "";
      return {
        pass: tabsZero && noActive,
        detail:
          `openTabs ${tabLen(before.vscode)} → ${tabLen(after.vscode)} (want 0); ` +
          `activeEditor=${JSON.stringify(after.vscode.activeEditor)} (want null)`,
        evidence: {
          tabsBefore: before.vscode.openTabs,
          tabsAfter: after.vscode.openTabs,
          activeAfter: after.vscode.activeEditor,
        },
      };
    },
  },

  // ── L1.EDITOR.008 — splitEditorDown stacks a second pane below ────────────────
  {
    id: "editor.splitDown",
    specId: "L1.EDITOR.008",
    title: "splitEditorDown adds a second visible editor (vertical split)",
    tags: ["editor"],
    isolation: "fresh",
    needs: ["openFile", "writeFile"],
    rationale: `
WHAT: Writes and opens \`fleet-splitdown.txt\` so there is exactly one visible editor,
counts \`visibleEditors\`, runs \`workbench.action.splitEditorDown\`, then asserts the
visible-editor count grew by exactly one (before + 1).

WHY THIS IS CORRECT: \`splitEditorDown\` clones the active document into a NEW editor group
stacked BELOW the current one — a vertical split — so both groups are simultaneously
visible and \`visibleEditors\` must increase by one. It is a distinct orientation command
from \`workbench.action.splitEditor\` (horizontal, tested by \`editor.splitRight\` in
files.mjs); both must register a new visible group. We open a real file first because
splitting an empty workbench has nothing to clone, and \`fresh\` isolation gives a clean
starting visible count so the exact +1 delta is deterministic.

WHY IT MATTERS: This guards the down/vertical split orientation specifically. A regression
where only the horizontal split registered a new group — or where the snapshot's
\`visibleEditors\` stopped tracking grouped panes for vertical splits — would leave the count
flat and trip this. It complements \`editor.splitRight\` so a reader can tell an
orientation-specific break from a general split-tracking break.`,
    async run(env) {
      const path = `${PROJECT}/fleet-splitdown.txt`;
      await env.request({ type: "writeFile", path, content: "split me down\n" });
      await env.request({ type: "openFile", path });
      await sleep(800);

      const before = await env.observe("editor.splitDown.before");
      const beforeVis = visLen(before.vscode);
      await env.act("workbench.action.splitEditorDown").catch(() => {});
      await sleep(1000);
      const after = await env.observe("editor.splitDown.after");
      const afterVis = visLen(after.vscode);

      const measurable = beforeVis !== null && afterVis !== null;
      return {
        pass: measurable && afterVis === beforeVis + 1,
        detail: measurable
          ? `visibleEditors ${beforeVis} → ${afterVis} (want +1)`
          : "visibleEditors not exposed by snapshot — cannot assert",
        evidence: {
          before: before.vscode.visibleEditors,
          after: after.vscode.visibleEditors,
        },
      };
    },
  },

  // ── L1.EDITOR.009 — next/previous editor cycles the active editor ─────────────
  {
    id: "editor.nextPrevCycles",
    specId: "L1.EDITOR.009",
    title: "nextEditor / previousEditor cycles the active editor among open tabs",
    tags: ["editor"],
    isolation: "fresh",
    needs: ["openFile", "writeFile"],
    rationale: `
WHAT: Opens two files \`a.txt\` then \`b.txt\` in one group (so \`b.txt\` is active), runs
\`workbench.action.nextEditor\` and asserts the active editor moved off \`b.txt\`, then runs
\`workbench.action.previousEditor\` and asserts it returned to \`b.txt\`.

WHY THIS IS CORRECT: With two tabs in a group, \`nextEditor\` advances the active tab to the
OTHER document, so \`activeEditor\` must no longer be \`b.txt\`; \`previousEditor\` reverses
that step, returning the active editor to \`b.txt\`. We assert the round-trip rather than a
specific intermediate basename because tab ordering after two opens is the only invariant we
need — "next moves away, previous comes back". \`fresh\` isolation guarantees exactly the two
tabs we opened, so the cycle is deterministic.

WHY IT MATTERS: Tab navigation must change the snapshot's \`activeEditor\` deterministically;
agents and users rely on cycling between open files. A regression where next/previous no-op,
desync from the snapshot, or wrap incorrectly would leave \`activeEditor\` stuck on \`b.txt\`
(failing the next-moves-away half) or fail to return (failing the previous half). The
two-step round-trip pins both directions.`,
    async run(env) {
      const a = `${PROJECT}/a.txt`;
      const b = `${PROJECT}/b.txt`;
      await env.request({ type: "writeFile", path: a, content: "aaa\n" });
      await env.request({ type: "writeFile", path: b, content: "bbb\n" });
      await env.request({ type: "openFile", path: a });
      await env.request({ type: "openFile", path: b });
      await sleep(900);

      const start = await env.observe("editor.nextPrevCycles.start");
      const startOnB = isActive(start.vscode, b);

      await env.act("workbench.action.nextEditor").catch(() => {});
      await sleep(800);
      const next = await env.observe("editor.nextPrevCycles.next");
      const movedAway = !isActive(next.vscode, b);

      await env.act("workbench.action.previousEditor").catch(() => {});
      await sleep(800);
      const prev = await env.observe("editor.nextPrevCycles.prev");
      const returned = isActive(prev.vscode, b);

      return {
        pass: startOnB && movedAway && returned,
        detail:
          `start active=${JSON.stringify(start.vscode.activeEditor)} (want ${base(b)}); ` +
          `after next moved-away=${movedAway}; after prev returned=${returned}`,
        evidence: {
          start: start.vscode.activeEditor,
          afterNext: next.vscode.activeEditor,
          afterPrev: prev.vscode.activeEditor,
        },
      };
    },
  },

  // ── L1.EDITOR.015 — new untitled file opens as an unsaved editor ──────────────
  {
    id: "editor.newUntitled",
    specId: "L1.EDITOR.015",
    title: "newUntitledFile opens an untitled (unsaved) editor and grows openTabs",
    tags: ["editor"],
    isolation: "fresh",
    needs: ["openFile"],
    rationale: `
WHAT: Captures \`openTabs.length == N\`, runs \`workbench.action.files.newUntitledFile\`,
then asserts \`openTabs.length === N + 1\` AND the new \`activeEditor\` matches an untitled
document (label \`Untitled\` or scheme \`untitled:\`).

WHY THIS IS CORRECT: \`newUntitledFile\` opens a fresh in-memory document with NO on-disk
path — an untitled editor under the \`untitled:\` scheme. So the open-tab count rises by
exactly one and the active editor references an \`Untitled-N\` label, not a filesystem path.
We match the label/scheme tolerantly (\`/^Untitled|untitled:/\`) because the snapshot may
report either the tab label or the URI. \`fresh\` isolation makes the +1 delta clean.

WHY IT MATTERS: Untitled docs are the one editor kind with no path, so they stress the
snapshot's ability to represent scheme-only editors and to count them. A regression where
the snapshot dropped pathless editors (so \`openTabs\` didn't grow), or mislabelled the
active untitled editor, would be caught. This is the foundation for typing into a pathless
buffer (L1.INPUT.002).`,
    async run(env) {
      const before = await env.observe("editor.newUntitled.before");
      const beforeN = tabLen(before.vscode);

      await env.act("workbench.action.files.newUntitledFile").catch(() => {});
      await sleep(900);

      const after = await env.observe("editor.newUntitled.after");
      const afterN = tabLen(after.vscode);
      const active = after.vscode.activeEditor || "";
      const isUntitled = /Untitled|untitled:/i.test(String(active));

      const measurable = beforeN !== null && afterN !== null;
      const grew = measurable && afterN === beforeN + 1;
      return {
        pass: grew && isUntitled,
        detail: measurable
          ? `openTabs ${beforeN} → ${afterN} (want +1); activeEditor=${JSON.stringify(active)} untitled=${isUntitled}`
          : "openTabs not exposed by snapshot — cannot assert",
        evidence: {
          tabsBefore: before.vscode.openTabs,
          tabsAfter: after.vscode.openTabs,
          activeAfter: active,
        },
      };
    },
  },

  // ── L1.EDITOR.023 — Select All selects the whole document ─────────────────────
  {
    id: "editor.selectAll",
    specId: "L1.EDITOR.023",
    title: "selectAll spans the whole document (0,0)→(lastLine,…)",
    tags: ["editor"],
    isolation: "fresh",
    needs: ["writeFile", "openFile"],
    rationale: `
WHAT: Writes a 3-line file \`fleet-selall.txt\`, opens it, runs
\`editor.action.selectAll\`, then asserts the resulting \`selection\` starts at
\`{line:0,character:0}\` and ENDS on the last line (line index 2 for a 3-line doc).

WHY THIS IS CORRECT: \`selectAll\` selects from the very start of the document to its end,
so the \`selection\` Snapshot field must report \`start == {0,0}\` and \`end.line\` equal to the
final line index. The file has three content lines (\`one\\ntwo\\nthree\\n\`); the trailing
newline can leave the document end on line 2 (end of "three") or line 3 (the empty line
after it), so we accept \`end.line >= 2\` to tolerate that boundary while still proving the
selection reached the last real line — never collapsing to a caret. We require the
\`selection\` cap (Track-D/E) to read this field.

WHY IT MATTERS: This proves the \`selection\` Snapshot field reports a real multi-line range,
not just a caret position — the foundation every selection-based input assertion rests on.
A regression where \`selection\` stopped reflecting command-driven selection changes, or
reported only the caret, would collapse the range and trip this. (Note: typeText does NOT
overwrite this selection — see the L1.INPUT.005 TODO — so this is purely a selection-state
guard.)`,
    async run(env) {
      const path = `${PROJECT}/fleet-selall.txt`;
      await env.request({ type: "writeFile", path, content: "one\ntwo\nthree\n" });
      await env.request({ type: "openFile", path });
      await sleep(800);
      await env.act("cursorTop").catch(() => {});
      await sleep(300);

      await env.act("editor.action.selectAll").catch(() => {});
      await sleep(600);
      const after = await env.observe("editor.selectAll.after");
      const sel = after.vscode.selection;

      const startOk =
        !!sel && sel.start && sel.start.line === 0 && sel.start.character === 0;
      const endOk = !!sel && sel.end && typeof sel.end.line === "number" && sel.end.line >= 2;
      return {
        pass: !!sel && startOk && endOk,
        detail: sel
          ? `selection ${JSON.stringify(sel.start)}→${JSON.stringify(sel.end)} (want start 0,0; end.line>=2)`
          : "selection not exposed by snapshot — cannot assert",
        evidence: { selection: sel },
      };
    },
  },

  // ── L1.EDITOR.028 — open the same file twice focuses the existing tab ─────────
  {
    id: "editor.openSameNoDup",
    specId: "L1.EDITOR.028",
    title: "Opening the same file twice focuses the existing tab (no duplicate)",
    tags: ["editor"],
    isolation: "fresh",
    needs: ["openFile", "writeFile"],
    rationale: `
WHAT: Writes \`c.txt\`, opens it once (one tab, \`openTabs.length == T\`), then requests
\`openFile {path}\` a SECOND time and asserts the tab count is UNCHANGED (no +1) and \`c.txt\`
is the active editor.

WHY THIS IS CORRECT: VS Code is tab-idempotent — opening a document that is already open
re-focuses the existing tab rather than creating a second one. So the open-tab count must
stay at \`T\` across the repeat open, and the file must be (re)focused as the active editor.
We compare the count delta (== before) rather than a hard number because the fresh-window
baseline (e.g. a Welcome tab) is environment-dependent; "no growth on re-open" is the true
invariant. \`fresh\` isolation keeps the baseline stable.

WHY IT MATTERS: This guards \`openFile\` idempotence — the repeat edge of L1.EDITOR.001. A
regression opening a duplicate tab on every \`openFile\` would inflate \`openTabs\` and leak
editors as agents re-open files they're working on. Asserting both "no new tab" and "still
focused" catches a duplicate-open as well as a re-open that lost focus.`,
    async run(env) {
      const path = `${PROJECT}/c.txt`;
      await env.request({ type: "writeFile", path, content: "see me\n" });
      await env.request({ type: "openFile", path });
      await sleep(800);

      const before = await env.observe("editor.openSameNoDup.before");
      const beforeN = tabLen(before.vscode);

      await env.request({ type: "openFile", path });
      await sleep(800);

      const after = await env.observe("editor.openSameNoDup.after");
      const afterN = tabLen(after.vscode);
      const active = isActive(after.vscode, path);

      const measurable = beforeN !== null && afterN !== null;
      const noDup = measurable && afterN === beforeN;
      return {
        pass: noDup && active,
        detail: measurable
          ? `openTabs ${beforeN} → ${afterN} (want no change); active=${active} (want ${base(path)})`
          : "openTabs not exposed by snapshot — cannot assert",
        evidence: {
          tabsBefore: before.vscode.openTabs,
          tabsAfter: after.vscode.openTabs,
          activeAfter: after.vscode.activeEditor,
        },
      };
    },
  },

  // ── L1.INPUT.002 — typeText into a fresh untitled editor populates editorText ──
  {
    id: "input.typeUntitled",
    specId: "L1.INPUT.002",
    title: "typeText into a fresh untitled editor surfaces via editorText",
    tags: ["input", "editor"],
    isolation: "fresh",
    needs: ["typeText"],
    rationale: `
WHAT: Opens a fresh untitled editor (\`workbench.action.files.newUntitledFile\`), requests
\`typeText {text:"HELLO_UNTITLED"}\`, then asserts the snapshot's \`editorText\` includes the
typed string.

WHY THIS IS CORRECT: An untitled editor is the active text editor with NO on-disk path, so
\`typeText\` (which inserts at the active text editor's caret) lands its characters in that
in-memory buffer. With no file path there is nothing to read via \`fileContent\`/\`exec\` — the
only observable is the live \`editorText\` Snapshot field, which must reflect the typed
content. (Reading \`editorText\` is itself the Track-D/E \`selection\`-family capability the
bridge advertises; absent → SKIP.)

WHY IT MATTERS: This is the no-path edge of L1.INPUT.001 — it proves \`typeText\` works
BEFORE any save target exists, and that the \`editorText\` field surfaces pathless buffers.
A regression where typeText silently required a backing file, or where \`editorText\` only
reflected disk-backed editors, would be caught here while the disk-backed input tests
stayed green — narrowing the break to the pathless/editorText path.`,
    async run(env) {
      await env.act("workbench.action.files.newUntitledFile").catch(() => {});
      await sleep(900);
      const typed = "HELLO_UNTITLED";
      await env.request({ type: "typeText", text: typed });
      await sleep(700);

      const after = await env.observe("input.typeUntitled.after");
      const editorText = after.vscode.editorText;
      const has = typeof editorText === "string" && editorText.includes(typed);
      return {
        pass: has,
        detail: has
          ? `editorText contains "${typed}"`
          : `editorText did not contain "${typed}" (got ${JSON.stringify(String(editorText ?? "").slice(0, 80))})`,
        evidence: { editorText: String(editorText ?? "").slice(0, 200), typed },
      };
    },
  },

  // ── L1.INPUT.003 — typeText with NO active editor is a clean no-op ────────────
  {
    id: "input.typeNoEditor",
    specId: "L1.INPUT.003",
    title: "typeText with no active editor is a clean no-op (no phantom file, no hang)",
    tags: ["input", "editor"],
    isolation: "fresh",
    needs: ["closeEditor", "typeText"],
    rationale: `
WHAT: Closes all editors (so \`activeEditor == null\`), lists \`PROJECT\` before, requests
\`typeText {text:"NOWHERE"}\`, and asserts: the request did NOT report a successful insert
(it rejected, i.e. \`ok:false\`), \`activeEditor\` is still null, and no new file appeared in
\`PROJECT\`.

WHY THIS IS CORRECT: The bridge's \`typeText\` targets \`vscode.window.activeTextEditor\`;
with no active editor it THROWS ("no active editor"), surfaced as a rejected request — it
must NOT fabricate a buffer or write a file. So the correct post-state is: the request
rejected, nothing is active, and the workspace file listing is unchanged. We capture the
\`ls\` before/after to prove no phantom file was created.

WHY IT MATTERS: This is the empty-state edge of L1.INPUT.001 — input with no target. A
regression where \`typeText\` silently created a scratch buffer, error-hung the bridge, or
(worst) materialised a file would be caught: the rejection guards against a silent success,
and the \`ls\` parity guards against a phantom file. Input must fail loudly-and-safely, never
land somewhere unexpected.`,
    async run(env) {
      await closeAll(env);
      const before = await env.observe("input.typeNoEditor.before");
      const lsBefore = env.exec(`ls -1 ${PROJECT}`);

      let inserted = true;
      try {
        const r = await env.request({ type: "typeText", text: "NOWHERE" });
        // If it didn't throw, only count it as a real insert when explicitly so.
        inserted = field(r, "inserted") === true;
      } catch {
        inserted = false; // bridge replied ok:false → no active editor → correct
      }
      await sleep(600);

      const after = await env.observe("input.typeNoEditor.after");
      const lsAfter = env.exec(`ls -1 ${PROJECT}`);
      const stillNull =
        after.vscode.activeEditor == null || after.vscode.activeEditor === "";
      const noNewFile = lsBefore === lsAfter;

      return {
        pass: !inserted && stillNull && noNewFile,
        detail:
          `typeText inserted=${inserted} (want false); activeEditor still null=${stillNull}; ` +
          `PROJECT listing unchanged=${noNewFile}`,
        evidence: {
          activeBefore: before.vscode.activeEditor,
          activeAfter: after.vscode.activeEditor,
          lsBefore,
          lsAfter,
        },
      };
    },
  },

  // ── L1.INPUT.004 — typeText routes to the editor, NOT a focused terminal ──────
  {
    id: "input.typeRoutesEditor",
    specId: "L1.INPUT.004",
    title: "typeText targets the active text editor, not a focused terminal",
    tags: ["input", "editor", "terminal"],
    isolation: "fresh",
    needs: ["writeFile", "openFile", "typeText", "fileContent"],
    rationale: `
WHAT: Opens \`route.txt\`, then opens a new terminal (\`workbench.action.terminal.new\`) so
the terminal is the focused panel ON TOP of the editor, requests
\`typeText {text:"ROUTE_TO_EDITOR"}\`, and asserts the marker landed in \`route.txt\`'s buffer
(\`fileContent\`) and did NOT appear in the terminal buffer (\`terminalText\`).

WHY THIS IS CORRECT: \`typeText\` is defined against \`vscode.window.activeTextEditor\` — the
active TEXT editor — which is independent of which panel (terminal) currently has keyboard
focus. So even with the terminal focused, the keystrokes must insert into \`route.txt\`, not
run as a shell command in the pty. The dual assertion (present in the file, absent from the
terminal) pins the routing boundary precisely.

WHY IT MATTERS: \`typeText\` and \`termSend\` are DISTINCT primitives. A regression routing
\`typeText\` to a focused terminal would silently execute an agent's intended editor
keystrokes as shell commands — a dangerous failure (arbitrary text becomes shell input).
This guards the editor/terminal routing boundary: text must reach the document and never
the pty.`,
    async run(env) {
      const path = `${PROJECT}/route.txt`;
      const marker = "ROUTE_TO_EDITOR";
      await env.request({ type: "writeFile", path, content: "" });
      await env.request({ type: "openFile", path });
      await sleep(800);
      // Focus a terminal ON TOP of the editor.
      await env.act("workbench.action.terminal.new").catch(() => {});
      await sleep(1800);

      await env.request({ type: "typeText", text: marker });
      await sleep(800);
      if (env.supports("saveAll")) await env.request({ type: "saveAll" }).catch(() => {});
      await sleep(500);

      const inFile = (await readFile(env, path)) || "";
      const term = await env
        .request({ type: "terminalText" })
        .catch(() => null);
      const termText = field(term, "text") || "";

      const landedInEditor = inFile.includes(marker);
      const notInTerminal = !termText.includes(marker);
      return {
        pass: landedInEditor && notInTerminal,
        detail:
          `marker in editor=${landedInEditor} (want true); marker in terminal=${!notInTerminal} (want false)`,
        evidence: {
          fileContent: inFile.slice(0, 120),
          terminalText: termText.slice(-120),
        },
      };
    },
  },

  // ── L1.INPUT.006 — cursor-position commands move where typeText inserts ───────
  {
    id: "input.cursorMovesInsert",
    specId: "L1.INPUT.006",
    title: "cursorTop/cursorBottom relocate the insertion point read by typeText",
    tags: ["input", "editor"],
    isolation: "fresh",
    needs: ["writeFile", "openFile", "typeText", "fileContent", "saveAll"],
    rationale: `
WHAT: Opens \`cursor.txt\` seeded \`"AAA\\nBBB\\n"\`, runs \`cursorTop\` then
\`typeText {text:"XX"}\` (inserts at the very top), then \`cursorBottom\` +
\`typeText {text:"YY"}\` (inserts at EOF), saves, and asserts the document STARTS with \`XX\`
and ENDS with \`YY\` (i.e. \`XX\` is before \`AAA\` and \`YY\` is after \`BBB\`).

WHY THIS IS CORRECT: \`typeText\` inserts at the active editor's CARET
(\`b.insert(ed.selection.active, …)\`). \`cursorTop\` moves that caret to \`(0,0)\` so \`XX\`
lands at the document start; \`cursorBottom\` moves it to EOF so \`YY\` lands at the end. The
observable is the resulting text shape: \`XX…\` at the front and \`…YY\` at the back. We assert
the prefix/suffix rather than an exact string to tolerate the file's trailing newline.

WHY IT MATTERS: This proves cursor-moving commands genuinely relocate the insertion point
that \`typeText\` reads — tying the \`selection\`/caret state to typed output. A regression
where typeText ignored the caret (always appended, or always inserted at 0) would put both
\`XX\` and \`YY\` in the same place and break the prefix/suffix shape. It is the mechanism that
lets agents place edits precisely.`,
    async run(env) {
      const path = `${PROJECT}/cursor.txt`;
      await env.request({ type: "writeFile", path, content: "AAA\nBBB\n" });
      await env.request({ type: "openFile", path });
      await sleep(800);

      await env.act("cursorTop").catch(() => {});
      await sleep(300);
      await env.request({ type: "typeText", text: "XX" });
      await sleep(400);

      await env.act("cursorBottom").catch(() => {});
      await sleep(300);
      await env.request({ type: "typeText", text: "YY" });
      await sleep(400);

      await env.request({ type: "saveAll" }).catch(() => {});
      await sleep(700);

      const text = (await readFile(env, path)) || env.exec(`cat ${path}`) || "";
      const t = String(text);
      const startsXX = t.startsWith("XX");
      const endsYY = t.trimEnd().endsWith("YY");
      return {
        pass: startsXX && endsYY,
        detail: `text starts "XX"=${startsXX}, ends "YY"=${endsYY} (${JSON.stringify(t)})`,
        evidence: { content: t },
      };
    },
  },

  // ── L1.INPUT.007 — multi-line typeText inserts real newlines ──────────────────
  {
    id: "input.multilineTypeText",
    specId: "L1.INPUT.007",
    title: "typeText with embedded \\n inserts real line breaks into the buffer",
    tags: ["input", "editor"],
    isolation: "fresh",
    needs: ["writeFile", "openFile", "typeText", "fileContent", "saveAll"],
    rationale: `
WHAT: Opens an empty \`multiline.txt\`, requests \`typeText {text:"line1\\nline2\\nline3"}\`,
saves, and asserts the document content equals \`"line1\\nline2\\nline3"\` (three distinct
lines) and the on-disk file has the expected line count.

WHY THIS IS CORRECT: \`typeText\` inserts the literal text at the caret; an embedded \`\\n\`
must become a REAL line break in the document model (not the two characters backslash-n,
and not a stripped/escaped newline). So a single \`typeText\` of \`"line1\\nline2\\nline3"\`
produces three lines. We assert via \`fileContent\` (the buffer/disk after save) and
corroborate the line count with \`wc -l\` over the file — \`wc -l\` counts trailing newlines,
so two or three is acceptable depending on the final newline; the content-equality check is
the strict observable.

WHY IT MATTERS: Agents write multi-line code in one \`typeText\` call. A regression that
escaped, stripped, or literalised embedded newlines would collapse the three lines into one
mangled line, silently corrupting generated source. This guards \`typeText\` honouring \`\\n\`
as a line break.`,
    async run(env) {
      const path = `${PROJECT}/multiline.txt`;
      await env.request({ type: "writeFile", path, content: "" });
      await env.request({ type: "openFile", path });
      await sleep(800);

      await env.request({ type: "typeText", text: "line1\nline2\nline3" });
      await sleep(600);
      await env.request({ type: "saveAll" }).catch(() => {});
      await sleep(700);

      const buf = (await readFile(env, path)) || "";
      const disk = String(env.exec(`cat ${path}`));
      const wc = Number(String(env.exec(`wc -l < ${path}`)).trim()) || 0;

      // Content must contain the three lines in order (tolerate a trailing newline).
      const wanted = "line1\nline2\nline3";
      const contentOk = buf.replace(/\n$/, "") === wanted || disk.replace(/\n$/, "") === wanted;
      // wc -l counts newline terminators: 2 (no final newline) or 3 (final newline).
      const linesOk = wc >= 2;
      return {
        pass: contentOk && linesOk,
        detail: `content matches 3 lines=${contentOk}; wc -l=${wc} (>=2)`,
        evidence: { buffer: buf, disk, wc },
      };
    },
  },

  // ── L1.INPUT.008 — typeText preserves Unicode / multibyte characters ──────────
  {
    id: "input.unicodeTypeText",
    specId: "L1.INPUT.008",
    title: "typeText preserves Unicode/multibyte characters through saveAll to disk",
    tags: ["input", "editor"],
    isolation: "fresh",
    needs: ["writeFile", "openFile", "typeText", "fileContent", "saveAll"],
    rationale: `
WHAT: Opens an empty \`unicode.txt\`, requests \`typeText {text:"héllo — 你好 🚀"}\`, saves,
and asserts the buffer/disk content includes \`"你好"\` and \`"🚀"\`, and that an out-of-band
\`grep\` for the rocket emoji over the file succeeds (a true UTF-8 round-trip).

WHY THIS IS CORRECT: \`typeText\` inserts the exact text it is given; \`saveAll\` writes it as
UTF-8. So the multibyte CJK characters and the (surrogate-pair) emoji must survive
unchanged through insert→save→disk. We assert via \`fileContent\` (buffer/disk) for the CJK
and the emoji, AND independently \`grep -q\` the emoji bytes on disk via \`exec\` — the shell
read proves real UTF-8 bytes landed, not a mojibake or ASCII-only fallback.

WHY IT MATTERS: Synthetic input must be encoding-safe. A regression to ASCII-only handling,
or one that split a surrogate pair (corrupting the emoji) or re-encoded CJK, would silently
mangle real source files containing non-ASCII identifiers, comments, or strings. The
on-disk \`grep\` is the unforgiving guard that the exact bytes round-tripped.`,
    async run(env) {
      const path = `${PROJECT}/unicode.txt`;
      const typed = "héllo — 你好 🚀";
      await env.request({ type: "writeFile", path, content: "" });
      await env.request({ type: "openFile", path });
      await sleep(800);

      await env.request({ type: "typeText", text: typed });
      await sleep(600);
      await env.request({ type: "saveAll" }).catch(() => {});
      await sleep(700);

      const buf = (await readFile(env, path)) || "";
      const disk = String(env.exec(`cat ${path}`));
      const hasCJK = buf.includes("你好") || disk.includes("你好");
      const hasEmoji = buf.includes("🚀") || disk.includes("🚀");
      const grepOk = env.exec(`grep -q '🚀' ${path} && echo yes || echo no`) === "yes";

      return {
        pass: hasCJK && hasEmoji && grepOk,
        detail: `CJK present=${hasCJK}; emoji present=${hasEmoji}; on-disk grep 🚀=${grepOk}`,
        evidence: { buffer: buf, disk, typed },
      };
    },
  },

  // ── L1.INPUT.012 — termSend with NO terminal open creates one ─────────────────
  {
    id: "input.termSendCreates",
    specId: "L1.INPUT.012",
    title: "termSend with no terminal open self-provisions one",
    tags: ["input", "terminal"],
    isolation: "fresh",
    needs: ["termSend"],
    rationale: `
WHAT: In a fresh env (verified \`terminalCount == 0\`), fires \`termSend {text:"echo CREATED"}\`
and asserts the env now has exactly one terminal (\`terminalCount == 1\`) and the reply's
\`.terminal\` is a non-empty name.

WHY THIS IS CORRECT: The bridge's \`termSend\` resolves a target via \`findTerminal()\`;
when none exists it CREATES one (\`vscode.window.createTerminal\`) before sending, per the
wire contract "else a freshly created one". So sending into an empty workbench must
bring \`terminalCount\` from 0 to 1 and return that new terminal's name. We assert the
exact 0→1 transition (fresh isolation makes the starting count a clean 0) plus a
non-empty \`.terminal\` so the self-provisioned terminal is genuinely registered.

WHY IT MATTERS: \`termSend\` must never silently DROP input when no terminal is open —
an agent's first command in a fresh env would otherwise vanish. This guards the
self-provisioning branch of \`termSend\` (the empty-state edge of L1.INPUT.009): a
regression where it no-ops or throws on an empty workbench would leave \`terminalCount\`
at 0 and the reply terminal-less, both caught here.`,
    async run(env) {
      const before = await env.observe("input.termSendCreates.before");
      if (before.vscode.terminalCount !== 0) {
        return {
          pass: false,
          detail: `precondition not met: expected 0 terminals, saw ${before.vscode.terminalCount}`,
          evidence: { terminalsBefore: before.vscode.terminals },
        };
      }
      const r = await env.request({ type: "termSend", text: "echo CREATED" });
      const termName = r && (r.terminal ?? r.data?.terminal);
      await sleep(1500);
      const after = await env.observe("input.termSendCreates.after");

      const created = after.vscode.terminalCount === 1;
      const named = typeof termName === "string" && termName.length > 0;
      return {
        pass: created && named,
        detail: `terminalCount 0 → ${after.vscode.terminalCount} (want 1); reply.terminal=${JSON.stringify(termName)}`,
        evidence: { terminalsAfter: after.vscode.terminals, replyTerminal: termName },
      };
    },
  },

  // ── L1.INPUT.013 — termSend honours embedded newlines as multiple commands ────
  {
    id: "input.termSendMultiCmd",
    specId: "L1.INPUT.013",
    title: "termSend with embedded \\n runs multiple commands in sequence",
    tags: ["input", "terminal"],
    isolation: "fresh",
    needs: ["termSend", "fileContent"],
    rationale: `
WHAT: Opens a fresh terminal, sends
\`termSend {text:"echo A > /tmp/fleet-seq.txt\\necho B >> /tmp/fleet-seq.txt"}\`, then
polls \`fileContent {/tmp/fleet-seq.txt}\` until it contains BOTH \`A\` and \`B\` on separate
lines.

WHY THIS IS CORRECT: \`termSend\` writes the text to the pty then appends a trailing
newline (\`sendText(text, true)\`). The embedded \`\\n\` separates the two \`echo\`
statements, so the shell runs them as TWO sequential commands — the first truncates the
file to \`A\`, the second appends \`B\` — and the appended trailing newline runs the last
line. We read the RESULT out-of-band via a file (\`fileContent\`) rather than scraping the
racy terminal output buffer, so the assertion is deterministic: both \`A\` and \`B\` must
be present, on their own lines.

WHY IT MATTERS: Agents send multi-step shell sequences in one \`termSend\` (e.g.
\`cd … && make\` split across lines, or setup + run). A regression that escaped or
collapsed the embedded \`\\n\` (running one mangled command, or only the first line) would
leave the file with only \`A\` (or neither), caught by the two-marker check. It guards
\`termSend\` passing raw newlines through to the pty rather than sanitising them.`,
    async run(env) {
      const out = "/tmp/fleet-seq.txt";
      env.exec(`rm -f ${out}`);
      await env.act("workbench.action.terminal.new");
      await sleep(1800);
      await env.request({
        type: "termSend",
        text: `echo A > ${out}\necho B >> ${out}`,
      });

      const a = await pollFile(env, out, "A", { tries: 14, gap: 500 });
      // After A is present, give B a beat and re-read.
      const text = (await (async () => {
        for (let i = 0; i < 8; i++) {
          await sleep(500);
          const t = (await readFile(env, out)) || "";
          if (t.includes("A") && t.includes("B")) return t;
        }
        return (await readFile(env, out)) || a.text;
      })());

      const hasA = /(^|\n)A(\n|$)/.test(text) || text.split("\n").includes("A");
      const hasB = /(^|\n)B(\n|$)/.test(text) || text.split("\n").includes("B");
      return {
        pass: hasA && hasB,
        detail: `seq file A=${hasA} B=${hasB} (${JSON.stringify(text.trim())})`,
        evidence: { content: text },
      };
    },
  },

  // ── L1.INPUT.014 — terminalText of an empty/unused terminal is honest ─────────
  {
    id: "input.terminalTextEmpty",
    specId: "L1.INPUT.014",
    title: "terminalText of a fresh, unused terminal reports an empty/honest buffer",
    tags: ["input", "terminal"],
    isolation: "fresh",
    needs: ["termSend", "terminalText"],
    rationale: `
WHAT: Opens a fresh terminal (NO command sent — only the shell prompt), reads
\`terminalText {name:<the terminal>}\`, and asserts: the reply is ok, its \`.text\` does
NOT contain a \`$ \`-prefixed command line we never sent (the bridge prefixes our own
\`termSend\` echoes with \`$ \`), and \`.source\` is one of \`{"","captured","buffer"}\`.

WHY THIS IS CORRECT: The bridge maintains a per-terminal capture buffer that it only
appends to when WE \`termSend\` (it records \`"$ <text>\\n"\`) or when shell integration
emits output. Since we never sent anything, the buffer must be empty or contain only
prompt/shell-integration noise — never a fabricated \`$ \`-command we didn't issue. The
\`source\` field exists precisely so callers know the capture state; it must be one of
the documented values. We assert the ABSENCE of an invented command rather than strict
emptiness, because shell-integration may legitimately populate a prompt string.

WHY IT MATTERS: \`terminalText\` must report buffers HONESTLY — returning \`source\` so a
caller knows whether output was actually captured — rather than inventing content. A
regression that seeded the buffer with a phantom command line, or returned a bogus
\`source\`, would mislead every output-scraping caller (and the agent reporters). This is
the empty-state honesty guard for the terminal read path.`,
    async run(env) {
      await env.act("workbench.action.terminal.new");
      await sleep(2000);
      const snap = (await env.observe("input.terminalTextEmpty.snap")).vscode;
      const name = Array.isArray(snap.terminals) ? snap.terminals[0] : undefined;

      const r = await env.request({ type: "terminalText", ...(name ? { name } : {}) });
      const text = field(r, "text");
      const source = field(r, "source");

      const textOk = typeof text === "string";
      // We never sent a command; the bridge prefixes our termSend echoes with "$ ".
      const noPhantomCmd = textOk && !text.includes("$ ");
      const sourceOk = ["", "captured", "buffer"].includes(source);
      return {
        pass: textOk && noPhantomCmd && sourceOk,
        detail: `terminalText source=${JSON.stringify(source)}, no phantom "$ " cmd: ${noPhantomCmd}`,
        evidence: { name, text: String(text).slice(0, 120), source },
      };
    },
  },

  // ── L1.INPUT.015 — termSend of a long command does not block the bridge ───────
  {
    id: "input.termSendNonBlocking",
    specId: "L1.INPUT.015",
    title: "termSend of a long-running command returns promptly and keeps the bridge live",
    tags: ["input", "terminal"],
    isolation: "fresh",
    needs: ["termSend", "terminalText"],
    rationale: `
WHAT: Opens a fresh terminal, then \`termSend {text:"sleep 5 && echo SLEPT_DONE"}\` and
TIMES the reply; immediately after, issues a \`query\` (snapshot) and asserts it also
replies; finally polls \`terminalText\` for \`SLEPT_DONE\`. Pass requires: the \`termSend\`
reply arrived in well under the 5s sleep (< ~2.5s), the interleaved \`query\` succeeded,
and \`SLEPT_DONE\` eventually appears.

WHY THIS IS CORRECT: The bridge's \`termSend\` only WRITES to the pty stdin
(\`sendText\`) and replies immediately — it does NOT await the command's completion. So
the reply must return promptly even though the shell will be busy for 5s, and the
bridge's other handlers (\`query\`) must stay responsive during the sleep. The
\`SLEPT_DONE\` poll afterwards confirms the command really ran to completion in the
background, not that it was dropped.

WHY IT MATTERS: Agents launch long jobs (builds, test suites, servers) through the
terminal; if \`termSend\` awaited completion it would wedge the entire observe/act
channel for the duration, freezing Fleet's view of the env. This guards the
fire-and-write (non-blocking) contract: a regression that awaited the command would
blow the <2.5s reply budget AND likely stall the interleaved \`query\`, both caught here.
The timing budget is generous (2.5s vs a 5s sleep) so normal scheduling jitter doesn't
false-fail it.`,
    async run(env) {
      await env.act("workbench.action.terminal.new");
      await sleep(1800);

      const t0 = Date.now();
      const sent = await env.request({ type: "termSend", text: "sleep 5 && echo SLEPT_DONE" });
      const replyMs = Date.now() - t0;
      const name = sent && (sent.terminal ?? sent.data?.terminal);

      // The bridge must answer a query immediately, while the sleep is still running.
      let queryOk = false;
      try {
        const q = await env.observe("input.termSendNonBlocking.during");
        queryOk = !!q.vscode && typeof q.vscode.terminalCount === "number";
      } catch {
        queryOk = false;
      }

      const done = await pollTerm(env, "SLEPT_DONE", { name, tries: 16, gap: 800 });

      const prompt = replyMs < 2500;
      return {
        pass: prompt && queryOk && done.hit,
        detail:
          `termSend reply ${replyMs}ms (want <2500), query-during=${queryOk}, ` +
          `SLEPT_DONE seen=${done.hit}`,
        evidence: { replyMs, queryOk, terminal: name, tail: done.text.slice(-120) },
      };
    },
  },

  // ── L1.INPUT.018 — repeated typeText accumulates (cursor-advancing insert) ────
  {
    id: "input.typeTextAccumulates",
    specId: "L1.INPUT.018",
    title: "Three typeText 'X' calls accumulate to 'XXX' (additive, not replace)",
    tags: ["input", "editor"],
    isolation: "fresh",
    needs: ["writeFile", "openFile", "typeText", "fileContent", "saveAll"],
    rationale: `
WHAT: Opens an empty \`accum.txt\`, places the cursor at the top (\`cursorTop\`), drives
\`typeText {text:"X"}\` THREE times, saves, and asserts the document content is exactly
\`XXX\` (three X's), not \`X\`.

WHY THIS IS CORRECT: \`typeText\` is a genuine keystroke INSERT at the caret — each
insert places its character and advances the caret past it, so the next insert lands
immediately after. Three single-char inserts therefore accumulate to \`XXX\`. If
\`typeText\` were a set-buffer / replace op (idempotent), the result would be a single
\`X\`; the \`=== "XXX"\` (trimmed) assertion distinguishes the two. cursorTop makes the
starting caret deterministic so the three inserts build a contiguous run.

WHY IT MATTERS: Agents rely on incremental typing accumulating — building up content
across calls. A regression where \`typeText\` overwrote rather than inserted (e.g. reset
the selection/caret each call, or replaced the buffer) would collapse \`XXX\` to \`X\` and
silently destroy multi-step edits. This is the additive-accumulation guard (the repeat
edge of L1.INPUT.001): it proves each call advances the cursor and appends rather than
replaces.`,
    async run(env) {
      const path = `${PROJECT}/fleet-accum.txt`;
      await env.request({ type: "writeFile", path, content: "" });
      await env.request({ type: "openFile", path });
      await sleep(700);
      await env.act("cursorTop").catch(() => {});
      await sleep(300);

      for (let i = 0; i < 3; i++) {
        await env.request({ type: "typeText", text: "X" });
        await sleep(350);
      }
      await env.request({ type: "saveAll" }).catch(() => {});
      await sleep(700);

      const buf = (await readFile(env, path)) || "";
      const disk = env.exec(`cat ${path}`);
      const accumulated = buf.trim() === "XXX" && String(disk).trim() === "XXX";
      return {
        pass: accumulated,
        detail: accumulated
          ? `three typeText "X" → "XXX" (additive)`
          : `expected "XXX", buffer=${JSON.stringify(buf)} disk=${JSON.stringify(String(disk))}`,
        evidence: { buffer: buf, disk: String(disk) },
      };
    },
  },
];
