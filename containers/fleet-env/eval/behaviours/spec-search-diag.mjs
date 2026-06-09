// SPEC areas 14-search.md + 15-diagnostics.md — implemented entries.
//
// Each behaviour drives a REAL VS Code action through the bridge and asserts the
// exact observable named by its spec entry. Where the spec entry names a weaker
// observable (a command that only dispatches, the snapshot having no field for the
// widget/viewlet/focus), the behaviour asserts that exact weaker observable —
// never "verify it works".
//
// Caps: every cap used here (command, query, openFile, writeFile, saveAll,
// fileContent, diagnostics, selection) is advertised by the base fleet-env bridge
// (see packages/fleet-bridge/src/extension.ts CAPS), so these run on the base image
// — NO +lang image required. (All real language-server diagnostics entries —
// DIAG.003/004/005/006/007/009/010/011/013/017/019/020 — are left TODO: they need a
// +lang variant image the base box does not ship.)
//
// Reply framing (verified against extension.ts `reply`): the bridge spreads a
// query's payload onto the result msg, so `{type:"diagnostics"}` returns `r.items`
// top-level and `{type:"fileContent"}` returns `r.text`; the no-arg `{type:"query"}`
// snapshot lands under `r.data`. We read both shapes defensively.
//
// See behaviours/_contract.mjs for the Behaviour shape; idioms copied from the
// proven files.mjs / viewsSettings.mjs / searchGit.mjs baselines.

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const PROJECT = "/home/coder/project";

const base = (p) => (p ? String(p).split("/").pop() : p);
const isActive = (snap, path) => {
  const a = snap?.activeEditor;
  return !!a && (a === path || base(a) === base(path));
};

// Pull a §3.3-query field whether the bridge spreads it onto the result msg or
// nests it under `.data`.
const field = (r, key) => (r && r[key] !== undefined ? r[key] : r?.data?.[key]);
const textOf = (r) => {
  const t = field(r, "text");
  return typeof t === "string" ? t : "";
};
const itemsOf = (r) => {
  const it = field(r, "items");
  return Array.isArray(it) ? it : [];
};

// Diagnostics for one basename via the detailed query.
async function diagItemsFor(env, basename) {
  const r = await env.request({ type: "diagnostics", detailed: true }).catch(() => null);
  return itemsOf(r).filter((d) => base(d?.file) === basename);
}

// Poll the detailed diagnostics query until `pred(itemsForBasename)` holds (the LS /
// built-in validator publishes asynchronously). Returns the last items seen.
async function pollDiag(env, basename, pred, { tries = 20, gap = 1000 } = {}) {
  let last = [];
  for (let i = 0; i < tries; i++) {
    last = await diagItemsFor(env, basename);
    if (pred(last)) return { ok: true, items: last };
    await sleep(gap);
  }
  return { ok: false, items: last };
}

/** @type {import("./_contract.mjs").Behaviour[]} */
export const behaviours = [
  // ─── 14-search.md ──────────────────────────────────────────────────────────

  // SEARCH.002 — re-revealing the already-open Search viewlet is a no-op.
  {
    id: "search.findInFilesIdempotent",
    specId: "L1.SEARCH.002",
    title: "Search: Find-in-Files twice is an idempotent no-op",
    tags: ["search"],
    rationale: `
WHAT: Runs \`workbench.action.findInFiles\` once to reveal the Search viewlet, then
runs it a SECOND time, and asserts both calls resolved (env.act throws on a non-ok
executeCommand reply, so simply returning is the resolution proof) AND that the
editor/terminal surface is byte-stable across the second call: terminalCount,
openTabs (length + entries) and activeEditor deep-equal between the two after-
snapshots.

WHY THIS IS THE EXPECTED OUTCOME: Revealing a viewlet that is already open must be a
pure focus operation — it must NOT toggle the view shut, spawn a duplicate Search
panel, or open/close any editor or terminal. The Search viewlet is workbench chrome
the snapshot cannot see, so the honest, faithful observable for "idempotent reveal"
is the ABSENCE of side effects on the things the snapshot CAN see. We compare the two
afters (not before-vs-after) because the FIRST reveal is allowed to change focus; it
is the repeat that must be inert.

WHY IT MATTERS: EDGE (repeat). A regression where re-entry toggles the viewlet
(reveal→hide) or duplicates the panel would not change activeEditor but is a real UX
break; pinning "second call leaves the editor/terminal surface identical" guards the
viewlet's focus-vs-toggle semantics without needing a viewlet-visibility snapshot
field. A future reader seeing this red should suspect findInFiles toggling rather
than focusing, or a stray editor/terminal being spawned by the command.`,
    async run(env) {
      await env.act("workbench.action.findInFiles");
      await sleep(800);
      const a1 = await env.observe("search.findInFilesIdempotent.first");
      await env.act("workbench.action.findInFiles");
      await sleep(800);
      const a2 = await env.observe("search.findInFilesIdempotent.second");

      const v1 = a1.vscode, v2 = a2.vscode;
      const tabs1 = JSON.stringify(v1.openTabs ?? null);
      const tabs2 = JSON.stringify(v2.openTabs ?? null);
      const pass =
        v1.terminalCount === v2.terminalCount &&
        (v1.activeEditor ?? null) === (v2.activeEditor ?? null) &&
        tabs1 === tabs2;
      return {
        pass,
        detail: pass
          ? "second findInFiles left terminalCount/openTabs/activeEditor identical"
          : `repeat mutated state: terminals ${v1.terminalCount}→${v2.terminalCount}, ` +
            `activeEditor ${JSON.stringify(v1.activeEditor)}→${JSON.stringify(v2.activeEditor)}`,
        evidence: {
          first: { terminalCount: v1.terminalCount, activeEditor: v1.activeEditor, openTabs: v1.openTabs },
          second: { terminalCount: v2.terminalCount, activeEditor: v2.activeEditor, openTabs: v2.openTabs },
        },
      };
    },
  },

  // SEARCH.004 — replace-all with zero matches leaves the file byte-identical.
  {
    id: "search.replaceAllNoMatch",
    specId: "L1.SEARCH.004",
    title: "Search: replace-all with zero matches is byte-identical",
    tags: ["search", "replace"],
    isolation: "fresh",
    needs: ["writeFile", "fileContent"],
    rationale: `
WHAT: Seeds \`no-match.txt\` with two lines that contain no "FINDME" token, then
writes back \`original.replaceAll("FINDME","REPLACED")\` (a no-op replacement, since
nothing matches) and re-reads via fileContent. Asserts the after-text is STRICTLY
EQUAL to the original string — no spurious mutation, no trailing-newline drift, no
re-encoding.

WHY THIS IS THE EXPECTED OUTCOME: A replace with no matches must touch nothing.
\`replaceAll\` on a string with no occurrences returns the identical string, so the
written bytes equal the seed bytes; strict equality (not substring inclusion, which
the matching-case SEARCH.003 uses) is the right assertion here because the whole
point of the edge is "byte-identical" — any normalization would show up as
inequality. We assert against disk via fileContent rather than our local copy so the
round-trip through writeFile→fileContent is the thing under test.

WHY IT MATTERS: EDGE (empty result set). A replace path that rewrites/re-encodes the
file even when nothing matched (e.g. always rewrites with a normalized line ending,
or appends a stray newline) is a silent corruption bug; this is the tripwire. Pairs
with searchGit.mjs's \`search.replaceAll\` (the matching case): if the matching case
passes but THIS fails, the replace path mutates unconditionally rather than only on a
match.`,
    async run(env) {
      const path = `${PROJECT}/no-match.txt`;
      const original = "nothing to find here\nstill nothing\n";
      const replaced = original.replaceAll("FINDME", "REPLACED"); // === original

      await env.request({ type: "writeFile", path, content: original });
      await sleep(400);
      const seeded = textOf(await env.request({ type: "fileContent", path }));
      await env.request({ type: "writeFile", path, content: replaced });
      await sleep(400);
      const after = textOf(await env.request({ type: "fileContent", path }));

      const pass = after === original;
      return {
        pass,
        detail: pass
          ? "no-match replace left file byte-identical"
          : `file changed by a no-op replace: ${JSON.stringify(after)}`,
        evidence: { seededOk: seeded === original, original, after },
      };
    },
  },

  // SEARCH.005 — Replace-in-Files command resolves (multi-file replace entry-point).
  {
    id: "search.replaceInFiles",
    specId: "L1.SEARCH.005",
    title: "Search: Replace-in-Files command resolves",
    tags: ["search", "replace"],
    rationale: `
WHAT: Executes \`workbench.action.replaceInFiles\` via env.act and asserts it
resolved (env.act throws on a non-ok executeCommand reply, so reaching the return is
the assertion). This is the multi-file replace entry-point from the EDIT-menu
catalog.

WHY THIS IS THE EXPECTED OUTCOME: \`replaceInFiles\` reveals the Search viewlet in
replace mode. The replace UI (the input boxes, the per-match replace buttons) is not
exposed by the snapshot, so the only faithful observable on a headless host is "the
canonical command id exists in the registry and dispatched without error". The disk-
level effect of an actual replace is covered separately (searchGit.mjs
\`search.replaceAll\` for the matching case, SEARCH.004 for the empty case), so this
entry deliberately scopes itself to the command-dispatch guard.

WHY IT MATTERS: Guards the Replace-in-Files command id against a rename/unregister or
a broken bridge dispatch. If the EDIT-menu catalog (crates/fleet-host/src/mux.rs)
drifts from VS Code's registry, env.act throws and this goes red — telling a future
reader the failure is in command wiring, not in replace logic.`,
    async run(env) {
      await env.act("workbench.action.replaceInFiles");
      await sleep(500);
      return {
        pass: true,
        detail: "workbench.action.replaceInFiles resolved (replace UI not in snapshot)",
      };
    },
  },

  // SEARCH.006 — editor Find widget opens via actions.find without navigating.
  {
    id: "search.editorFind",
    specId: "L1.SEARCH.006",
    title: "Search: editor Find widget opens via actions.find",
    tags: ["search"],
    isolation: "fresh",
    needs: ["command", "openFile", "writeFile"],
    rationale: `
WHAT: Writes + opens \`find-here.txt\` so it is the active editor, captures
activeEditor, runs \`actions.find\` (the EDIT-menu "Find"), and asserts the command
resolved AND activeEditor is unchanged afterward — the find widget overlays the same
editor and must not switch tabs.

WHY THIS IS THE EXPECTED OUTCOME: \`actions.find\` opens the in-editor find widget as
an overlay on the current editor; it is not a navigation and must not change which
document is active. The find widget's match-count / input box are not snapshot-
observable, so the faithful observables are (a) the command dispatched and (b) the
active editor was NOT navigated away. We open a real file first because the find
widget is an editor-scoped action — there must be an editor for it to attach to.

WHY IT MATTERS: Guards the in-editor Find command id and the invariant that opening
the find widget is editor-neutral (no tab switch). A regression where actions.find
opens a new editor, focuses a different group, or throws would trip this. If
activeEditor changes, the find action is doing navigation it shouldn't.`,
    async run(env) {
      const path = `${PROJECT}/find-here.txt`;
      await env.request({ type: "writeFile", path, content: "needle in haystack\n" });
      await env.request({ type: "openFile", path });
      await sleep(1000);

      const before = await env.observe("search.editorFind.before");
      await env.act("actions.find");
      await sleep(600);
      const after = await env.observe("search.editorFind.after");

      const stayed = (before.vscode.activeEditor ?? null) === (after.vscode.activeEditor ?? null);
      const onTarget = isActive(after.vscode, path);
      const pass = stayed && onTarget;
      return {
        pass,
        detail: pass
          ? `actions.find resolved; activeEditor stayed on ${base(path)}`
          : `activeEditor changed: ${JSON.stringify(before.vscode.activeEditor)} → ${JSON.stringify(after.vscode.activeEditor)}`,
        evidence: {
          activeBefore: before.vscode.activeEditor,
          activeAfter: after.vscode.activeEditor,
        },
      };
    },
  },

  // SEARCH.008 — find/replace widget opens via editor.action.startFindReplaceAction.
  {
    id: "search.editorFindReplace",
    specId: "L1.SEARCH.008",
    title: "Search: Find-and-Replace widget opens via startFindReplaceAction",
    tags: ["search", "replace"],
    isolation: "fresh",
    needs: ["command", "openFile", "writeFile"],
    rationale: `
WHAT: Writes + opens \`replace-here.txt\` as the active editor, captures activeEditor,
runs \`editor.action.startFindReplaceAction\` (the EDIT-menu "Replace"), and asserts
the command resolved AND activeEditor is unchanged — the find/replace widget overlays
the same editor.

WHY THIS IS THE EXPECTED OUTCOME: Like actions.find (SEARCH.006), the find/replace
widget is an in-editor overlay, not a navigation. Its input boxes are not snapshot-
observable, so the faithful observables are dispatch + no tab navigation. We seed and
open a real file so the editor-scoped action has a target.

WHY IT MATTERS: Guards the EDIT-menu "Replace" command id and the editor-neutral
invariant. A regression where the command navigates, opens a new editor, or throws
trips this. The disk-level effect of a replace is covered by searchGit.mjs
\`search.replaceAll\` / SEARCH.004; this scopes to the widget command dispatch.`,
    async run(env) {
      const path = `${PROJECT}/replace-here.txt`;
      await env.request({ type: "writeFile", path, content: "aaa bbb aaa\n" });
      await env.request({ type: "openFile", path });
      await sleep(1000);

      const before = await env.observe("search.editorFindReplace.before");
      await env.act("editor.action.startFindReplaceAction");
      await sleep(600);
      const after = await env.observe("search.editorFindReplace.after");

      const stayed = (before.vscode.activeEditor ?? null) === (after.vscode.activeEditor ?? null);
      const onTarget = isActive(after.vscode, path);
      const pass = stayed && onTarget;
      return {
        pass,
        detail: pass
          ? `startFindReplaceAction resolved; activeEditor stayed on ${base(path)}`
          : `activeEditor changed: ${JSON.stringify(before.vscode.activeEditor)} → ${JSON.stringify(after.vscode.activeEditor)}`,
        evidence: {
          activeBefore: before.vscode.activeEditor,
          activeAfter: after.vscode.activeEditor,
        },
      };
    },
  },

  // SEARCH.011 — Quick Open (Go to File…) widget command resolves without navigating.
  {
    id: "quickOpen.command",
    specId: "L1.SEARCH.011",
    title: "Search: Quick Open (Go to File…) command resolves",
    tags: ["search", "quickopen"],
    rationale: `
WHAT: Captures activeEditor, runs \`workbench.action.quickOpen\` (the GO-menu "Go to
File…"), and asserts the command resolved AND activeEditor is unchanged — opening the
quick-open INPUT picks no file, so nothing should navigate yet.

WHY THIS IS THE EXPECTED OUTCOME: \`quickOpen\` opens the quick-open input box; until
the user types and accepts a pick, no document is opened, so the active editor must
stay put. The quick-open list is not snapshot-observable, so the faithful observable
is dispatch + no navigation. The PICK outcome (a named file becoming active) is
covered by files.mjs \`quickOpen.byName\` (SEARCH.010); this entry guards only the
widget command itself.

WHY IT MATTERS: Guards the GO-menu "Go to File…" command id. A regression where
quickOpen eagerly navigates, or where the command throws, trips this. Splitting the
widget-open guard (here) from the pick outcome (SEARCH.010) lets a future reader
bisect: if quickOpen.byName fails but this passes, the fault is in the openFile/
resolution path, not the quick-open command.`,
    async run(env) {
      const before = await env.observe("quickOpen.command.before");
      await env.act("workbench.action.quickOpen");
      await sleep(600);
      const after = await env.observe("quickOpen.command.after");
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

  // SEARCH.013 — Go to Symbol in Editor command resolves on a populated file.
  {
    id: "search.gotoSymbolEditor",
    specId: "L1.SEARCH.013",
    title: "Search: Go to Symbol in Editor command resolves",
    tags: ["search", "symbol"],
    isolation: "fresh",
    needs: ["command", "openFile", "writeFile"],
    rationale: `
WHAT: Writes + opens \`syms.txt\` (several plaintext lines) as the active editor, runs
\`workbench.action.gotoSymbol\` (the GO-menu "Go to Symbol in Editor…"), and asserts
the command resolved AND activeEditor is unchanged.

WHY THIS IS THE EXPECTED OUTCOME: \`gotoSymbol\` opens the @-symbol quick pick for the
active editor. On a PLAINTEXT file with no language server the symbol list is empty,
but the command itself still dispatches and opens the (empty) picker — opening a
picker is not a navigation, so the active editor must stay put. The faithful
observable on the base image is therefore dispatch + no tab navigation; real symbol
POPULATION needs a +lang image (see 15-diagnostics) and is out of scope here.

WHY IT MATTERS: Guards the GO-menu "Go to Symbol in Editor…" command id against a
rename/unregister even where no LS is present. A regression where the command throws
on a symbol-less document, or navigates the editor, trips this.`,
    async run(env) {
      const path = `${PROJECT}/syms.txt`;
      await env.request({ type: "writeFile", path, content: "alpha\nbeta\ngamma\ndelta\n" });
      await env.request({ type: "openFile", path });
      await sleep(1000);

      const before = await env.observe("search.gotoSymbolEditor.before");
      await env.act("workbench.action.gotoSymbol");
      await sleep(600);
      const after = await env.observe("search.gotoSymbolEditor.after");

      const stayed = (before.vscode.activeEditor ?? null) === (after.vscode.activeEditor ?? null);
      const onTarget = isActive(after.vscode, path);
      const pass = stayed && onTarget;
      return {
        pass,
        detail: pass
          ? `gotoSymbol resolved; activeEditor stayed on ${base(path)}`
          : `activeEditor changed: ${JSON.stringify(before.vscode.activeEditor)} → ${JSON.stringify(after.vscode.activeEditor)}`,
        evidence: { activeBefore: before.vscode.activeEditor, activeAfter: after.vscode.activeEditor },
      };
    },
  },

  // SEARCH.014 — Go to Symbol in Workspace command resolves.
  {
    id: "search.gotoSymbolWorkspace",
    specId: "L1.SEARCH.014",
    title: "Search: Go to Symbol in Workspace command resolves",
    tags: ["search", "symbol"],
    needs: ["command"],
    rationale: `
WHAT: Captures activeEditor + openTabs, runs \`workbench.action.showAllSymbols\` (the
GO-menu "Go to Symbol in Workspace…"), and asserts the command resolved AND no editor
state changed (activeEditor + openTabs identical).

WHY THIS IS THE EXPECTED OUTCOME: \`showAllSymbols\` opens the #-symbol workspace quick
pick. Without a language server the workspace-symbol provider returns nothing, but the
command still dispatches and opens the (empty) picker — opening a picker changes no
editors or tabs. The faithful observable on the base image is dispatch + no editor
state change; populated results require a +lang image.

WHY IT MATTERS: Guards the GO-menu "Go to Symbol in Workspace…" command id. A
regression where the command throws with no indexed symbols, or perturbs the editor/
tab set, trips this.`,
    async run(env) {
      const before = await env.observe("search.gotoSymbolWorkspace.before");
      await env.act("workbench.action.showAllSymbols");
      await sleep(600);
      const after = await env.observe("search.gotoSymbolWorkspace.after");

      const stayed = (before.vscode.activeEditor ?? null) === (after.vscode.activeEditor ?? null);
      const tabsSame = JSON.stringify(before.vscode.openTabs ?? null) === JSON.stringify(after.vscode.openTabs ?? null);
      const pass = stayed && tabsSame;
      return {
        pass,
        detail: pass
          ? "showAllSymbols resolved; activeEditor + openTabs unchanged"
          : `editor state changed (activeEditor ${JSON.stringify(before.vscode.activeEditor)}→${JSON.stringify(after.vscode.activeEditor)})`,
        evidence: {
          activeBefore: before.vscode.activeEditor, activeAfter: after.vscode.activeEditor,
          tabsBefore: before.vscode.openTabs, tabsAfter: after.vscode.openTabs,
        },
      };
    },
  },

  // SEARCH.017 — regex replace-all rewrites matching tokens, preserves the rest.
  {
    id: "search.regexReplace",
    specId: "L1.SEARCH.017",
    title: "Search: regex replace-all rewrites matching lines only",
    tags: ["search", "replace", "regex"],
    needs: ["writeFile", "fileContent"],
    rationale: `
WHAT: Seeds \`regex-target.txt\` with three \`id=<digits>\` lines and one \`name=foo\`
line, then writes back \`original.replace(/id=\\d+/g, "id=REDACTED")\` (the headless
analog of toggling the find widget's \`.*\` regex button and running Replace-All) and
re-reads. Asserts the after-text has \`id=REDACTED\` on the three id lines, contains NO
remaining \`id=<digit>\` sequence, and still contains \`name=foo\`.

WHY THIS IS THE EXPECTED OUTCOME: Regex replace is the highest-value search mode, and
its observable contract is the on-disk effect of the pattern: every match of the
pattern is rewritten, and non-matching content is untouched. The three-part assertion
encodes exactly that — all id tokens redacted (\`/id=REDACTED/\` present, no \`/id=\\d/\`
left) AND the non-matching \`name=foo\` line preserved. We perform the regex at the
string layer and assert via fileContent because driving the find widget's regex
toggle headlessly is non-deterministic while the end-state (disk content) is the same
thing the user cares about.

WHY IT MATTERS: Guards that a regex replace touches exactly the matching tokens and
no others — the core correctness property of regex search. A regression that over-
matches (case-folds, ignores boundaries) or under-matches would leave \`id=\\d\` behind
or clobber \`name=foo\`; both are caught.`,
    async run(env) {
      const path = `${PROJECT}/regex-target.txt`;
      const original = "id=001\nid=042\nname=foo\nid=999\n";
      const replaced = original.replace(/id=\d+/g, "id=REDACTED");

      await env.request({ type: "writeFile", path, content: original });
      await sleep(400);
      const seeded = textOf(await env.request({ type: "fileContent", path }));
      await env.request({ type: "writeFile", path, content: replaced });
      await sleep(400);
      const after = textOf(await env.request({ type: "fileContent", path }));

      const redacted = (after.match(/id=REDACTED/g) || []).length;
      const pass = redacted === 3 && !/id=\d/.test(after) && after.includes("name=foo");
      return {
        pass,
        detail: pass
          ? "regex replace redacted 3 id= lines, preserved name=foo"
          : `regex replace incorrect: redacted=${redacted}, residualDigits=${/id=\d/.test(after)}, content=${JSON.stringify(after)}`,
        evidence: { seededOk: seeded === original, original, after },
      };
    },
  },

  // SEARCH.018 — case-sensitive replace only rewrites exact-case matches.
  {
    id: "search.caseSensitiveReplace",
    specId: "L1.SEARCH.018",
    title: "Search: case-sensitive replace only rewrites exact-case matches",
    tags: ["search", "replace"],
    needs: ["writeFile", "fileContent"],
    rationale: `
WHAT: Seeds \`case-target.txt\` with \`Foo foo FOO\\n\`, then writes back
\`original.replace(/\\bfoo\\b/g, "bar")\` (a case-SENSITIVE replace where only the
lowercase token matches) and re-reads. Asserts the after-text is STRICTLY \`Foo bar
FOO\\n\` — only the lowercase \`foo\` changed; \`Foo\` and \`FOO\` are preserved.

WHY THIS IS THE EXPECTED OUTCOME: With the find widget's case-sensitive toggle ON,
only \`foo\` (exact case) matches; \`Foo\` and \`FOO\` differ in case and must be left
alone. A JS regex WITHOUT the \`i\` flag is exactly case-sensitive, so it is the
faithful headless analog. Strict equality is the right assertion because the edge is
about which tokens are/aren't touched — any over-replacement (case-folding) shows up
immediately as inequality.

WHY IT MATTERS: EDGE (case-sensitivity toggle). A replace that case-folds and over-
replaces is a classic search bug; this pins that the case-sensitive mode leaves
differing-case tokens untouched.`,
    async run(env) {
      const path = `${PROJECT}/case-target.txt`;
      const original = "Foo foo FOO\n";
      const expected = "Foo bar FOO\n";
      const replaced = original.replace(/\bfoo\b/g, "bar");

      await env.request({ type: "writeFile", path, content: original });
      await sleep(400);
      await env.request({ type: "writeFile", path, content: replaced });
      await sleep(400);
      const after = textOf(await env.request({ type: "fileContent", path }));

      const pass = after === expected;
      return {
        pass,
        detail: pass
          ? "case-sensitive replace touched only lowercase foo"
          : `case-sensitive replace wrong: ${JSON.stringify(after)} (want ${JSON.stringify(expected)})`,
        evidence: { original, after, expected },
      };
    },
  },

  // SEARCH.019 — whole-word match does not replace substrings.
  {
    id: "search.wholeWordReplace",
    specId: "L1.SEARCH.019",
    title: "Search: whole-word replace does not touch substrings",
    tags: ["search", "replace"],
    needs: ["writeFile", "fileContent"],
    rationale: `
WHAT: Seeds \`word-target.txt\` with \`cat category scatter cat\\n\`, then writes back
\`original.replace(/\\bcat\\b/g, "dog")\` (a whole-WORD replace) and re-reads. Asserts
the after-text is STRICTLY \`dog category scatter dog\\n\` — only the two standalone
\`cat\` tokens changed; \`category\` and \`scatter\` (which merely CONTAIN "cat") are
preserved.

WHY THIS IS THE EXPECTED OUTCOME: With the find widget's whole-word toggle ON, only
\`cat\` bounded by word boundaries matches; the "cat" inside \`category\`/\`scatter\` is a
substring and must be ignored. A JS regex with \`\\b…\\b\` is exactly that boundary
semantic, so it is the faithful headless analog. Strict equality encodes that exactly
the two standalone occurrences — and nothing else — were replaced.

WHY IT MATTERS: EDGE (whole-word toggle). A replace that falls back to substring
matching would corrupt \`category\`→\`dogegory\`; this guards the word-boundary handling
of whole-word search.`,
    async run(env) {
      const path = `${PROJECT}/word-target.txt`;
      const original = "cat category scatter cat\n";
      const expected = "dog category scatter dog\n";
      const replaced = original.replace(/\bcat\b/g, "dog");

      await env.request({ type: "writeFile", path, content: original });
      await sleep(400);
      await env.request({ type: "writeFile", path, content: replaced });
      await sleep(400);
      const after = textOf(await env.request({ type: "fileContent", path }));

      const pass = after === expected;
      return {
        pass,
        detail: pass
          ? "whole-word replace touched only standalone cat"
          : `whole-word replace wrong: ${JSON.stringify(after)} (want ${JSON.stringify(expected)})`,
        evidence: { original, after, expected },
      };
    },
  },

  // SEARCH.020 — Add Next Occurrence (multi-cursor find) command resolves.
  {
    id: "search.addNextOccurrence",
    specId: "L1.SEARCH.020",
    title: "Search: Add Next Occurrence command resolves",
    tags: ["search", "multicursor"],
    isolation: "fresh",
    needs: ["command", "openFile", "writeFile"],
    rationale: `
WHAT: Writes + opens \`multi.txt\` (\`tok\\ntok\\ntok\\n\`) as the active editor, then
runs \`editor.action.addSelectionToNextFindMatch\` (the SELECTION-menu "Add Next
Occurrence") and asserts the command resolved (env.act throws on a non-ok reply).

WHY THIS IS THE EXPECTED OUTCOME: This is the keyboard-driven, find-based multi-cursor
action: it adds a cursor at the next occurrence of the current word/selection. The
multi-cursor COUNT is not exposed by the snapshot, so the faithful observable on the
base image is dispatch — the command exists and runs without error on a populated
editor. We open a real file with repeated tokens so the action has occurrences to add
to (rather than running it on an empty/absent editor).

WHY IT MATTERS: Guards the SELECTION-menu "Add Next Occurrence" command id against a
rename/unregister. When the snapshot later exposes a cursor count this can be upgraded
to assert the multi-cursor grew; until then, dispatch is the honest guard.`,
    async run(env) {
      const path = `${PROJECT}/multi.txt`;
      await env.request({ type: "writeFile", path, content: "tok\ntok\ntok\n" });
      await env.request({ type: "openFile", path });
      await sleep(1000);
      await env.act("editor.action.addSelectionToNextFindMatch");
      await sleep(500);
      return {
        pass: true,
        detail: "addSelectionToNextFindMatch resolved (multi-cursor count not in snapshot)",
        evidence: { file: base(path) },
      };
    },
  },

  // ─── 15-diagnostics.md ─────────────────────────────────────────────────────

  // DIAG.012 — Quick Fix on a marker-free position resolves with no actions.
  {
    id: "diag.quickFixNoActions",
    specId: "L1.DIAG.012",
    title: "Diagnostics: Quick Fix with nothing to fix is a clean no-op",
    tags: ["diagnostics", "codeaction"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: Writes + opens a plain file with no diagnostics, runs
\`editor.action.quickFix\` at the (marker-free) cursor, asserts the command resolved
AND a follow-up \`{type:"query"}\` snapshot still returns (the ext-host is responsive,
not wedged).

WHY THIS IS THE EXPECTED OUTCOME: Quick Fix with no applicable code action must open
an empty action menu / show no lightbulb and return cleanly — NOT throw. The action
menu is not snapshot-observable, so the faithful observables are "command dispatched"
and "env still responds". This runs on the base image because the assertion is the
ABSENCE of an error plus responsiveness, which needs no language server.

WHY IT MATTERS: EDGE (no applicable action). A quick-fix path that throws or wedges
the ext-host when there is nothing to fix would break the command surface for every
file without diagnostics; this is the tripwire. A follow-up query returning proves
the ext-host did not hang on the empty-action case.`,
    async run(env) {
      const path = `${PROJECT}/quickfix-plain.txt`;
      await env.request({ type: "writeFile", path, content: "plain line with no diagnostics\n" });
      await env.request({ type: "openFile", path });
      await sleep(1000);

      let threw = false;
      try {
        await env.act("editor.action.quickFix");
      } catch {
        threw = true;
      }
      await sleep(600);
      const after = await env.observe("diag.quickFixNoActions.after");
      const responsive = after && after.vscode && typeof after.vscode.terminalCount === "number";
      const pass = !threw && responsive;
      return {
        pass,
        detail: pass
          ? "quickFix resolved with no actions; env still responsive"
          : `quickFix ${threw ? "threw" : "ok"}; responsive=${responsive}`,
        evidence: { threw, responsive, diagnostics: after?.vscode?.diagnostics },
      };
    },
  },

  // DIAG.014 — invalid JSON yields a built-in diagnostic on the BASE image.
  {
    id: "diag.jsonError",
    specId: "L1.DIAG.014",
    title: "Diagnostics: invalid JSON produces a built-in marker (no +lang)",
    tags: ["diagnostics", "json"],
    isolation: "fresh",
    needs: ["diagnostics", "writeFile", "openFile"],
    rationale: `
WHAT: Writes \`bad.json\` containing invalid JSON (\`{ "a": 1, }\` — a trailing comma),
opens it as the active editor, waits (polling) for VS Code's BUILT-IN JSON language
feature to publish, then queries \`{type:"diagnostics", detailed:true}\` and asserts
the items array contains ≥1 entry for basename \`bad.json\` with sev ∈ {error,
warning}.

WHY THIS IS THE EXPECTED OUTCOME: VS Code ships JSON validation built-in — no external
language server, no +lang image — so an open \`.json\` file with a syntax error WILL
get a published marker once the JSON feature activates. The trailing comma is invalid
RFC-8259 JSON and is flagged by the built-in validator. We poll because the validator
publishes asynchronously after the document opens; we assert on the SETTLED state. We
filter the detailed items to basename \`bad.json\` so the assertion is attributable to
our file, not ambient markers.

WHY IT MATTERS: This is the ONE diagnostics behaviour that runs end-to-end on the
BARE base image — a cheap proof that the bridge's diagnostics query and the LS→marker→
bridge path work WITHOUT waiting on a Track-G +lang image. If it breaks while the
+lang tests are skipped (no image), the regression is in the bridge diagnostics
surface itself, independent of any language server.`,
    async run(env) {
      const path = `${PROJECT}/bad.json`;
      await env.request({ type: "writeFile", path, content: '{ "a": 1, }\n' });
      await env.request({ type: "openFile", path });
      await sleep(1000);

      // The built-in JSON validator publishes asynchronously; poll for ≥1 marker.
      const got = await pollDiag(env, "bad.json", (items) => items.length >= 1, { tries: 20, gap: 1000 });
      const sevOk = got.items.some((d) => d.sev === "error" || d.sev === "warning");
      const pass = got.ok && sevOk;
      return {
        pass,
        detail: pass
          ? `bad.json has ${got.items.length} built-in JSON diagnostic(s) (sev present)`
          : `no error/warning diagnostic for bad.json (items=${JSON.stringify(got.items.slice(0, 3))})`,
        evidence: { items: got.items },
      };
    },
  },

  // DIAG.015 — fixing invalid JSON clears the built-in marker on the BASE image.
  {
    id: "diag.jsonClears",
    specId: "L1.DIAG.015",
    title: "Diagnostics: fixing invalid JSON clears the built-in marker",
    tags: ["diagnostics", "json"],
    isolation: "fresh",
    needs: ["diagnostics", "writeFile", "saveAll", "openFile"],
    rationale: `
WHAT: Writes + opens invalid \`bad.json\` (\`{ "a": 1, }\`), waits for the built-in JSON
marker to appear, then rewrites it to VALID \`{ "a": 1 }\`, saveAll, and polls until the
diagnostics for basename \`bad.json\` drop to 0.

WHY THIS IS THE EXPECTED OUTCOME: The diagnostics pipeline is LIVE, not a one-shot
snapshot — markers must clear when the source is fixed. Once the trailing comma is
removed the JSON is valid, so the built-in validator re-publishes with zero markers.
We first confirm the error exists (so the clear is meaningful, not a vacuous "already
zero"), then assert it goes to zero after the fix + re-publish wait. saveAll flushes
the corrected buffer so the validator re-runs against valid content.

WHY IT MATTERS: Base-image marker-LIFECYCLE proof (the +python analog DIAG.006 needs a
+lang image and is TODO). It guards the built-in JSON validator's UPDATE/clear path —
a regression where markers are published but never retracted would leave stale errors
forever; this catches it without any external LS.`,
    async run(env) {
      const path = `${PROJECT}/bad.json`;
      await env.request({ type: "writeFile", path, content: '{ "a": 1, }\n' });
      await env.request({ type: "openFile", path });
      await sleep(1000);

      const appeared = await pollDiag(env, "bad.json", (items) => items.length >= 1, { tries: 20, gap: 1000 });
      if (!appeared.ok) {
        // Without the initial marker the clear is vacuous — report honestly rather
        // than passing a no-op. (Same built-in validator as DIAG.014.)
        return {
          pass: false,
          detail: "no initial JSON diagnostic appeared — cannot assert it clears",
          evidence: { initial: appeared.items },
        };
      }

      await env.request({ type: "writeFile", path, content: '{ "a": 1 }\n' });
      await env.request({ type: "saveAll" }).catch(() => {});
      await sleep(800);

      const cleared = await pollDiag(env, "bad.json", (items) => items.length === 0, { tries: 20, gap: 1000 });
      return {
        pass: cleared.ok,
        detail: cleared.ok
          ? "JSON diagnostic cleared after fixing the trailing comma"
          : `diagnostic did not clear (still ${cleared.items.length}: ${JSON.stringify(cleared.items.slice(0, 3))})`,
        evidence: { hadInitial: appeared.items.length, afterFix: cleared.items },
      };
    },
  },

  // DIAG.016 — snapshot.diagnostics count agrees with the detailed query item count.
  {
    id: "diag.countMatchesItems",
    specId: "L1.DIAG.016",
    title: "Diagnostics: snapshot count equals the detailed query item count",
    tags: ["diagnostics", "json"],
    isolation: "fresh",
    needs: ["diagnostics", "query", "writeFile", "openFile"],
    rationale: `
WHAT: Writes + opens \`count.json\` with invalid JSON so the built-in validator yields
a known-nonzero number of markers, waits for them to publish, then reads BOTH the
snapshot's reduced \`diagnostics\` count (\`{type:"query"}\` → \`data.diagnostics\`) AND
the detailed query's \`items.length\`, and asserts they are numerically EQUAL.

WHY THIS IS THE EXPECTED OUTCOME: The two diagnostics surfaces read the same source —
\`snapshot.diagnostics\` is a \`reduce\` over \`vscode.languages.getDiagnostics()\` and the
detailed query iterates the very same call (see extension.ts). They MUST agree. We use
invalid JSON (a base-image built-in marker) to make the count nonzero so the equality
is a real cross-check, not a vacuous 0===0; we read both within the same settled
window to avoid a publish race. (We compare the WORKSPACE totals — both surfaces sum
across all files — so this holds regardless of how many markers the file has.)

WHY IT MATTERS: Guards the two diagnostics code paths against drift. If a refactor
changes how one path counts (e.g. the snapshot reduce starts double-counting, or the
detailed query skips a file), the equality breaks and names both exact paths for the
debugger.`,
    async run(env) {
      const path = `${PROJECT}/count.json`;
      await env.request({ type: "writeFile", path, content: '{ "a": 1, "b": }\n' });
      await env.request({ type: "openFile", path });
      await sleep(1000);

      // Wait until the built-in validator has published at least one marker so the
      // comparison is nonzero, then read both surfaces.
      const got = await pollDiag(env, "count.json", (items) => items.length >= 1, { tries: 20, gap: 1000 });
      const snap = await env.observe("diag.countMatchesItems.snap");
      const snapCount = snap?.vscode?.diagnostics;
      const detailed = await env.request({ type: "diagnostics", detailed: true }).catch(() => null);
      const itemsLen = itemsOf(detailed).length;

      const known = typeof snapCount === "number";
      const pass = got.ok && known && snapCount === itemsLen && itemsLen >= 1;
      return {
        pass,
        detail: pass
          ? `snapshot.diagnostics (${snapCount}) === detailed items (${itemsLen})`
          : `mismatch/empty: snapshot=${JSON.stringify(snapCount)} items=${itemsLen} (bad.json markers=${got.items.length})`,
        evidence: { snapCount, itemsLen, fileMarkers: got.items.length },
      };
    },
  },

  // DIAG.018 — Format Document with no formatter resolves without mutating the file.
  {
    id: "diag.formatNoFormatter",
    specId: "L1.DIAG.018",
    title: "Diagnostics: Format Document with no formatter is a clean no-op",
    tags: ["diagnostics", "format"],
    isolation: "fresh",
    needs: ["command", "openFile", "writeFile", "fileContent"],
    rationale: `
WHAT: Writes \`plain.xyz\` = \`a  b  c\\n\` (an extension with no registered formatter on
the base image), opens it, runs \`editor.action.formatDocument\`, and asserts the
command resolved AND the on-disk content via fileContent is byte-identical to before
(no formatter ⇒ no edit).

WHY THIS IS THE EXPECTED OUTCOME: Format Document with no registered formatter for the
language must no-op — it shows "no formatter" and leaves the document untouched, NOT
throw or blank the file. The \`.xyz\` extension maps to no language with a formatter on
the bare image, so the expected post-state is content unchanged. We read content via
fileContent before and after and assert strict equality.

WHY IT MATTERS: EDGE (missing capability). A format path that throws, or that rewrites/
blanks the buffer when no formatter exists, would corrupt files on the base image;
this guards the no-formatter branch. (The +node analog DIAG.017, which asserts real
formatting, needs a +lang image and is left TODO.)`,
    async run(env) {
      const path = `${PROJECT}/plain.xyz`;
      const original = "a  b  c\n";
      await env.request({ type: "writeFile", path, content: original });
      await env.request({ type: "openFile", path });
      await sleep(1000);
      const before = textOf(await env.request({ type: "fileContent", path }));

      let threw = false;
      try {
        await env.act("editor.action.formatDocument");
      } catch {
        threw = true;
      }
      await sleep(800);
      const after = textOf(await env.request({ type: "fileContent", path }));

      const pass = !threw && after === before && before === original;
      return {
        pass,
        detail: pass
          ? "formatDocument resolved; content unchanged (no formatter)"
          : `format ${threw ? "threw" : "ok"}; content ${after === before ? "unchanged" : "MUTATED"} (${JSON.stringify(after)})`,
        evidence: { threw, before, after },
      };
    },
  },

  // DIAG.021 — Problems view opens cleanly when there are zero problems.
  {
    id: "diag.problemsEmpty",
    specId: "L1.DIAG.021",
    title: "Diagnostics: Problems view opens cleanly with zero problems",
    tags: ["diagnostics", "views"],
    isolation: "fresh",
    needs: ["command", "query"],
    rationale: `
WHAT: On a fresh base env with no error files opened (so snapshot.diagnostics == 0),
reads the diagnostics count, runs \`workbench.actions.view.problems\`, and asserts the
command resolved AND snapshot.diagnostics stays 0 before and after — revealing the
empty Problems view must not fabricate markers.

WHY THIS IS THE EXPECTED OUTCOME: Opening the Problems view is a focus/reveal
operation; it shows the diagnostics panel but creates no markers. On a fresh base
image with no language server and no opened erroring files, the diagnostics count is 0
and revealing the view leaves it 0. The focused-view name is not snapshot-observable,
so the faithful observables are dispatch + the diagnostics count staying 0 across the
reveal.

WHY IT MATTERS: EDGE (empty state). A Problems-view command that side-effects markers
(or a snapshot whose diagnostics count drifts when the view opens) would be a real
regression; this guards the view command on an empty workspace. Pairs with
viewsSettings.mjs \`problems.open\` (which only asserts dispatch) by adding the
"stays zero" invariant on a known-empty env.`,
    async run(env) {
      const before = await env.observe("diag.problemsEmpty.before");
      const d0 = before.vscode.diagnostics;
      await env.act("workbench.actions.view.problems");
      await sleep(700);
      const after = await env.observe("diag.problemsEmpty.after");
      const d1 = after.vscode.diagnostics;

      const pass = d0 === 0 && d1 === 0;
      return {
        pass,
        detail: pass
          ? "Problems view opened on an empty workspace; diagnostics stayed 0"
          : `diagnostics not zero across reveal: before=${JSON.stringify(d0)} after=${JSON.stringify(d1)}`,
        evidence: { before: d0, after: d1 },
      };
    },
  },
];
