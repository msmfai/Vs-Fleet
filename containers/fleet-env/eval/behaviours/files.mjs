// Files / editor behaviours (§6 "Files / editor", Track B). Each drives a real
// VS Code action and ASSERTS the effect via the bridge snapshot/queries — never
// "command returned ok". Behaviours needing a not-yet-shipped §3.3 capability
// declare `needs:[...]`; the runner SKIPS them cleanly until Track E ships it.
//
// See behaviours/_contract.mjs for the Behaviour shape, and lib/env.mjs for the
// Env surface (act / observe / request / exec). Patterns copied from the proven
// terminal.new baseline (observe → act → settle → observe → assert + evidence).
//
// Contract assumptions (coded against §3.3 only):
//  - request({type:"openFile",  path})           → {ok}                  ["openFile"]
//  - request({type:"writeFile",  path, content}) → {ok}                  ["writeFile"]
//  - request({type:"saveAll"})                   → {ok}                  ["saveAll"]
//  - request({type:"typeText",   text})          → {ok}                  ["typeText"]
//  - request({type:"fileContent",path})          → {ok, text}            ["fileContent"]
//    The §3.3 query reply spreads its payload onto the result msg
//    ({type:"result",reqId,ok,text}); the no-arg snapshot lands in `.data`. We read
//    BOTH shapes defensively (r.text ?? r.data?.text) so we don't bind to one.
//  - The snapshot (env.observe().vscode) exposes `activeEditor` (path of the active
//    editor) and `openTabs` (labels/paths) per §3.3 Snapshot. We tolerate either
//    full paths or basenames in those arrays.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const PROJECT = "/home/coder/project";

// Pull a §3.3-query payload field whether the bridge spreads it onto the result
// msg (the §3.3 shape) or nests it under `.data` (the snapshot shape).
const field = (r, key) => (r && r[key] !== undefined ? r[key] : r?.data?.[key]);

// Does a snapshot array (openTabs / visibleEditors) reference `path`? Tolerates
// full-path or basename entries, and missing arrays.
const base = (p) => (p ? String(p).split("/").pop() : p);
const refsPath = (arr, path) => {
  if (!Array.isArray(arr)) return false;
  const b = base(path);
  return arr.some((e) => {
    const s = typeof e === "string" ? e : e?.path || e?.label || "";
    return s === path || base(s) === b;
  });
};
const isActive = (snap, path) => {
  const a = snap?.activeEditor;
  return !!a && (a === path || base(a) === base(path));
};

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // file.create* — writeFile a new file then openFile it → it becomes the active
  // editor and its on-disk content matches what we wrote (via fileContent query).
  {
    id: "file.create",
    title: "Create a file (writeFile) and open it → it is the active editor",
    tags: ["files", "editor"],
    isolation: "fresh", // we mutate the workspace fs; don't leak to siblings
    needs: ["writeFile", "openFile", "fileContent"],
    rationale: `
WHAT: Writes a brand-new file (\`fleet-create.txt\`) via the bridge \`writeFile\`
request, then \`openFile\`s it, and asserts TWO independent things: (a) the snapshot's
\`activeEditor\` resolves to that path (basename-tolerant), and (b) a \`fileContent\`
query reads back bytes containing the exact marker "FLEET_CREATE_OK" we wrote. Both
must hold for pass.

WHY THIS IS CORRECT: In VS Code, opening a document that isn't already visible makes
it the active editor of the active group — that is the defined outcome of
\`openFile\`, so \`activeEditor\` pointing at our path is the expected post-state. The
\`fileContent\` round-trip is the orthogonal half: it proves \`writeFile\` actually
flushed our bytes to the workspace fs (not just buffered them in an unsaved model),
because the query reads disk/document content, not our local copy. We assert content
*inclusion* (not equality) so a trailing-newline or BOM normalisation by the editor
doesn't false-fail the byte check.

WHY IT MATTERS: This is the foundational write→open→read loop every other file
behaviour builds on. If a bridge refactor regresses \`writeFile\` (e.g. it starts
creating an unsaved buffer instead of a real file, or silently no-ops on a missing
parent dir) the \`fileContent\` half catches it; if \`openFile\` stops focusing the
document (e.g. opens it in the background or in a non-active group) the \`activeEditor\`
half catches it. Splitting the assertion into two halves means a future reader staring
at a break can immediately tell *which* capability broke from the \`detail\` string
rather than guessing.`,
    async run(env) {
      const path = `${PROJECT}/fleet-create.txt`;
      const content = "FLEET_CREATE_OK\nline two\n";
      const before = await env.observe("file.create.before");

      await env.request({ type: "writeFile", path, content });
      await env.request({ type: "openFile", path });
      await sleep(1500);

      const after = await env.observe("file.create.after");
      const fc = await env.request({ type: "fileContent", path });
      const text = field(fc, "text");

      const active = isActive(after.vscode, path);
      const matches = typeof text === "string" && text.includes("FLEET_CREATE_OK");
      return {
        pass: active && matches,
        detail:
          `activeEditor=${JSON.stringify(after.vscode.activeEditor)} ` +
          `(want ${base(path)}); fileContent ${matches ? "matches" : "MISMATCH"}`,
        evidence: {
          wrote: content,
          fileContent: text,
          activeBefore: before.vscode.activeEditor,
          activeAfter: after.vscode.activeEditor,
        },
      };
    },
  },

  // file.openWelcomeClose — the Welcome tab is open on a fresh workbench; closing
  // the active editor shrinks openTabs. Needs only the baseline {command,query}.
  {
    id: "file.openWelcomeClose",
    title: "Close the active (Welcome) tab → open tab count shrinks",
    tags: ["files", "editor", "smoke"],
    rationale: `
WHAT: On a fresh workbench (where the VS Code Welcome/Get-Started tab is open by
default), captures \`openTabs.length\`, fires the real \`workbench.action.closeActiveEditor\`
command, then asserts the count strictly decreased (afterN < beforeN). If the snapshot
doesn't expose \`openTabs\` at all it reports "not measurable" rather than asserting.

WHY THIS IS CORRECT: \`closeActiveEditor\` closes exactly the focused editor/tab; on a
fresh window the focused tab IS the Welcome tab, so the open-tab set must shrink by one.
We assert a *delta* (after < before) rather than a hard "tabs == N" because the exact
fresh-window tab count is environment-dependent (extensions, restored editors) — the
only invariant we can rely on is "closing removed one". The measurable-guard exists
because this behaviour runs with baseline {command,query} caps only and must degrade
honestly if the snapshot shape changes.

WHY IT MATTERS: This is the smoke test (tagged "smoke") that proves the command channel
round-trips a genuine workbench mutation AND that the snapshot reflects editor lifecycle
changes. If a refactor breaks the bridge's command dispatch, or makes \`openTabs\` stale
(cached and never invalidated on close), this is the cheapest, earliest tripwire — a
break here implies the entire observe→act→observe loop is untrustworthy for every other
files test, so it's deliberately kept dependency-free.`,
    async run(env) {
      const before = await env.observe("file.openWelcomeClose.before");
      const beforeTabs = before.vscode.openTabs;
      const beforeN = Array.isArray(beforeTabs) ? beforeTabs.length : null;

      await env.act("workbench.action.closeActiveEditor");
      await sleep(800);

      const after = await env.observe("file.openWelcomeClose.after");
      const afterTabs = after.vscode.openTabs;
      const afterN = Array.isArray(afterTabs) ? afterTabs.length : null;

      // If the snapshot doesn't expose openTabs we can't assert the effect — say so
      // (still not a hard failure; the runner records pass=false with the reason).
      const measurable = beforeN !== null && afterN !== null;
      return {
        pass: measurable && afterN < beforeN,
        detail: measurable
          ? `openTabs ${beforeN} → ${afterN}`
          : "openTabs not exposed by snapshot — cannot assert",
        evidence: { beforeTabs, afterTabs },
      };
    },
  },

  // editor.splitRight — split the editor group; two editors become visible. We
  // first open a file so there's something to split, then assert the visible /
  // group count grew. Falls back to the snapshot's editor-group signals.
  {
    id: "editor.splitRight",
    title: "Split editor right → two editors visible",
    tags: ["files", "editor"],
    isolation: "fresh",
    needs: ["openFile", "writeFile"],
    rationale: `
WHAT: Seeds and opens \`fleet-split.txt\` so there is a concrete editor to split, counts
\`visibleEditors\`, runs \`workbench.action.splitEditor\`, then asserts the visible-editor
count strictly grew (afterVis > beforeVis).

WHY THIS IS CORRECT: Splitting the active editor in VS Code clones the active document
into a NEW editor group beside the current one — both groups are simultaneously visible,
so \`visibleEditors\` must increase by one. We open a real file first because splitting
an empty workbench with nothing to split is a no-op; with a document present the split is
deterministic. We assert a growth delta rather than "== 2" because the starting visible
count isn't guaranteed (a sibling test or restored layout could leave more than one group
already), and growth is the true invariant of "split".

WHY IT MATTERS: This guards the distinction between *tabs* (logical open documents) and
*editor groups / visible editors* (the on-screen pane layout) — a distinction that is easy
to conflate in a snapshot refactor. If someone "simplifies" the snapshot to report only
\`openTabs\` and derives \`visibleEditors\` incorrectly (splitEditor doesn't open a new tab,
it shows the same doc in a second group, so a tab-count proxy would NOT change), this test
fails and flags that the layout signal has been lost. For a future reader, a break here
almost always means \`visibleEditors\` stopped tracking the group/pane layout.`,
    async run(env) {
      const path = `${PROJECT}/fleet-split.txt`;
      await env.request({ type: "writeFile", path, content: "split me\n" });
      await env.request({ type: "openFile", path });
      await sleep(800);

      const before = await env.observe("editor.splitRight.before");
      const beforeVis = Array.isArray(before.vscode.visibleEditors)
        ? before.vscode.visibleEditors.length
        : null;

      await env.act("workbench.action.splitEditor");
      await sleep(1000);

      const after = await env.observe("editor.splitRight.after");
      const afterVis = Array.isArray(after.vscode.visibleEditors)
        ? after.vscode.visibleEditors.length
        : null;

      const measurable = beforeVis !== null && afterVis !== null;
      return {
        pass: measurable ? afterVis > beforeVis : false,
        detail: measurable
          ? `visibleEditors ${beforeVis} → ${afterVis}`
          : "visibleEditors not exposed by snapshot — cannot assert",
        evidence: {
          before: before.vscode.visibleEditors,
          after: after.vscode.visibleEditors,
        },
      };
    },
  },

  // editor.saveDirty* — open a file, type text into it (making the editor dirty),
  // saveAll, then assert the on-disk bytes (via `exec cat`) contain the typed text.
  {
    id: "editor.saveDirty",
    title: "Type into an editor and saveAll → file on disk reflects the edit",
    tags: ["files", "editor"],
    isolation: "fresh",
    needs: ["typeText", "saveAll", "writeFile", "openFile"],
    rationale: `
WHAT: Seeds an EMPTY \`fleet-save.txt\`, opens it, snapshots the on-disk bytes via
\`exec cat\`, then \`typeText\`s a marker (making the editor dirty), issues \`saveAll\`, and
asserts the on-disk bytes (re-read via \`exec cat\`) now contain that marker. Crucially the
final read goes through the container shell (\`exec\`), NOT the bridge — it inspects real
disk, bypassing any editor-side caching.

WHY THIS IS CORRECT: \`typeText\` mutates the in-memory document model and marks it dirty;
those keystrokes are NOT on disk until a save. \`saveAll\` is the action that flushes every
dirty model to its backing file. So the expected post-state is: disk == (empty seed) before
typing, disk contains marker after saveAll. Reading via \`exec cat\` (independent of the
editor) is deliberate — it proves the *persistence path*, not just that the editor *believes*
it saved. The empty seed makes "before" unambiguous so the marker can't be a stale leftover.

WHY IT MATTERS: This is the dirty→save→persist contract — the difference between an editor
that *looks* saved (no dot on the tab) and bytes that actually hit disk. A regression where
\`typeText\` lands in the wrong/unfocused editor, or where \`saveAll\` fires the command but the
document was never wired as dirty (so VS Code skips the write), would leave disk empty and
trip this test. Because the assertion uses an out-of-band shell read, a future reader can
trust a pass here means genuine durability, and a failure isolates the break to the
type-or-save half (the \`before\`/\`after\` evidence shows which).`,
    async run(env) {
      const path = `${PROJECT}/fleet-save.txt`;
      const marker = "FLEET_SAVED_MARKER";
      // Seed an empty file and open it so typeText lands in a real editor.
      await env.request({ type: "writeFile", path, content: "" });
      await env.request({ type: "openFile", path });
      await sleep(1000);

      const before = env.exec(`cat ${path}`);
      await env.request({ type: "typeText", text: marker });
      await sleep(500);
      await env.request({ type: "saveAll" });
      await sleep(1000);

      const after = env.exec(`cat ${path}`);
      return {
        pass: typeof after === "string" && after.includes(marker),
        detail: `on-disk ${after.includes?.(marker) ? "contains" : "MISSING"} ${marker} (was ${JSON.stringify(before)})`,
        evidence: { diskBefore: before, diskAfter: after, typed: marker },
      };
    },
  },

  // file.rename — rename a file on the fs (via exec) then reload the window; the
  // tab label / openTabs should reflect the new name. Baseline caps only (exec +
  // reload command + query). We open the file first so it has a tab to rename.
  {
    id: "file.rename",
    title: "Rename a file on disk + reload → tab label updates",
    tags: ["files", "editor"],
    isolation: "fresh",
    needs: ["openFile", "writeFile"],
    rationale: `
WHAT: Writes + opens \`fleet-rename-old.txt\`, then renames it ON DISK via \`exec mv\` to
\`fleet-rename-new.txt\`, closes the now-stale editor, and \`openFile\`s the new path. Asserts
both (a) the new path is referenced by the snapshot (\`openTabs\` entry or \`activeEditor\`)
and (b) the new file actually exists on disk (\`test -f\`).

WHY THIS IS CORRECT: A filesystem \`mv\` is an out-of-band change VS Code didn't initiate, so
the already-open editor still points at the vanished old path — it is stale. Rather than do a
full window reload (which would tear down the bridge websocket and orphan the test), we
deliberately close the stale tab and open the new path; the workbench then surfaces the new
basename in its tab set. So "tab references the new name" is the correct observable that the
workbench has caught up to the on-disk reality. The independent \`test -f\` guard ensures we're
asserting against a real rename, not a phantom where \`mv\` failed but a leftover model lingered.

WHY IT MATTERS: This documents and protects a real constraint of the harness — that we model
rename as close-stale + open-new because a hard reload kills the bridge. If a future refactor
"helpfully" switches this to \`workbench.action.reloadWindow\`, the connection drops and the
test hangs/fails, and this rationale tells the reader why the indirect dance is intentional.
It also guards the snapshot's ability to reflect basename changes (the \`refsPath\` tolerance for
full-path vs basename matters here), catching a regression where \`openTabs\` caches old labels.`,
    async run(env) {
      const oldPath = `${PROJECT}/fleet-rename-old.txt`;
      const newPath = `${PROJECT}/fleet-rename-new.txt`;
      await env.request({ type: "writeFile", path: oldPath, content: "rename me\n" });
      await env.request({ type: "openFile", path: oldPath });
      await sleep(800);

      const before = await env.observe("file.rename.before");

      // Rename on disk, then close the stale editor and open the new path so the
      // workbench surfaces the new name (a full reload would drop the bridge conn).
      env.exec(`mv ${oldPath} ${newPath}`);
      await env.act("workbench.action.closeActiveEditor").catch(() => {});
      await env.request({ type: "openFile", path: newPath });
      await sleep(1000);

      const after = await env.observe("file.rename.after");
      const hasNew = refsPath(after.vscode.openTabs, newPath) || isActive(after.vscode, newPath);
      const onDisk = env.exec(`test -f ${newPath} && echo yes || echo no`) === "yes";

      return {
        pass: hasNew && onDisk,
        detail:
          `disk ${onDisk ? "renamed" : "NOT renamed"}; ` +
          `tab references ${base(newPath)}: ${hasNew}`,
        evidence: {
          tabsBefore: before.vscode.openTabs,
          tabsAfter: after.vscode.openTabs,
          activeAfter: after.vscode.activeEditor,
        },
      };
    },
  },

  // quickOpen.byName* — seed a known file, then open it directly (the bridge's
  // openFile is the headless equivalent of Quick Open picking it) and assert it
  // becomes the active editor. (Driving the Quick Open *widget* + typing is a
  // typeText concern; here we assert the navigation outcome the widget produces.)
  {
    id: "quickOpen.byName",
    title: "Quick-open a known file by name → it becomes active",
    tags: ["files", "editor", "quickopen"],
    isolation: "fresh",
    needs: ["openFile"],
    rationale: `
WHAT: Seeds \`fleet-quickopen.txt\` directly via \`exec printf\` (so it exists even where the
\`writeFile\` cap isn't advertised — note \`needs\` lists only \`openFile\`), then \`openFile\`s it
by path and asserts it becomes the \`activeEditor\`.

WHY THIS IS CORRECT: The bridge's \`openFile\` is the headless equivalent of the outcome a user
gets from the Quick Open widget: type a filename, hit enter, that file opens and focuses. We
deliberately assert the *navigation outcome* (the named file becomes active), not the act of
driving the Quick Open UI widget + keystrokes — typing into the widget is a \`typeText\` concern
and would couple this test to fuzzy-match ranking. Opening by exact path isolates the single
invariant that matters: "name resolves to that file becoming the active editor." Seeding via
shell rather than \`writeFile\` keeps the test runnable on the leanest bridge.

WHY IT MATTERS: This is the navigation/resolution contract — proof that a file IDENTIFIED by
name (the user's mental model of Quick Open) ends up focused. It guards against a regression
where \`openFile\` resolves the path but opens it in the background, in the wrong group, or
attaches to a preview editor that doesn't register as \`activeEditor\`. Because it shares the
\`isActive\` basename-tolerant check with \`file.create\`, a break that's isolated to *this* test
(not \`file.create\`) points specifically at the seed/resolution path rather than \`openFile\`
focus mechanics — a useful narrowing signal for whoever is interrogating the failure.`,
    async run(env) {
      const path = `${PROJECT}/fleet-quickopen.txt`;
      // Seed via exec so this works even where writeFile isn't advertised.
      env.exec(`printf 'quick open target\\n' > ${path}`);
      await sleep(300);

      const before = await env.observe("quickOpen.byName.before");
      await env.request({ type: "openFile", path });
      await sleep(1000);

      const after = await env.observe("quickOpen.byName.after");
      const active = isActive(after.vscode, path);
      return {
        pass: active,
        detail: `activeEditor=${JSON.stringify(after.vscode.activeEditor)} (want ${base(path)})`,
        evidence: {
          activeBefore: before.vscode.activeEditor,
          activeAfter: after.vscode.activeEditor,
        },
      };
    },
  },
];
